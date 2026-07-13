use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BASE_URL: &str = "https://api.letterboxd.com/api/v0";
const LOG_ENTRIES_CACHE: &str = "log_entries.json";
const WATCHLIST_CACHE: &str = "watchlist.json";
const CACHE_TTL_SECS: u64 = 3600;

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

#[derive(Debug, Deserialize)]
struct WatchlistResponse {
    items: Vec<WatchlistItem>,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WatchlistItem {
    film: Film,
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

// ── Caché de log entries ────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CachedEntries {
    timestamp: u64,
    entries: Vec<LogEntry>,
}

fn cache_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("letterboxd-cli");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(LOG_ENTRIES_CACHE))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("El tiempo no puede ir hacia atrás")
        .as_secs()
}

fn load_cached_entries() -> Option<Vec<LogEntry>> {
    let path = cache_path().ok()?;
    let data = std::fs::read_to_string(path).ok()?;
    let cached: CachedEntries = serde_json::from_str(&data).ok()?;
    if now_unix() - cached.timestamp < CACHE_TTL_SECS {
        Some(cached.entries)
    } else {
        None
    }
}

fn save_entries_cache(entries: &[LogEntry]) -> Result<()> {
    let cached = CachedEntries {
        timestamp: now_unix(),
        entries: entries.to_vec(),
    };
    let path = cache_path()?;
    let json = serde_json::to_string(&cached)?;
    std::fs::write(path, json).context("No se puede guardar la caché de log entries")?;
    Ok(())
}

// ── Caché de watchlist ──────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CachedWatchlist {
    timestamp: u64,
    films: Vec<Film>,
}

fn watchlist_cache_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("letterboxd-cli");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(WATCHLIST_CACHE))
}

fn load_cached_watchlist() -> Option<Vec<Film>> {
    let path = watchlist_cache_path().ok()?;
    let data = std::fs::read_to_string(path).ok()?;
    let cached: CachedWatchlist = serde_json::from_str(&data).ok()?;
    if now_unix() - cached.timestamp < CACHE_TTL_SECS {
        Some(cached.films)
    } else {
        None
    }
}

fn save_watchlist_cache(films: &[Film]) -> Result<()> {
    let cached = CachedWatchlist {
        timestamp: now_unix(),
        films: films.to_vec(),
    };
    let path = watchlist_cache_path()?;
    let json = serde_json::to_string(&cached)?;
    std::fs::write(path, json).context("No se puede guardar la caché de watchlist")?;
    Ok(())
}

// ── Cliente de Letterboxd ───────────────────────────────────────────────────

pub struct LetterboxdClient<'a> {
    http: &'a reqwest::Client,
    token: &'a str,
}

impl<'a> LetterboxdClient<'a> {
    pub fn new(http: &'a reqwest::Client, token: &'a str) -> Self {
        Self { http, token }
    }

    async fn get_member_id(&self) -> Result<String> {
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

        let raw = resp.text().await.context("Error al leer body de /me")?;
        let me: MeResponse = serde_json::from_str(&raw).context("Error al parsear /me")?;
        Ok(me.member.id)
    }

    pub async fn get_log_entries(&self) -> Result<Vec<LogEntry>> {
        if let Some(entries) = load_cached_entries() {
            return Ok(entries);
        }

        let member_id = self.get_member_id().await?;

        let mut all_entries: Vec<LogEntry> = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url = format!("{BASE_URL}/log-entries?member={member_id}&perPage=100");
            if let Some(ref c) = cursor {
                url.push_str(&format!("&cursor={c}"));
            }

            let resp = self
                .http
                .get(&url)
                .bearer_auth(self.token)
                .send()
                .await
                .context("Error al obtener log entries")?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Error en log-entries ({status}): {body}");
            }

            let page: LogEntriesResponse =
                resp.json().await.context("Error al parsear log entries")?;

            all_entries.extend(page.items);

            match page.next {
                Some(next_cursor) => cursor = Some(next_cursor),
                None => break,
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        save_entries_cache(&all_entries).ok();
        Ok(all_entries)
    }

    /// Obtiene las películas en la watchlist del usuario (paginado, cacheado).
    pub async fn get_watchlist(&self) -> Result<Vec<Film>> {
        if let Some(films) = load_cached_watchlist() {
            return Ok(films);
        }

        let member_id = self.get_member_id().await?;

        let mut all_films: Vec<Film> = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url = format!("{BASE_URL}/members/{member_id}/watchlist?perPage=100");
            if let Some(ref c) = cursor {
                url.push_str(&format!("&cursor={c}"));
            }

            let resp = self
                .http
                .get(&url)
                .bearer_auth(self.token)
                .send()
                .await
                .context("Error al obtener watchlist")?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Error en watchlist ({status}): {body}");
            }

            let page: WatchlistResponse =
                resp.json().await.context("Error al parsear watchlist")?;

            all_films.extend(page.items.into_iter().map(|i| i.film));

            match page.next {
                Some(next_cursor) => cursor = Some(next_cursor),
                None => break,
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        save_watchlist_cache(&all_films).ok();
        Ok(all_films)
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
