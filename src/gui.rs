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
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use tokio::sync::Mutex;

use crate::auth;
use crate::config::Config;
use crate::credentials::{self, Credentials};
use crate::letterboxd::LetterboxdClient;
use crate::preferences::{self, Preferences};
use crate::progress::Progress;
use crate::recommend::{build_recommendations, Recommendation};
use crate::stream::{self, StreamHandle, StreamStats};
use crate::subtitles::{self, Subtitle};
use crate::tmdb::{MovieView, TmdbClient, TmdbMovie};
use crate::torrents::{self, AudioHint, MovieQuery, Torrent};

const SEARCH_CACHE_FILE: &str = "search_cache.json";
const SEARCH_CACHE_TTL_SECS: u64 = 24 * 3600;

/// Entrada del cache de `search_movies_tmdb` (persistido en disco).
/// La key es la query normalizada (trim + lowercase). El valor es el
/// resultado final ya filtrado por presencia de torrents — cachear el
/// resultado enriquecido evita repetir el sondeo caro a los providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedSearch {
    timestamp: u64,
    hits: Vec<MovieHit>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("El tiempo no puede ir hacia atrás")
        .as_secs()
}

fn config_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("videodrome");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn search_cache_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join(SEARCH_CACHE_FILE))
}

fn load_search_cache() -> HashMap<String, CachedSearch> {
    let Ok(path) = search_cache_path() else {
        return HashMap::new();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_search_cache(cache: &HashMap<String, CachedSearch>) {
    if let Ok(path) = search_cache_path() {
        if let Ok(json) = serde_json::to_string(cache) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn normalize_query(q: &str) -> String {
    q.trim().to_lowercase()
}

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
    /// Caché en memoria (y persistida) de `search_movies_tmdb`. Evita
    /// repetir el sondeo a providers cuando el user repite una búsqueda.
    search_cache: Arc<Mutex<HashMap<String, CachedSearch>>>,
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
    tmdb.get_movie_view(tmdb_id)
        .await
        .map_err(|e| e.to_string())
}

/// Búsqueda TMDB por texto libre. Alimenta la pantalla intermedia de la
/// GUI: el user teclea "matrix" y ve posters de todas las coincidencias
/// antes de decidir de cuál quiere torrents. Evita el problema de "he
/// pedido una peli y me han salido resultados de otra distinta".
///
/// Cada hit de TMDB se cruza en paralelo con los providers de torrents:
/// si ninguno devuelve nada con seeders ≥ 1, la película se descarta —
/// no tiene sentido enseñar carátulas de pelis que después no vamos a
/// poder ver. Se preserva el orden de relevancia de TMDB.
#[tauri::command]
async fn search_movies_tmdb(
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<MovieHit>, String> {
    let key = normalize_query(&query);
    if key.is_empty() {
        return Ok(vec![]);
    }

    // Cache hit: si tenemos resultados recientes para la query
    // normalizada, devolvemos sin tocar TMDB ni providers.
    {
        let cache = state.search_cache.lock().await;
        if let Some(cached) = cache.get(&key) {
            if now_unix() - cached.timestamp < SEARCH_CACHE_TTL_SECS {
                return Ok(cached.hits.clone());
            }
        }
    }

    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer);
    let movies = tmdb.search_movies(&query).await.map_err(|e| e.to_string())?;

    let providers = torrents::default_providers();
    let http = state.http.clone();

    // Sondeo ligero por película en paralelo (concurrencia 6 para no
    // saturar Knaben/YTS). Pedimos solo 5 resultados por película, lo
    // justo para saber si hay algo con seeders. min_seeders=1 filtra
    // torrents muertos ya en el provider.
    let checks = movies.into_iter().enumerate().map(|(idx, m)| {
        let providers = providers.clone();
        let http = http.clone();
        async move {
            let q = MovieQuery {
                title: m.title.clone(),
                year: m.year(),
                imdb_id: None,
                tmdb_id: Some(m.id),
                original_language: None,
            };
            let list = torrents::search_all(&http, &providers, &q, 1, 5).await;
            (idx, m, list.len() as u32)
        }
    });

    let mut results: Vec<(usize, TmdbMovie, u32)> = futures::stream::iter(checks)
        .buffer_unordered(6)
        .filter(|(_, _, n)| futures::future::ready(*n > 0))
        .collect()
        .await;

    // FuturesUnordered rompe el orden; restauramos el de TMDB (por
    // relevancia) que es lo que el user espera visualmente.
    results.sort_by_key(|(idx, _, _)| *idx);

    let hits: Vec<MovieHit> = results
        .into_iter()
        .map(|(_, movie, torrent_count)| MovieHit {
            movie,
            torrent_count,
        })
        .collect();

    // Persistir en cache. Solo cacheamos hits no vacíos: si no ha salido
    // nada la próxima vez volvemos a preguntar (los indexadores pueden
    // haber revivido). Se guarda de forma tolerante: fallar aquí no
    // rompe la respuesta al frontend.
    if !hits.is_empty() {
        let mut cache = state.search_cache.lock().await;
        cache.insert(
            key,
            CachedSearch {
                timestamp: now_unix(),
                hits: hits.clone(),
            },
        );
        save_search_cache(&cache);
    }

    Ok(hits)
}

/// Película de TMDB anotada con el número de torrents que los providers
/// devolvieron para ella. Se usa en la pantalla intermedia de búsqueda.
/// `Deserialize` para poder rehidratarlo desde el cache en disco.
#[derive(Debug, Serialize, Deserialize, Clone)]
struct MovieHit {
    #[serde(flatten)]
    movie: TmdbMovie,
    torrent_count: u32,
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
        original_language: original_language.clone(),
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
                original_language: original_language.clone(),
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
        original_language: None,
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
async fn start_stream(magnet: String, state: State<'_, AppState>) -> Result<StreamInfo, String> {
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
async fn stream_stats(id: u64, state: State<'_, AppState>) -> Result<StreamStatsDto, String> {
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
    let dest = std::env::temp_dir().join("videodrome-subs");
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

// ---------- Ajustes: caché + preferencias ----------

/// Descripción de un archivo de caché para la vista Ajustes. El frontend
/// pinta esto en una tabla y ofrece un botón "Borrar" por fila.
#[derive(Serialize)]
struct CacheEntry {
    /// Identificador estable que usa `clear_cache` (`"log_entries"`,
    /// `"watchlist"`, `"tmdb_recs"`, `"search"`).
    kind: &'static str,
    /// Etiqueta legible para la UI.
    label: &'static str,
    /// Ruta absoluta del archivo (por si el user quiere inspeccionarlo).
    path: String,
    exists: bool,
    size_bytes: u64,
    /// Última modificación en segundos desde epoch (0 si no existe).
    modified_at: u64,
}

fn cache_files() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("log_entries", "Historial Letterboxd", "log_entries.json"),
        ("watchlist", "Watchlist Letterboxd", "watchlist.json"),
        ("tmdb_recs", "Recomendaciones TMDB", "tmdb_recs_cache.json"),
        ("search", "Búsquedas TMDB + torrents", SEARCH_CACHE_FILE),
    ]
}

#[tauri::command]
async fn cache_info() -> Result<Vec<CacheEntry>, String> {
    let dir = config_dir().map_err(|e| e.to_string())?;
    Ok(cache_files()
        .into_iter()
        .map(|(kind, label, file)| {
            let path = dir.join(file);
            let (exists, size_bytes, modified_at) = match std::fs::metadata(&path) {
                Ok(m) => {
                    let ts = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    (true, m.len(), ts)
                }
                Err(_) => (false, 0, 0),
            };
            CacheEntry {
                kind,
                label,
                path: path.display().to_string(),
                exists,
                size_bytes,
                modified_at,
            }
        })
        .collect())
}

/// Borra uno o todos los ficheros de caché. `kind = "all"` los borra
/// todos de golpe. Nunca borra `token.json` — la sesión se cierra con
/// `logout`, no aquí.
#[tauri::command]
async fn clear_cache(kind: String, state: State<'_, AppState>) -> Result<(), String> {
    let dir = config_dir().map_err(|e| e.to_string())?;
    let known = cache_files();
    let to_delete: Vec<&'static str> = if kind == "all" {
        known.iter().map(|(_, _, f)| *f).collect()
    } else {
        known
            .iter()
            .find(|(k, _, _)| *k == kind)
            .map(|(_, _, f)| vec![*f])
            .ok_or_else(|| format!("caché desconocida: {kind}"))?
    };

    for file in to_delete {
        let path = dir.join(file);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Error al borrar {}: {e}", path.display()))?;
        }
    }

    // El cache de búsqueda vive también en memoria: si lo borramos del
    // disco pero no del state, la siguiente consulta vuelve a escribirlo
    // con los datos viejos. Vaciar el mapa cuando corresponda.
    if kind == "all" || kind == "search" {
        state.search_cache.lock().await.clear();
    }
    Ok(())
}

#[tauri::command]
async fn get_preferences() -> Result<Preferences, String> {
    Ok(preferences::load())
}

#[tauri::command]
async fn set_preferences(prefs: Preferences) -> Result<(), String> {
    preferences::save(&prefs).map_err(|e| e.to_string())
}

// ---------- Entry point ----------

pub fn run(config: Config, http: reqwest::Client) -> anyhow::Result<()> {
    let state = AppState {
        config: Arc::new(Mutex::new(config)),
        http,
        streams: Arc::new(Mutex::new(HashMap::new())),
        next_stream_id: Arc::new(Mutex::new(0)),
        pending_subs: Arc::new(Mutex::new(HashMap::new())),
        search_cache: Arc::new(Mutex::new(load_search_cache())),
    };

    tauri::Builder::default()
        // Single-instance: si el usuario hace doble click en el atajo del
        // Start Menu varias veces (o Windows re-lanza el .exe por
        // cualquier motivo), en lugar de abrir N ventanas Tauri, el
        // segundo (y sucesivos) procesos salen inmediatamente después de
        // notificar al primero, que trae su ventana al foco.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            use tauri::Manager;
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
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
            search_movies_tmdb,
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
            cache_info,
            clear_cache,
            get_preferences,
            set_preferences,
        ])
        .run(tauri::generate_context!())
        .context("Error al ejecutar la app Tauri")
}
