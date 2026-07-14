use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const BASE_URL: &str = "https://api.letterboxd.com/api/v0";
const LOG_ENTRIES_CACHE: &str = "log_entries.json";
const WATCHLIST_CACHE: &str = "watchlist.json";
const CACHE_TTL_SECS: u64 = 3600;

/// Contrato mínimo para una respuesta paginada de la API de Letterboxd.
/// Las respuestas de `/log-entries`, `/watchlist`, etc. tienen todas la
/// misma forma: una lista de `items` y un `next` opcional (cursor).
trait Paginated: DeserializeOwned {
    type Item;
    fn drain(self) -> (Vec<Self::Item>, Option<String>);
}

// ── Estructuras de respuesta ────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FilmLink {
    #[serde(rename = "type")]
    pub link_type: String,
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Film {
    pub id: String,
    pub name: String,
    pub links: Option<Vec<FilmLink>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LogEntry {
    pub film: Film,
    pub rating: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct LogEntriesResponse {
    items: Vec<LogEntry>,
    next: Option<String>,
}

impl Paginated for LogEntriesResponse {
    type Item = LogEntry;
    fn drain(self) -> (Vec<Self::Item>, Option<String>) {
        (self.items, self.next)
    }
}

#[derive(Debug, Deserialize)]
struct WatchlistResponse {
    items: Vec<Film>,
    next: Option<String>,
}

impl Paginated for WatchlistResponse {
    type Item = Film;
    fn drain(self) -> (Vec<Self::Item>, Option<String>) {
        (self.items, self.next)
    }
}

#[derive(Debug, Deserialize)]
struct FilmSummaryResponse {
    items: Vec<FilmSummaryItem>,
}

#[derive(Debug, Deserialize)]
struct FilmSummaryItem {
    rating: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct MemberSummary {
    id: String,
}

#[derive(Debug, Deserialize)]
struct MeResponse {
    member: MemberSummary,
}

// ── Caché en disco (log entries + watchlist) ────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Cached<T> {
    timestamp: u64,
    #[serde(alias = "entries", alias = "films")]
    items: Vec<T>,
}

fn cache_path(file: &str) -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("videodrome");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(file))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("El tiempo no puede ir hacia atrás")
        .as_secs()
}

fn load_cache<T: DeserializeOwned>(file: &str) -> Option<Vec<T>> {
    let path = cache_path(file).ok()?;
    let data = std::fs::read_to_string(path).ok()?;
    let cached: Cached<T> = serde_json::from_str(&data).ok()?;
    (now_unix() - cached.timestamp < CACHE_TTL_SECS).then_some(cached.items)
}

fn save_cache<T: Serialize>(file: &str, items: &[T]) {
    let Ok(path) = cache_path(file) else { return };
    let payload = serde_json::json!({ "timestamp": now_unix(), "items": items });
    if let Ok(json) = serde_json::to_string(&payload) {
        let _ = std::fs::write(path, json);
    }
}

// ── Cliente de Letterboxd ───────────────────────────────────────────────────

pub struct LetterboxdClient<'a> {
    http: &'a reqwest::Client,
    token: &'a str,
    /// Cache del member ID en memoria — evita una llamada duplicada a
    /// `/me` cuando `get_log_entries` y `get_watchlist` se lanzan en
    /// paralelo con `try_join!`.
    member_id: Mutex<Option<String>>,
}

impl<'a> LetterboxdClient<'a> {
    pub fn new(http: &'a reqwest::Client, token: &'a str) -> Self {
        Self {
            http,
            token,
            member_id: Mutex::new(None),
        }
    }

    async fn get_member_id(&self) -> Result<String> {
        if let Some(id) = self.member_id.lock().unwrap().clone() {
            return Ok(id);
        }

        let resp = self
            .http
            .get(format!("{BASE_URL}/me"))
            .bearer_auth(self.token)
            .send()
            .await
            .context("Error al obtener /me")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Error en /me ({status}): {body}");
        }

        let me: MeResponse = resp.json().await.context("Error al parsear /me")?;
        *self.member_id.lock().unwrap() = Some(me.member.id.clone());
        Ok(me.member.id)
    }

    /// Recorre un endpoint paginado de Letterboxd (cursor-based). Cada
    /// página trae `items` + `next` (Option). Se detiene cuando `next` es
    /// `None`.
    async fn paginate<R>(&self, base: &str) -> Result<Vec<R::Item>>
    where
        R: Paginated,
    {
        let mut items: Vec<R::Item> = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let url = match &cursor {
                Some(c) => format!("{base}&cursor={c}"),
                None => base.to_string(),
            };

            let resp = self
                .http
                .get(&url)
                .bearer_auth(self.token)
                .send()
                .await
                .with_context(|| format!("Error al GET {base}"))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Error paginando {base} ({status}): {body}");
            }

            let page: R = resp
                .json()
                .await
                .with_context(|| format!("Error al parsear {base}"))?;
            let (page_items, next) = page.drain();
            items.extend(page_items);

            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }

        Ok(items)
    }

    pub async fn get_log_entries(&self) -> Result<Vec<LogEntry>> {
        if let Some(entries) = load_cache::<LogEntry>(LOG_ENTRIES_CACHE) {
            return Ok(entries);
        }

        let member_id = self.get_member_id().await?;
        let base = format!("{BASE_URL}/log-entries?member={member_id}&perPage=100");
        let entries = self.paginate::<LogEntriesResponse>(&base).await?;

        save_cache(LOG_ENTRIES_CACHE, &entries);
        Ok(entries)
    }

    /// Obtiene las películas en la watchlist del usuario (paginado, cacheado).
    pub async fn get_watchlist(&self) -> Result<Vec<Film>> {
        if let Some(films) = load_cache::<Film>(WATCHLIST_CACHE) {
            return Ok(films);
        }

        let member_id = self.get_member_id().await?;
        let base = format!("{BASE_URL}/member/{member_id}/watchlist?perPage=100");
        let films = self.paginate::<WatchlistResponse>(&base).await?;

        save_cache(WATCHLIST_CACHE, &films);
        Ok(films)
    }

    /// Obtiene el rating comunitario de Letterboxd para un film dado su TMDB ID.
    pub async fn get_lb_rating(&self, tmdb_id: u64) -> Option<f32> {
        let url = format!("{BASE_URL}/films?filmId=tmdb:{tmdb_id}&perPage=1");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.token)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: FilmSummaryResponse = resp.json().await.ok()?;
        body.items.into_iter().next().and_then(|f| f.rating)
    }
}
