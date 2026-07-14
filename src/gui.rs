//! GUI (Tauri) backend. Solo se compila con `--features gui` (activa por
//! defecto). Reutiliza los módulos existentes (`auth`, `letterboxd`,
//! `tmdb`, `recommend`, `torrents`, `stream`, `subtitles`, `credentials`)
//! y los expone al frontend React como `#[tauri::command]`.
//!
//! Comandos expuestos, agrupados por vista:
//! - Sesión: `has_session`, `login`
//! - Recomendaciones: `get_recommendations`
//! - Torrents: `search_torrents_by_tmdb`, `search_torrents_direct`,
//!   `open_magnet`
//! - Streaming: `start_stream`, `stream_stats`, `stop_stream`
//! - Subtítulos: `search_subtitles`, `download_subtitle`

use anyhow::Context;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

use crate::auth;
use crate::config::Config;
use crate::credentials::{self, Credentials};
use crate::letterboxd::LetterboxdClient;
use crate::progress::Progress;
use crate::recommend::{build_recommendations, Recommendation};
use crate::stream::{self, StreamHandle, StreamStats};
use crate::subtitles::{self, Subtitle};
use crate::tmdb::{MovieView, TmdbClient};
use crate::torrents::{self, AudioHint, MovieQuery, Torrent};

/// Estado global compartido con los comandos Tauri.
pub struct AppState {
    config: Arc<Mutex<Config>>,
    http: reqwest::Client,
    /// Streams activos indexados por id. La TUI solo tiene uno a la vez,
    /// aquí también, pero un `HashMap` permite polling limpio.
    streams: Arc<Mutex<HashMap<u64, StreamHandle>>>,
    next_stream_id: Arc<Mutex<u64>>,
    /// Subs descargados para pasarlos a VLC (`--sub-file=…`) al lanzar el
    /// stream. Indexado por `stream_id`.
    pending_subs: Arc<Mutex<HashMap<u64, PathBuf>>>,
}

/// Progress no-op: la GUI no necesita ver las etapas por ahora.
struct Silent;
impl Progress for Silent {
    fn stage(&self, _msg: &str, _total: u64) {}
    fn inc(&self) {}
    fn finish(&self) {}
}

// ---------- Sesión ----------

#[tauri::command]
async fn has_session(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.config.lock().await.refresh_token.is_some())
}

#[tauri::command]
async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    credentials::clear().map_err(|e| e.to_string())?;
    let mut cfg = state.config.lock().await;
    cfg.refresh_token = None;
    cfg.username = String::new();
    Ok(())
}

#[tauri::command]
async fn login(
    username: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let (client_id, client_secret) = {
        let cfg = state.config.lock().await;
        (cfg.client_id.clone(), cfg.client_secret.clone())
    };
    let res = auth::login_with_password(
        &state.http,
        &client_id,
        &client_secret,
        &username,
        &password,
    )
    .await
    .map_err(|e| e.to_string())?;

    let creds = Credentials {
        refresh_token: Some(res.refresh_token.clone()),
        username: Some(username.clone()),
    };
    credentials::save(&creds).map_err(|e| e.to_string())?;

    let mut cfg = state.config.lock().await;
    cfg.refresh_token = Some(res.refresh_token);
    cfg.username = username.clone();
    Ok(username)
}

#[tauri::command]
async fn get_username(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.config.lock().await.username.clone())
}

// ---------- Recomendaciones ----------

#[tauri::command]
async fn get_recommendations(
    count: usize,
    min_rating: f32,
    state: State<'_, AppState>,
) -> Result<Vec<Recommendation>, String> {
    let config = state.config.lock().await.clone();
    let token = auth::get_access_token(&state.http, &config)
        .await
        .map_err(|e| e.to_string())?;
    let lb = LetterboxdClient::new(&state.http, &token);
    let tmdb = TmdbClient::new(&state.http, &config.tmdb_bearer_token);
    build_recommendations(&lb, &tmdb, count, min_rating, &Silent)
        .await
        .map_err(|e| e.to_string())
}

/// Detalle de una película para el modal estilo Stremio: sinopsis,
/// backdrop, runtime, géneros.
#[tauri::command]
async fn get_movie_view(
    tmdb_id: u64,
    state: State<'_, AppState>,
) -> Result<Option<MovieView>, String> {
    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer);
    tmdb.get_movie_view(tmdb_id).await.map_err(|e| e.to_string())
}

// ---------- Torrents ----------

/// Datos enriquecidos que la GUI muestra sobre la película en la vista de
/// torrents (título original + IMDb + idioma) además de la lista.
#[derive(Serialize)]
struct TorrentSearchResult {
    title: String,
    imdb_id: Option<String>,
    original_language: Option<String>,
    year: Option<u16>,
    results: Vec<TorrentDto>,
}

/// Torrent con el idioma de audio inferido (para la bandera en la UI).
/// Espejo de `Torrent` + `audio`.
#[derive(Serialize)]
struct TorrentDto {
    title: String,
    magnet: String,
    size_bytes: u64,
    seeders: u32,
    leechers: u32,
    quality: Option<String>,
    source: &'static str,
    /// Código ISO 639-1 del audio inferido (`"en"`, `"es"`, `"ru"`…) o
    /// marcador especial (`"multi"`, `"unknown"`, `"dub"`).
    audio: String,
}

impl TorrentDto {
    fn from_torrent(t: Torrent, original_language: Option<&str>) -> Self {
        let hint = torrents::classify_audio(&t.title, original_language);
        let audio = match hint {
            AudioHint::Original => original_language
                .filter(|s| !s.is_empty())
                .unwrap_or("orig")
                .to_string(),
            AudioHint::Dubbed("??") => "dub".to_string(),
            AudioHint::Dubbed(l) => l.to_string(),
            AudioHint::Multi => "multi".to_string(),
            AudioHint::Unknown => "unknown".to_string(),
        };
        Self {
            title: t.title,
            magnet: t.magnet,
            size_bytes: t.size_bytes,
            seeders: t.seeders,
            leechers: t.leechers,
            quality: t.quality,
            source: t.source,
            audio,
        }
    }
}

/// Búsqueda a partir de una película Letterboxd (recomendación con TMDB
/// id). Reproduce `spawn_torrents` de la TUI: resuelve detalles TMDB
/// (título original, IMDb, idioma) antes de consultar los providers.
#[tauri::command]
async fn search_torrents_by_tmdb(
    tmdb_id: u64,
    fallback_title: String,
    fallback_year: Option<u16>,
    state: State<'_, AppState>,
) -> Result<TorrentSearchResult, String> {
    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer);
    let details = tmdb.get_movie_details(tmdb_id).await.ok().flatten();
    let (title, russian_title, year, imdb_id, original_language) = match details {
        Some(d) => (
            d.original_title
                .or(d.fallback_title)
                .unwrap_or(fallback_title.clone()),
            d.russian_title,
            d.year.or(fallback_year),
            d.imdb_id,
            d.original_language,
        ),
        None => (fallback_title.clone(), None, fallback_year, None, None),
    };

    let providers = torrents::default_providers();
    let primary = MovieQuery {
        title: title.clone(),
        year,
        imdb_id: imdb_id.clone(),
        tmdb_id: Some(tmdb_id),
    };
    let mut list = torrents::search_all(&state.http, &providers, &primary, 1, 40).await;

    // Fallback ruso, como en la TUI.
    if list.is_empty() {
        if let Some(ru) = russian_title.filter(|s| s != &title) {
            let ru_q = MovieQuery {
                title: ru.clone(),
                year,
                imdb_id: imdb_id.clone(),
                tmdb_id: Some(tmdb_id),
            };
            let raw = torrents::search_all(&state.http, &providers, &ru_q, 1, 40).await;
            list = raw
                .into_iter()
                .filter(|t| t.title.starts_with(&ru))
                .collect();
        }
    }

    Ok(TorrentSearchResult {
        title,
        imdb_id,
        original_language: original_language.clone(),
        year,
        results: list
            .into_iter()
            .map(|t| TorrentDto::from_torrent(t, original_language.as_deref()))
            .collect(),
    })
}

/// Búsqueda directa: el user teclea un título en la vista Search. No pasa
/// por Letterboxd/TMDB.
#[tauri::command]
async fn search_torrents_direct(
    query: String,
    state: State<'_, AppState>,
) -> Result<TorrentSearchResult, String> {
    // Extrae año trailing si viene ("Funny Games 2007").
    let (title, year) = split_trailing_year(&query);
    let providers = torrents::default_providers();
    let q = MovieQuery {
        title: title.clone(),
        year,
        imdb_id: None,
        tmdb_id: None,
    };
    let list = torrents::search_all(&state.http, &providers, &q, 1, 40).await;
    Ok(TorrentSearchResult {
        title,
        imdb_id: None,
        original_language: None,
        year,
        results: list
            .into_iter()
            .map(|t| TorrentDto::from_torrent(t, None))
            .collect(),
    })
}

fn split_trailing_year(s: &str) -> (String, Option<u16>) {
    let trimmed = s.trim();
    if trimmed.len() > 5 {
        let (rest, tail) = trimmed.split_at(trimmed.len() - 5);
        if let Some(year_str) = tail.strip_prefix(' ') {
            if let Ok(y) = year_str.parse::<u16>() {
                if (1888..=2100).contains(&y) {
                    return (rest.trim_end().to_string(), Some(y));
                }
            }
        }
    }
    (trimmed.to_string(), None)
}

#[tauri::command]
fn open_magnet(magnet: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let out = std::process::Command::new("open").arg(&magnet).spawn();
    #[cfg(target_os = "linux")]
    let out = std::process::Command::new("xdg-open").arg(&magnet).spawn();
    #[cfg(target_os = "windows")]
    let out = std::process::Command::new("cmd")
        .args(["/C", "start", "", &magnet])
        .spawn();
    out.map(|_| ()).map_err(|e| e.to_string())
}

// ---------- Streaming ----------

#[derive(Serialize)]
struct StreamInfo {
    id: u64,
    url: String,
    file_name: String,
}

#[tauri::command]
async fn start_stream(
    magnet: String,
    state: State<'_, AppState>,
) -> Result<StreamInfo, String> {
    let handle = stream::start(magnet).await.map_err(|e| e.to_string())?;

    let mut id_lock = state.next_stream_id.lock().await;
    *id_lock += 1;
    let id = *id_lock;
    drop(id_lock);

    let info = StreamInfo {
        id,
        url: handle.url.clone(),
        file_name: handle.file_name.clone(),
    };

    // Si hay sub descargado apuntado a este id (esperado antes de arrancar
    // stream), se pasa a VLC. Como el flujo es "arranca stream, después
    // VLC", solo consumimos si ya está registrado con el mismo id.
    let sub_path = state.pending_subs.lock().await.remove(&id);
    let _alive = stream::open_in_vlc(&handle.url, sub_path.as_deref());

    state.streams.lock().await.insert(id, handle);
    Ok(info)
}

#[derive(Serialize)]
struct StreamStatsDto {
    progress_bytes: u64,
    total_bytes: u64,
    live_peers: u32,
    down_mbps: f64,
    alive: bool,
}

#[tauri::command]
async fn stream_stats(
    id: u64,
    state: State<'_, AppState>,
) -> Result<StreamStatsDto, String> {
    let streams = state.streams.lock().await;
    let handle = streams
        .get(&id)
        .ok_or_else(|| format!("stream {id} no encontrado"))?;
    let StreamStats {
        progress_bytes,
        total_bytes,
        live_peers,
        down_mbps,
    } = handle.stats();
    Ok(StreamStatsDto {
        progress_bytes,
        total_bytes,
        live_peers,
        down_mbps,
        alive: true,
    })
}

#[tauri::command]
async fn stop_stream(id: u64, state: State<'_, AppState>) -> Result<(), String> {
    state.streams.lock().await.remove(&id);
    state.pending_subs.lock().await.remove(&id);
    Ok(())
}

// ---------- Subtítulos ----------

#[tauri::command]
async fn subtitles_available() -> Result<bool, String> {
    Ok(subtitles::is_available())
}

#[tauri::command]
async fn search_subtitles(
    imdb_id: Option<String>,
    query: Option<String>,
    languages: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Subtitle>, String> {
    let langs = languages.unwrap_or_else(|| subtitles::DEFAULT_LANGUAGES.to_string());
    subtitles::search(&state.http, imdb_id.as_deref(), query.as_deref(), &langs)
        .await
        .map_err(|e| e.to_string())
}

/// Descarga un subtítulo. Si `stream_id` viene, lo asocia a ese stream
/// para que `start_stream` lo pase a VLC como `--sub-file`. Si no, el
/// path devuelto puede pasarlo el frontend a la próxima llamada de
/// `start_stream_with_sub`.
#[tauri::command]
async fn download_subtitle(
    sub: Subtitle,
    stream_id: Option<u64>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let dest = std::env::temp_dir().join("letterboxd-cli-subs");
    let path = subtitles::download(&state.http, &sub, &dest)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(sid) = stream_id {
        state.pending_subs.lock().await.insert(sid, path.clone());
    }
    Ok(path.display().to_string())
}

/// Como `start_stream`, pero pasando explícitamente un path de subtítulo
/// para VLC. Útil cuando la GUI descarga el sub ANTES de decidir el id.
#[tauri::command]
async fn start_stream_with_sub(
    magnet: String,
    sub_path: Option<String>,
    state: State<'_, AppState>,
) -> Result<StreamInfo, String> {
    let handle = stream::start(magnet).await.map_err(|e| e.to_string())?;

    let mut id_lock = state.next_stream_id.lock().await;
    *id_lock += 1;
    let id = *id_lock;
    drop(id_lock);

    let sub = sub_path.map(PathBuf::from);
    let _alive = stream::open_in_vlc(&handle.url, sub.as_deref());

    let info = StreamInfo {
        id,
        url: handle.url.clone(),
        file_name: handle.file_name.clone(),
    };
    state.streams.lock().await.insert(id, handle);
    Ok(info)
}

// ---------- Entry point ----------

pub fn run(config: Config, http: reqwest::Client) -> anyhow::Result<()> {
    let state = AppState {
        config: Arc::new(Mutex::new(config)),
        http,
        streams: Arc::new(Mutex::new(HashMap::new())),
        next_stream_id: Arc::new(Mutex::new(0)),
        pending_subs: Arc::new(Mutex::new(HashMap::new())),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
            }
            let _ = app;
            Ok(())
        })
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            has_session,
            logout,
            login,
            get_username,
            get_recommendations,
            get_movie_view,
            search_torrents_by_tmdb,
            search_torrents_direct,
            open_magnet,
            start_stream,
            start_stream_with_sub,
            stream_stats,
            stop_stream,
            subtitles_available,
            search_subtitles,
            download_subtitle,
        ])
        .run(tauri::generate_context!())
        .context("Error al ejecutar la app Tauri")
}
