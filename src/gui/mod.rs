//! GUI (Tauri) backend. Solo se compila con `--features gui` (opt-in;
//! `default = []` en Cargo.toml — los builds CLI/TUI no lo tocan).
//! Reutiliza los módulos existentes (`auth`, `letterboxd`, `tmdb`,
//! `recommend`, `torrents`, `stream`, `subtitles`, `credentials`) y los
//! expone al frontend React como `#[tauri::command]`.
//!
//! Comandos expuestos, agrupados por vista:
//! - Sesión: `has_session`, `login`
//! - Recomendaciones: `get_recommendations`
//! - Torrents: `search_torrents_by_tmdb`, `search_torrents_direct`,
//!   `open_magnet`
//! - Streaming: `start_stream`, `stream_stats`, `stop_stream`
//! - Subtítulos: `search_subtitles`, `download_subtitle`
//!
//! ## Estructura de módulos (audit v1.2.0 C1, parcial)
//!
//! `cache.rs` — helpers puros de disco/caché (paths, load/save, keys,
//! TTL). Extraído por ser lógica pura sin `#[tauri::command]` — el
//! tipo natural para vivir en su propio módulo.
//!
//! Los 36 comandos Tauri permanecen en este archivo. La razón NO es
//! técnica (Tauri 2 acepta paths tipo `generate_handler![mod::cmd]`)
//! sino de coste/beneficio: partir los comandos requeriría reescribir
//! cada uno como `pub fn` en el submódulo, exportarlo, y verificar el
//! comportamiento a mano — sin tests de integración en la GUI eso es
//! trabajo con riesgo alto y payoff bajo. El límite de tamaño del
//! archivo es real (>2000 líneas) pero se soluciona mejor con nuevos
//! comandos ya nacidos en submódulos que refactoreando los existentes.

use anyhow::Context;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tauri::State;
use tokio::sync::Mutex;

use crate::auth;
use crate::config::Config;
use crate::credentials::{self, Credentials};
use crate::dismissed::{self, DismissedEntry};
use crate::letterboxd::LetterboxdClient;
use crate::preferences::{self, Preferences};
use crate::progress::Progress;
use crate::recommend::{build_candidate_pool, enrich_batch, Recommendation};
use crate::stream::{self, StreamHandle, StreamStats};
use crate::subtitles::{self, Subtitle};
use crate::tmdb::{self, MovieView, TmdbClient, TmdbMovie};
use crate::torrents::{
    self, merge_provider_statuses, release_starts_with, split_trailing_year, AudioHint, MovieQuery,
    ProviderStatus, Torrent,
};
use crate::watched::{self, WatchedEntry};
pub(super) mod cache;
use self::cache::{
    config_dir, current_ui_lang, load_search_cache, load_torrent_cache, normalize_query, now_unix,
    save_search_cache, torrent_cache_get_fresh, torrent_cache_key, torrent_cache_key_with_ep,
    torrent_cache_put,
};

// v2 sufijo (v1.7.2): el `torrent_count` de cada `MovieHit` cambió
// de significado entre v1.7.0/v1.7.1 (usaba título localizado como
// query → 0 hits para pelis no-inglesas) y v1.7.2 (usa
// `original_title`). Con el mismo filename los usuarios que
// arrancaron las versiones previas verían counts erróneos durante
// 24 h. Renombramos el fichero para forzar re-fetch inmediato al
// actualizar. `search_cache.json` queda huérfano y lo recoge el
// prune periódico.
const SEARCH_CACHE_FILE: &str = "search_cache_v2.json";
const SEARCH_CACHE_TTL_SECS: u64 = 24 * 3600;

/// Fase 4a — caché de resultados de búsqueda de torrents.
///
/// Es un caché DISTINTO del `search_cache.json` de arriba (que
/// guarda los hits de `search_movies_tmdb`, el buscador de TMDB por
/// texto que puebla la vista Search). Este es el resultado ya
/// enriquecido de `search_torrents_by_tmdb` / `search_torrents_direct`
/// — el sondeo caro a los 4 providers.
///
/// Política:
///   * TTL 30 min para resultados con torrents (`ttl_hits`).
///   * TTL 5 min para resultados vacíos (`ttl_empty`). El resultado
///     vacío también se cachea a propósito: evita martillear los
///     providers cuando el user vuelve una y otra vez a una peli
///     sin releases (típicamente estrenos que aún no han salido en
///     digital — ver Fase 4b para el mensaje al user).
///   * Key = `tt<imdb_id>` si TMDB nos lo dio, o `"direct:" +
///     norm(title) + ":year"` para búsquedas directas. Estable
///     entre sesiones.
const TORRENT_CACHE_FILE: &str = "torrent_search_cache.json";
const TORRENT_CACHE_TTL_HITS: u64 = 30 * 60;
const TORRENT_CACHE_TTL_EMPTY: u64 = 5 * 60;
/// TTL corto aplicado cuando ALGÚN provider falló durante la búsqueda
/// que produjo la entrada. Los errores transitorios (DNS bloqueado
/// puntual, mirror caído 30s, timeout ocasional) NO deberían
/// clavarse 30 min en la vista — cachearlos poco tiempo permite
/// que un reintento poco después vea un estado sano sin que el user
/// tenga que ir a Settings > Limpiar caché.
const TORRENT_CACHE_TTL_PARTIAL_FAIL: u64 = 60;

#[derive(Clone, Serialize, Deserialize)]
struct CachedTorrentSearch {
    timestamp: u64,
    result: TorrentSearchResult,
}

/// Entrada del cache de `search_movies_tmdb` (persistido en disco).
/// La key es la query normalizada (trim + lowercase). El valor es el
/// resultado final ya filtrado por presencia de torrents — cachear el
/// resultado enriquecido evita repetir el sondeo caro a los providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedSearch {
    timestamp: u64,
    hits: Vec<MovieHit>,
}

pub struct AppState {
    config: Arc<Mutex<Config>>,
    http: reqwest::Client,
    /// Streams activos indexados por id. La TUI solo tiene uno a la vez,
    /// aquí también, pero un `HashMap` permite polling limpio.
    streams: Arc<Mutex<HashMap<u64, ActiveStream>>>,
    next_stream_id: Arc<Mutex<u64>>,
    /// Caché en memoria (y persistida) de `search_movies_tmdb`. Evita
    /// repetir el sondeo a providers cuando el user repite una búsqueda.
    search_cache: Arc<Mutex<HashMap<String, CachedSearch>>>,
    /// Caché en memoria (y persistida) del sondeo de torrents
    /// (Fase 4a del audit). TTL 30 min para hits, 5 min para vacío.
    /// Al arrancar se lee de disco (`torrent_search_cache.json`) para
    /// que la primera visita a Torrents tras reabrir la app sea
    /// instantánea si estaba cacheada.
    torrent_cache: Arc<Mutex<HashMap<String, CachedTorrentSearch>>>,
    /// Pool de recomendaciones ya computadas para la sesión actual.
    /// `get_recommendations_page` sirve slices de aquí. Se invalida
    /// cuando cambia el `min_rating` o el user pulsa "Refrescar".
    /// TTL 1h para que la próxima apertura de la vista no vuelva a
    /// gastar 5-10s recomputando toda la pipeline (TMDB recs + LB
    /// ratings) si el user ya lo vio recientemente.
    recs_pool: Arc<Mutex<Option<RecsPool>>>,
}

/// Pool de recomendaciones cacheado en memoria, construido de forma
/// perezosa por lotes. La primera petición hace los pasos 1-3 del
/// pipeline (historial + TMDB recs + pre-score) para llenar
/// `candidates` — es barato, ~1-2s con caché caliente de TMDB. Cada
/// página del scroll infinito LB-enriquece el siguiente lote sobre
/// `candidates` y lo apendea a `enriched`.
///
/// Así la primera página aparece en ~1s (10 fetches de LB) en lugar
/// de ~10s (600 fetches), y el trabajo se difumina a medida que el
/// user scrollea.
struct RecsPool {
    /// `min_rating * 10` para comparar sin lío de f32. Cuando cambia,
    /// se invalida el pool entero.
    min_rating_x10: u32,
    /// Candidatos pre-scored por freq × TMDB, ya ordenados. Se llena
    /// una vez y no cambia hasta la próxima invalidación.
    candidates: Vec<(TmdbMovie, f32)>,
    /// Recomendaciones ya LB-enriquecidas, apendeadas por lotes.
    /// `enriched.len()` marca hasta dónde llega la ventana servible.
    enriched: Vec<Recommendation>,
    /// Índice del próximo candidato pendiente de enriquecer. Cuando
    /// `next_to_enrich == candidates.len()` se acabó el pool.
    next_to_enrich: usize,
    /// Set de `snapshot_start` de batches que están AHORA mismo
    /// enriqueciéndose (fuera del lock, en LB I/O). Si una segunda
    /// request llega mientras un enrich sigue vivo, ve su
    /// snapshot_start en este set y salta el enrich (no vuelve a
    /// pedir el mismo rango a Letterboxd). Sin esto, dos requests
    /// concurrentes contra el mismo cursor gastaban 2× llamadas a
    /// LB para el mismo trabajo, aunque el resultado del segundo
    /// se descartaba por el guard de `next_to_enrich`.
    in_flight: std::collections::HashSet<usize>,
    computed_at: u64,
}

const RECS_POOL_TTL_SECS: u64 = 3600;
/// Tamaño máximo del pool de candidatos que pre-scoreamos. Techo
/// del scroll infinito: si el user llega al final, hemos servido
/// todo lo que TMDB nos ha dado como plausible.
const RECS_POOL_CAP: usize = 500;
/// Tamaño mínimo de un batch de LB-enrich. Si el frontend pide una
/// página de 10, enriquecemos 10 (no overshooteamos: el orden
/// dentro del batch se ordena por score final, cross-batch se
/// preserva por TMDB pre-score — buena aproximación).
const RECS_BATCH_MIN: usize = 10;

/// Un stream vivo + (opcionalmente) el handle del reproductor externo.
/// `player = Some` es la ruta legacy con VLC como proceso hijo — la
/// UI polla `alive` para saber cuándo el user cerró VLC. `player =
/// None` es el modo player HTML embebido (view `Player.tsx`): la vida
/// del stream la controla el frontend con `stop_stream` explícito, y
/// `stream_stats.alive` es siempre `true` mientras el slot exista.
struct ActiveStream {
    handle: StreamHandle,
    player: Option<stream::PlayerHandle>,
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
    // Cierre "de verdad": borrar credenciales, el token de acceso
    // cacheado (si no, get_access_token seguiría devolviéndolo hasta 1h
    // después del logout) y los cachés de historial/watchlist (para que
    // si otro usuario entra a continuación no vea las recomendaciones
    // calculadas con datos del anterior).
    credentials::clear().map_err(|e| e.to_string())?;
    auth::clear_cached_token().map_err(|e| e.to_string())?;

    let dir = config_dir().map_err(|e| e.to_string())?;
    for file in [
        "log_entries.json",
        "watchlist.json",
        "tmdb_recs_cache.json",
        SEARCH_CACHE_FILE,
    ] {
        let p = dir.join(file);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    state.search_cache.lock().await.clear();
    // También el pool de recomendaciones en memoria — si no, el
    // siguiente `get_recommendations_page` (misma sesión, otro
    // user) serviría las recs computadas con el historial anterior
    // hasta que expirase el TTL de 1h.
    *state.recs_pool.lock().await = None;

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

/// Página de recomendaciones para el scroll infinito. La primera vez
/// (o tras `force_refresh = true` o cambio de `min_rating`) computa
/// un pool grande (`RECS_POOL_SIZE`) y lo cachea en memoria; las
/// siguientes llamadas sirven un slice del pool sin volver a pegar a
/// TMDB/Letterboxd. Al llegar al final, `has_more = false` — el
/// frontend deja de disparar fetches.
///
/// `dismissed` se filtra en cada request para que descartar una peli
/// en tiempo real la haga desaparecer sin recomputar el pool.
#[tauri::command]
async fn get_recommendations_page(
    offset: usize,
    limit: usize,
    min_rating: f32,
    force_refresh: bool,
    state: State<'_, AppState>,
) -> Result<RecsPage, String> {
    let key = (min_rating * 10.0).round() as u32;
    let now = now_unix();

    // ¿Necesitamos construir/reconstruir el pool de candidatos?
    // (No los "enriched" — esos se construyen incrementalmente por
    // lote a medida que el scroll los va necesitando.)
    let needs_rebuild = {
        let guard = state.recs_pool.lock().await;
        force_refresh
            || match guard.as_ref() {
                Some(p) => {
                    p.min_rating_x10 != key
                        || now.saturating_sub(p.computed_at) >= RECS_POOL_TTL_SECS
                }
                None => true,
            }
    };

    // Precomputamos config + clientes fuera del lock del pool.
    let config = state.config.lock().await.clone();

    if needs_rebuild {
        let token = auth::get_access_token(&state.http, &config)
            .await
            .map_err(|e| e.to_string())?;
        let lb = LetterboxdClient::new(&state.http, &token);
        let tmdb = TmdbClient::new(
            &state.http,
            &config.tmdb_bearer_token,
            current_ui_lang().as_deref(),
        );

        // Solo pasos 1-3 del pipeline: pre-score candidatos por
        // freq × TMDB. NO tocamos Letterboxd todavía — eso se
        // difiere al bucle de enriquecimiento incremental abajo.
        let candidates = build_candidate_pool(&lb, &tmdb, min_rating, RECS_POOL_CAP, &Silent)
            .await
            .map_err(|e| e.to_string())?;

        *state.recs_pool.lock().await = Some(RecsPool {
            min_rating_x10: key,
            candidates,
            enriched: Vec::new(),
            next_to_enrich: 0,
            in_flight: std::collections::HashSet::new(),
            computed_at: now,
        });
    }

    // Enriquecimiento incremental: LB-hydrate hasta que tengamos
    // `offset + limit` items (con margen ligero por dismisses).
    // Cada iteración enriquece un batch pequeño (`limit` items) en
    // paralelo con concurrencia 40 en `enrich_batch`. Para una
    // primera página de 10 son ~200-500ms en total, en vez de los
    // 5-10s del build_recommendations monolítico.
    loop {
        // Snapshot del estado bajo lock corto para decidir si hay
        // que enriquecer más y qué slice de candidatos coger.
        // Devolvemos también `snapshot_start` para poder detectar
        // si otro request avanzó el cursor mientras hacíamos LB.
        //
        // Si el snapshot_start ya está en `in_flight`, otro request
        // está enriqueciendo exactamente ese rango — salimos del
        // loop y servimos lo que ya tengamos: cuando el otro
        // termine, la próxima paginación verá el pool actualizado.
        // Esto evita 2× llamadas a LB para el mismo trabajo.
        let batch_to_enrich: Option<(usize, Vec<(TmdbMovie, f32)>)> = {
            let mut guard = state.recs_pool.lock().await;
            let pool = guard.as_mut().expect("just rebuilt");
            let target = offset + limit + RECS_BATCH_MIN; // margen anti-dismiss
            if pool.enriched.len() >= target || pool.next_to_enrich >= pool.candidates.len() {
                None
            } else {
                let start = pool.next_to_enrich;
                if pool.in_flight.contains(&start) {
                    None
                } else {
                    let batch_size = limit.max(RECS_BATCH_MIN);
                    let end = (start + batch_size).min(pool.candidates.len());
                    pool.in_flight.insert(start);
                    Some((start, pool.candidates[start..end].to_vec()))
                }
            }
        };
        let Some((snapshot_start, batch)) = batch_to_enrich else {
            break;
        };
        // LB-enrich fuera del lock (network I/O). El slot está
        // reservado en `in_flight`, así que ningún request concurrente
        // pedirá el mismo rango mientras estemos aquí.
        let token = auth::get_access_token(&state.http, &config)
            .await
            .map_err(|e| e.to_string())?;
        let lb = LetterboxdClient::new(&state.http, &token);
        let batch_len = batch.len();
        let new_recs = enrich_batch::<Silent>(&lb, &batch, None).await;
        // Volvemos a coger el lock para apendear y liberar el slot
        // `in_flight`. Solo aplicamos si `next_to_enrich` NO se
        // movió desde el snapshot: si otro request logró avanzar
        // pese al in_flight (p. ej. tras un logout que reset-eó el
        // pool y otro thread lo repobló), tiramos el trabajo.
        let mut guard = state.recs_pool.lock().await;
        let pool = guard.as_mut().expect("still alive");
        pool.in_flight.remove(&snapshot_start);
        if pool.next_to_enrich == snapshot_start {
            pool.enriched.extend(new_recs);
            pool.next_to_enrich += batch_len;
        }
    }

    // Filtro de dismissed sobre el pool cacheado. Se hace en cada
    // request para reflejar dismiss/undismiss al instante sin
    // invalidar la caché entera.
    let dismissed = dismissed::load();
    let dismissed_ids = dismissed.ids();
    let watched = watched::load();
    let watched_ids = watched.ids();
    let guard = state.recs_pool.lock().await;
    let pool = guard.as_ref().expect("just rebuilt or was fresh");
    let filtered: Vec<Recommendation> = pool
        .enriched
        .iter()
        .filter(|r| !dismissed_ids.contains(&r.movie.id) && !watched_ids.contains(&r.movie.id))
        .cloned()
        .collect();

    let end = (offset + limit).min(filtered.len());
    let items = filtered
        .get(offset..end)
        .map(|s| s.to_vec())
        .unwrap_or_default();
    // `has_more` refleja tanto items ya-enriched no servidos como
    // candidatos pendientes de enriquecer. Solo cuando hemos
    // agotado el pool devolvemos false.
    let has_more = end < filtered.len() || pool.next_to_enrich < pool.candidates.len();
    Ok(RecsPage {
        items,
        has_more,
        total: filtered.len(),
    })
}

#[derive(Serialize)]
struct RecsPage {
    items: Vec<Recommendation>,
    /// Si `true`, todavía hay más elementos disponibles para paginar
    /// con `offset += limit`. Cuando `has_more = false` el frontend
    /// deja de disparar fetches — hemos agotado el pool computado.
    has_more: bool,
    /// Tamaño total de recomendaciones disponibles tras filtro de
    /// dismissed. Útil para mostrar "N pelis disponibles" opcional.
    total: usize,
}

/// Marca una película como "no sugerir". El frontend la elimina de la
/// lista visible al instante (sin recargar); el servidor solo persiste
/// el `dismissed.json` para que las próximas páginas del scroll
/// infinito la filtren via `get_recommendations_page`.
#[tauri::command]
async fn dismiss_recommendation(
    tmdb_id: u64,
    title: String,
    poster_path: Option<String>,
) -> Result<(), String> {
    let mut store = dismissed::load();
    store.insert(DismissedEntry {
        id: tmdb_id,
        title,
        poster_path,
        dismissed_at: now_unix(),
    });
    dismissed::save(&store).map_err(|e| e.to_string())?;
    Ok(())
}

/// Restaura una película descartada (la borra del `dismissed.json`).
/// No refresca recomendaciones — el usuario lo hace desde Ajustes; en
/// la próxima carga de la vista Recs aparecerá si califica.
#[tauri::command]
async fn undismiss_recommendation(tmdb_id: u64) -> Result<(), String> {
    let mut store = dismissed::load();
    store.remove(tmdb_id);
    dismissed::save(&store).map_err(|e| e.to_string())?;
    Ok(())
}

/// Lista los descartes actuales, ordenados por más recientes primero.
/// Alimenta el panel "Restaurar sugerencias" en Ajustes.
#[tauri::command]
async fn list_dismissed() -> Result<Vec<DismissedEntry>, String> {
    let mut entries = dismissed::load().entries;
    entries.sort_by_key(|e| std::cmp::Reverse(e.dismissed_at));
    Ok(entries)
}

/// Vacía por completo el catálogo de descartes. Ajustes → "Restablecer
/// sugerencias descartadas".
#[tauri::command]
async fn clear_dismissed() -> Result<(), String> {
    dismissed::clear().map_err(|e| e.to_string())
}

/// Marca una película como "vista". Igual que dismiss, la elimina del
/// feed al instante y se persiste en `watched.json` para futuras
/// páginas del scroll. Se guarda por separado del store de dismisses
/// para que Ajustes pueda ofrecer dos catálogos y dos botones de
/// "Restablecer" independientes.
#[tauri::command]
async fn mark_watched(
    tmdb_id: u64,
    title: String,
    poster_path: Option<String>,
) -> Result<(), String> {
    let mut store = watched::load();
    store.insert(WatchedEntry {
        id: tmdb_id,
        title,
        poster_path,
        watched_at: now_unix(),
    });
    watched::save(&store).map_err(|e| e.to_string())?;
    Ok(())
}

/// Quita una peli del catálogo de "vistas". Simétrico a
/// `undismiss_recommendation`.
#[tauri::command]
async fn unmark_watched(tmdb_id: u64) -> Result<(), String> {
    let mut store = watched::load();
    store.remove(tmdb_id);
    watched::save(&store).map_err(|e| e.to_string())?;
    Ok(())
}

/// Lista las pelis marcadas como vistas, ordenadas por más recientes
/// primero. Alimenta el panel "Catálogo de vistas" en Ajustes.
#[tauri::command]
async fn list_watched() -> Result<Vec<WatchedEntry>, String> {
    let mut entries = watched::load().entries;
    entries.sort_by_key(|e| std::cmp::Reverse(e.watched_at));
    Ok(entries)
}

/// Vacía por completo el catálogo de vistas. Ajustes → "Restablecer
/// catálogo de vistas".
#[tauri::command]
async fn clear_watched() -> Result<(), String> {
    watched::clear().map_err(|e| e.to_string())
}

/// Detalle de una película para el modal estilo Stremio: sinopsis,
/// backdrop, runtime, géneros.
#[tauri::command]
async fn get_movie_view(
    tmdb_id: u64,
    state: State<'_, AppState>,
) -> Result<Option<MovieView>, String> {
    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer, current_ui_lang().as_deref());
    let start = std::time::Instant::now();
    let out = tmdb
        .get_movie_view(tmdb_id)
        .await
        .map_err(|e| e.to_string());
    tracing::info!(
        target: "gui",
        tmdb_id,
        elapsed_ms = start.elapsed().as_millis() as u64,
        "get_movie_view"
    );
    out
}

/// Detalle de una serie para la vista SeriesDetail: metadata general
/// (name, overview, poster, backdrop) + lista de temporadas + IMDb id
/// (necesario para los providers direccionables por id — EZTV,
/// Torznab tvsearch). §7 audit series.
#[tauri::command]
async fn get_series_view(
    tmdb_id: u64,
    state: State<'_, AppState>,
) -> Result<Option<tmdb::SeriesDetails>, String> {
    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer, current_ui_lang().as_deref());
    let start = std::time::Instant::now();
    let out = tmdb
        .get_series_details(tmdb_id)
        .await
        .map_err(|e| e.to_string());
    tracing::info!(
        target: "gui",
        tmdb_id,
        elapsed_ms = start.elapsed().as_millis() as u64,
        "get_series_view"
    );
    out
}

/// Episodios de una temporada. Se llama cuando el user selecciona un
/// tab de temporada en la vista SeriesDetail. Cacheado por TMDB
/// client con TTL largo (24 h) — las temporadas emitidas no cambian.
#[tauri::command]
async fn get_series_season(
    tmdb_id: u64,
    season: u16,
    state: State<'_, AppState>,
) -> Result<Vec<tmdb::SeriesEpisode>, String> {
    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer, current_ui_lang().as_deref());
    tmdb.get_season(tmdb_id, season)
        .await
        .map_err(|e| e.to_string())
}

/// Búsqueda TMDB por texto libre. Alimenta la pantalla intermedia de la
/// GUI: el user teclea "matrix" y ve posters de todas las coincidencias
/// antes de decidir de cuál quiere torrents. Evita el problema de "he
/// pedido una peli y me han salido resultados de otra distinta".
///
/// Cada hit de TMDB se cruza en paralelo con los providers de torrents
/// para poblar `torrent_count`. El sondeo aplica el MISMO filtro
/// mínimo que la vista detallada de Torrents (`min_seeders=3` +
/// `title_variants=[title]`) — así el número es un predictor fiable
/// de si al abrir la peli habrá torrents. El frontend usa este
/// campo para el toggle `hide_empty_results` (Ajustes): cuando está
/// activo, los hits con `torrent_count == 0` se ocultan de la lista.
/// Sin el toggle, se muestran igualmente y la vista de Torrents pinta
/// un mensaje adecuado al abrir.
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
            if now_unix().saturating_sub(cached.timestamp) < SEARCH_CACHE_TTL_SECS {
                return Ok(cached.hits.clone());
            }
        }
    }

    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer, current_ui_lang().as_deref());
    // §7 audit series: `search_multi` mezcla movie + tv en el mismo
    // request (single-shot) y respeta el ranking de popularidad de
    // TMDB. Cada hit trae `kind` para que el frontend pinte badges
    // Movie/Series. Sondeo de torrents (`torrent_count`) SOLO para
    // pelis — las series no tienen "torrent único" y el conteo
    // engañaría (siempre 0 sin S/E, no representativo).
    let movies = tmdb.search_multi(&query).await.map_err(|e| e.to_string())?;

    let providers = torrents::default_providers();
    let http = state.http.clone();

    // Sondeo ligero por película en paralelo (concurrencia 6 para no
    // saturar Knaben/YTS). Pedimos solo 5 resultados por película, lo
    // justo para saber si hay algo.
    //
    // **Query = `original_title` cuando existe** (v1.7.2): scene/P2P
    // siempre nombra las pelis en el idioma original. TMDB con
    // `locale=es-ES` devuelve "La pasión de China Blue" en `title`
    // — buscar ese string en apibay/knaben devuelve 0 hits. Scene
    // indexa "Crimes of Passion" (original_title). Sin este cambio,
    // toda peli no-inglesa se quedaba con `torrent_count = 0`
    // aunque tuviera decenas de releases con el título original.
    //
    // **title_variants = [orig, localized] dedup**: acepta releases
    // parseados con cualquiera de las dos formas. Si `original_title`
    // es None (peli inglesa donde ya coincide con `title`) queda como
    // `[title]`.
    //
    // **min_seeders=1**: la vista detallada usa 3, pero para el
    // conteo del catálogo aceptamos 1+ para no ocultar pelis
    // indies/oscuras cuyo swarm apenas respira. Sigue matando
    // el ruido de swarms muertos (=0 seeders). Trade-off consciente:
    // `torrent_count > 0` no garantiza al 100 % que la vista
    // detallada mostrará algo (por el bump a 3 seeders), pero
    // subestimar aquí ha probado ser peor UX (feed vacío para
    // catálogos indies).
    let checks = movies.into_iter().enumerate().map(|(idx, m)| {
        let providers = providers.clone();
        let http = http.clone();
        async move {
            // Series: skip el sondeo. Sin S/E no hay query útil y
            // los providers de series (EZTV, torznab tvsearch)
            // devolverían todos los episodios — el conteo no
            // significaría nada. Frontend pintará "Serie" sin count.
            if matches!(m.kind, crate::tmdb::MediaKind::Series) {
                return (idx, m, 0u32);
            }
            let localized = m.title.clone();
            let original = m
                .original_title
                .clone()
                .filter(|s| !s.is_empty() && s != &localized);
            let query_title = original.clone().unwrap_or_else(|| localized.clone());
            let mut variants: Vec<String> = Vec::with_capacity(2);
            if let Some(o) = original {
                variants.push(o);
            }
            if !variants.iter().any(|v| v == &localized) {
                variants.push(localized);
            }
            let q = MovieQuery {
                title: query_title,
                year: m.year(),
                imdb_id: m.imdb_id.clone(),
                tmdb_id: if m.id > 0 { Some(m.id) } else { None },
                original_language: None,
                title_variants: variants,
                kind: crate::tmdb::MediaKind::Movie,
                season: None,
                episode: None,
            };
            let list = torrents::search_all(&http, &providers, &q, 1, 5).await;
            (idx, m, list.results.len() as u32)
        }
    });

    // NO filtramos por torrent_count > 0: mostramos TODAS las pelis
    // que TMDB devolvió. Si el user hace click en una sin torrents
    // (torrent_count == 0), la vista de Torrents ya muestra un
    // mensaje adecuado con opciones. Mejor que desaparecer la peli
    // del catálogo silenciosamente.
    let mut results: Vec<(usize, TmdbMovie, u32)> = futures::stream::iter(checks)
        .buffer_unordered(6)
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
/// torrents (título original + IMDb + idioma + runtime) además de la lista.
#[derive(Clone, Serialize, Deserialize)]
struct TorrentSearchResult {
    title: String,
    imdb_id: Option<String>,
    original_language: Option<String>,
    year: Option<u16>,
    /// Duración en minutos según TMDB, usada para convertir la fracción
    /// del resume a segundos (`--start-time` de VLC). `None` cuando no
    /// venimos de TMDB o TMDB no la expone.
    runtime_minutes: Option<u32>,
    results: Vec<TorrentDto>,
    /// Estado por provider (Fase 1b — observabilidad). La UI pinta una
    /// línea tipo `knaben ✓ 34 · apibay ✗ timeout · yts ✓ 5` para que el
    /// user vea si la lista es corta por filtros o porque un provider
    /// se cayó. `[]` cuando no se ha lanzado ninguna búsqueda (p.ej.
    /// futura ruta 100% cacheada — Fase 4).
    #[serde(default)]
    providers: Vec<ProviderStatus>,
    /// Fecha de estreno TMDB (`YYYY-MM-DD`). Fase 4b: la UI la usa
    /// para el mensaje "todavía en cines" cuando `results` está vacío
    /// y la fecha es reciente o futura. `None` en búsquedas directas
    /// (sin TMDB) o si TMDB no la expone.
    #[serde(default)]
    release_date: Option<String>,
}

/// Torrent con el idioma de audio inferido (para la bandera en la UI).
/// Espejo de `Torrent` + `audio`.
#[derive(Clone, Serialize, Deserialize)]
struct TorrentDto {
    title: String,
    magnet: String,
    size_bytes: u64,
    seeders: u32,
    leechers: u32,
    quality: Option<String>,
    /// Nombre del provider (`"yts"`, `"knaben"`, ...). Antes era
    /// `&'static str` pero se cambió a `String` en Fase 4a del audit
    /// para poder round-tripear a través del caché en disco.
    source: String,
    /// Código ISO 639-1 del audio inferido (`"en"`, `"es"`, `"ru"`…) o
    /// marcador especial (`"multi"`, `"unknown"`, `"dub"`).
    audio: String,
    /// Cómo matchea contra la query (§7 audit): `"movie"`,
    /// `"episode"`, `"season_pack"`, `"series_pack"`. La UI pinta un
    /// badge acorde ("E03" / "Pack S01" / "Serie completa").
    match_kind: String,
    /// Índice de fichero pre-resuelto por el provider (Torrentio).
    /// El frontend lo pasa a `start_stream_html` como `file_hint`
    /// para saltarse la heurística de `select_file` en packs con
    /// numeración rara.
    #[serde(skip_serializing_if = "Option::is_none")]
    file_hint: Option<usize>,
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
        let match_kind = match t.match_kind {
            torrents::MatchKind::Movie => "movie",
            torrents::MatchKind::Episode => "episode",
            torrents::MatchKind::SeasonPack => "season_pack",
            torrents::MatchKind::SeriesPack => "series_pack",
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
            match_kind: match_kind.to_string(),
            file_hint: t.file_hint,
        }
    }
}

/// Búsqueda a partir de una película Letterboxd (recomendación con TMDB
/// id). Reproduce `spawn_torrents` de la TUI: resuelve detalles TMDB
/// (título original, IMDb, idioma) antes de consultar los providers.
///
/// Fase 3b — recall por variantes: en vez de lanzar UNA búsqueda por
/// título original y otra por inglés secuencialmente, construimos un
/// conjunto de hasta 3 variantes ([original, inglés, mejor alt de
/// TMDB]) deduplicadas por forma normalizada, y lanzamos las 3
/// `search_all` EN PARALELO. Los resultados se mergen por infohash.
/// Cada `MovieQuery` lleva ADEMÁS el conjunto completo de variantes
/// como `title_variants` para que el filtro central de `search_all`
/// acepte releases que matcheen CUALQUIERA de ellas.
#[tauri::command]
async fn search_torrents_by_tmdb(
    tmdb_id: u64,
    fallback_title: String,
    fallback_year: Option<u16>,
    state: State<'_, AppState>,
) -> Result<TorrentSearchResult, String> {
    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer, current_ui_lang().as_deref());
    let details = tmdb.get_movie_details(tmdb_id).await.ok().flatten();

    // Fase 4a — cache check ANTES del sondeo a providers. Key
    // preferente: imdb_id (canónico). Si el details lookup falló y
    // no tenemos imdb_id, caemos a `direct:norm(fallback_title):year`.
    let imdb_key = details.as_ref().and_then(|d| d.imdb_id.clone());
    let year_key = details.as_ref().and_then(|d| d.year).or(fallback_year);
    let cache_key = torrent_cache_key(imdb_key.as_deref(), &fallback_title, year_key);
    if let Some(hit) = torrent_cache_get_fresh(&*state.torrent_cache.lock().await, &cache_key) {
        return Ok(hit);
    }
    let (
        title,
        english_title,
        russian_title,
        year,
        imdb_id,
        original_language,
        runtime,
        alt_titles,
        release_date,
    ) = match details {
        Some(d) => {
            // `title` = original (italiano para Salò, coreano para Parasite…).
            // `english_title` = title en inglés de TMDB (viene en `fallback_title`
            // porque pedimos `/movie/{id}?language=en-US`). Los indexadores
            // (YTS/PirateBay/Knaben) INDEXAN por título inglés casi siempre —
            // los releases se llaman `Salo.or.the.120.Days.of.Sodom.1975.*`,
            // no `Salò.o.le.120.giornate.di.Sodoma.*`. Sin buscar por el
            // inglés, títulos no-inglés devuelven cero o casi cero torrents.
            let orig = d
                .original_title
                .clone()
                .unwrap_or_else(|| fallback_title.clone());
            let eng = d.fallback_title.filter(|s| !s.is_empty() && s != &orig);
            (
                orig,
                eng,
                d.russian_title,
                d.year.or(fallback_year),
                d.imdb_id,
                d.original_language,
                d.runtime,
                d.alt_titles,
                d.release_date,
            )
        }
        None => (
            fallback_title.clone(),
            None,
            None,
            fallback_year,
            None,
            None,
            None,
            Vec::new(),
            None,
        ),
    };

    let providers = torrents::default_providers();

    // Construimos el conjunto de variantes de búsqueda. El orden es
    // importante: original primero (idioma nativo → indexers
    // regionales), inglés segundo (scene global), alt-titles el
    // resto. Deduplicamos por forma normalizada — dos variantes que
    // colapsan a la misma tras `normalize_title` no aportan hits
    // nuevos. Cap a 3 (límite del audit: ≤3 por provider para no
    // multiplicar latencia).
    const MAX_VARIANTS: usize = 3;
    let mut variants: Vec<String> = Vec::new();
    let mut seen_norm: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push_variant = |v: String, variants: &mut Vec<String>| {
        let norm = torrents::release_name::normalize_title(&v);
        if norm.is_empty() {
            return;
        }
        if seen_norm.insert(norm) && variants.len() < MAX_VARIANTS {
            variants.push(v);
        }
    };
    push_variant(title.clone(), &mut variants);
    if let Some(eng) = english_title.as_ref() {
        push_variant(eng.clone(), &mut variants);
    }
    for alt in &alt_titles {
        push_variant(alt.clone(), &mut variants);
    }

    // El `title_variants` que va DENTRO de cada `MovieQuery` es el
    // conjunto completo — así el filtro central de `search_all` acepta
    // cualquier release que matchee UNA de las variantes,
    // independientemente de con qué `title` se lanzó la búsqueda.
    let filter_variants = variants.clone();

    // Lanzamos las búsquedas en paralelo (una por variante). Cada
    // `search_all` interno ya paraleliza sobre providers; aquí
    // multiplicamos por N variantes. Con 3 variantes × 4 providers
    // salen 12 requests HTTP en flight — dentro de rangos sanos.
    let variant_futures = variants.iter().cloned().map(|v| {
        let q = MovieQuery {
            title: v,
            year,
            imdb_id: imdb_id.clone(),
            tmdb_id: Some(tmdb_id),
            original_language: original_language.clone(),
            title_variants: filter_variants.clone(),
            kind: crate::tmdb::MediaKind::Movie,
            season: None,
            episode: None,
        };
        let http = state.http.clone();
        let providers = providers.clone();
        async move { torrents::search_all(&http, &providers, &q, 3, 40).await }
    });
    let outcomes: Vec<torrents::SearchOutcome> = futures::future::join_all(variant_futures).await;

    // Merge por infohash + consolidación de providers status.
    use std::collections::HashMap;
    let mut merged: HashMap<String, torrents::Torrent> = HashMap::new();
    let mut providers_status: Vec<ProviderStatus> = Vec::new();
    for o in outcomes {
        providers_status = merge_provider_statuses(providers_status, o.providers);
        for t in o.results {
            match merged.get_mut(&t.infohash) {
                Some(prev) if prev.seeders < t.seeders => *prev = t,
                Some(_) => {}
                None => {
                    merged.insert(t.infohash.clone(), t);
                }
            }
        }
    }
    let mut list: Vec<torrents::Torrent> = merged.into_values().collect();
    list.sort_by_key(|t| std::cmp::Reverse(t.seeders));
    list.truncate(40);

    // Fallback ruso, como en la TUI. Solo si la lista sigue vacía
    // tras las variantes principales. NOTA: no pasamos
    // `title_variants` aquí — el título ruso en cirílico no matchea
    // ninguna variante latina; usamos `release_starts_with` como
    // filtro post-hoc (patrón `<Nombre ruso> / <Nombre original>`).
    if list.is_empty() {
        if let Some(ru) = russian_title.filter(|s| s != &title) {
            let ru_q = MovieQuery {
                title: ru.clone(),
                year,
                imdb_id: imdb_id.clone(),
                tmdb_id: Some(tmdb_id),
                original_language: original_language.clone(),
                title_variants: Vec::new(),
                kind: crate::tmdb::MediaKind::Movie,
                season: None,
                episode: None,
            };
            let ru_outcome = torrents::search_all(&state.http, &providers, &ru_q, 3, 40).await;
            providers_status = merge_provider_statuses(providers_status, ru_outcome.providers);
            list = ru_outcome
                .results
                .into_iter()
                .filter(|t| release_starts_with(&t.title, &ru))
                .collect();
        }
    }

    let result = TorrentSearchResult {
        title,
        imdb_id,
        original_language: original_language.clone(),
        year,
        runtime_minutes: runtime,
        results: list
            .into_iter()
            .map(|t| TorrentDto::from_torrent(t, original_language.as_deref()))
            .collect(),
        providers: providers_status,
        release_date,
    };

    // Fase 4a: persistimos el resultado. El TTL efectivo lo decide
    // `torrent_cache_ttl` según si `results` está vacío (5 min) o
    // no (30 min).
    torrent_cache_put(&mut *state.torrent_cache.lock().await, cache_key, &result);
    Ok(result)
}

/// Búsqueda de torrents de un episodio (o pack de temporada, si
/// `episode = None`) de una serie. §7 audit series.
///
/// Flujo:
///   1. TMDB `/tv/{id}` para obtener nombre original + IMDb id + alt
///      titles + idioma original — el imdb_id es CLAVE para EZTV y
///      Torznab tvsearch.
///   2. Construimos hasta 3 variantes de título (original, inglés,
///      mejor alt) igual que la búsqueda de películas.
///   3. Lanzamos `search_all` con `kind=Series, season, episode` en
///      paralelo por variante y mergemos por infohash.
///
/// Se cachea con clave `imdb:sSSeEE` (o `direct:norm:year:sSS` si no
/// hay imdb). El TTL efectivo lo decide `torrent_cache_ttl`.
#[tauri::command]
async fn search_torrents_series(
    tmdb_id: u64,
    season: u16,
    episode: Option<u16>,
    state: State<'_, AppState>,
) -> Result<TorrentSearchResult, String> {
    let bearer = state.config.lock().await.tmdb_bearer_token.clone();
    let tmdb = TmdbClient::new(&state.http, &bearer, current_ui_lang().as_deref());
    let details = tmdb
        .get_series_details(tmdb_id)
        .await
        .map_err(|e| e.to_string())?;
    let details = details.ok_or_else(|| format!("Serie tmdb_id={tmdb_id} no encontrada"))?;

    let imdb_key = details.imdb_id.clone();
    let year: Option<u16> = details
        .first_air_date
        .as_deref()
        .and_then(|s| s.get(0..4).and_then(|y| y.parse::<u16>().ok()));
    let cache_key = torrent_cache_key_with_ep(
        imdb_key.as_deref(),
        &details.name,
        year,
        Some(season),
        episode,
    );
    if let Some(hit) = torrent_cache_get_fresh(&*state.torrent_cache.lock().await, &cache_key) {
        return Ok(hit);
    }

    // Título canónico para la UI. Preferimos el nombre en el idioma
    // original (evita "Cazadores de sombras" cuando el user busca
    // "Shadowhunters") — el name localizado se usa para display.
    let orig = details
        .original_name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| details.name.clone());
    let english = if orig != details.name {
        Some(details.name.clone())
    } else {
        None
    };
    let original_language = details.original_language.clone();
    let imdb_id = details.imdb_id.clone();
    let release_date = details.first_air_date.clone();

    // Variantes: original + inglés + alt_titles (Fase alt-titles-series
    // — antes solo teníamos orig+english y las series CJK/cirílicas
    // se caían enteras en el filtro de título de search_all).
    const MAX_VARIANTS: usize = 3;
    let mut variants: Vec<String> = Vec::new();
    let mut seen_norm: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push_variant = |v: String, variants: &mut Vec<String>| {
        let norm = torrents::release_name::normalize_title(&v);
        if norm.is_empty() {
            return;
        }
        if seen_norm.insert(norm) && variants.len() < MAX_VARIANTS {
            variants.push(v);
        }
    };
    push_variant(orig.clone(), &mut variants);
    if let Some(eng) = english.as_ref() {
        push_variant(eng.clone(), &mut variants);
    }
    for alt in &details.alt_titles {
        push_variant(alt.clone(), &mut variants);
    }

    let filter_variants = variants.clone();
    let providers = torrents::default_providers();

    let variant_futures = variants.iter().cloned().map(|v| {
        let q = MovieQuery {
            title: v,
            year,
            imdb_id: imdb_id.clone(),
            tmdb_id: Some(tmdb_id),
            original_language: original_language.clone(),
            title_variants: filter_variants.clone(),
            kind: crate::tmdb::MediaKind::Series,
            season: Some(season),
            episode,
        };
        let http = state.http.clone();
        let providers = providers.clone();
        async move { torrents::search_all(&http, &providers, &q, 3, 40).await }
    });
    let outcomes: Vec<torrents::SearchOutcome> = futures::future::join_all(variant_futures).await;

    use std::collections::HashMap;
    let mut merged: HashMap<String, torrents::Torrent> = HashMap::new();
    let mut providers_status: Vec<ProviderStatus> = Vec::new();
    for o in outcomes {
        providers_status = merge_provider_statuses(providers_status, o.providers);
        for t in o.results {
            match merged.get_mut(&t.infohash) {
                Some(prev) if prev.seeders < t.seeders => *prev = t,
                Some(_) => {}
                None => {
                    merged.insert(t.infohash.clone(), t);
                }
            }
        }
    }
    let mut list: Vec<torrents::Torrent> = merged.into_values().collect();
    list.sort_by_key(|t| std::cmp::Reverse(t.seeders));
    list.truncate(40);

    let result = TorrentSearchResult {
        title: orig,
        imdb_id,
        original_language: original_language.clone(),
        year,
        // Runtime medio de un episodio (min): no lo pedimos aún
        // desde /tv/{id} — se puede sacar de get_season si hace
        // falta. Dejamos None; el player HTML no depende de él
        // (usa duration_seconds de ffprobe).
        runtime_minutes: None,
        results: list
            .into_iter()
            .map(|t| TorrentDto::from_torrent(t, original_language.as_deref()))
            .collect(),
        providers: providers_status,
        release_date,
    };

    torrent_cache_put(&mut *state.torrent_cache.lock().await, cache_key, &result);
    Ok(result)
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

    // Fase 4a — cache check.
    let cache_key = torrent_cache_key(None, &title, year);
    if let Some(hit) = torrent_cache_get_fresh(&*state.torrent_cache.lock().await, &cache_key) {
        return Ok(hit);
    }

    let providers = torrents::default_providers();
    let q = MovieQuery {
        title: title.clone(),
        year,
        imdb_id: None,
        tmdb_id: None,
        original_language: None,
        title_variants: Vec::new(),
        kind: crate::tmdb::MediaKind::Movie,
        season: None,
        episode: None,
    };
    let list = torrents::search_all(&state.http, &providers, &q, 3, 40).await;
    let result = TorrentSearchResult {
        title,
        imdb_id: None,
        original_language: None,
        year,
        runtime_minutes: None,
        results: list
            .results
            .into_iter()
            .map(|t| TorrentDto::from_torrent(t, None))
            .collect(),
        providers: list.providers,
        release_date: None,
    };
    torrent_cache_put(&mut *state.torrent_cache.lock().await, cache_key, &result);
    Ok(result)
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
    start_stream_inner(magnet, None, None, None, PlayerMode::Vlc, &state).await
}

/// Como `start_stream`, pero pasando explícitamente un path de subtítulo
/// para VLC y — opcionalmente — la posición inicial en segundos para
/// reanudar desde donde el user lo dejó. Se usa desde el flujo de
/// diálogo pre-stream cuando la preferencia es VLC.
#[tauri::command]
async fn start_stream_with_sub(
    magnet: String,
    sub_path: Option<String>,
    resume_seconds: Option<u32>,
    season: Option<u16>,
    episode: Option<u16>,
    file_hint: Option<usize>,
    state: State<'_, AppState>,
) -> Result<StreamInfo, String> {
    let target = build_target(season, episode, file_hint);
    start_stream_inner(
        magnet,
        sub_path.map(PathBuf::from),
        resume_seconds,
        target,
        PlayerMode::Vlc,
        &state,
    )
    .await
}

/// Arranca el stream en modo player HTML: no spawnea VLC, solo librqbit +
/// el servidor HTTP local. La URL devuelta se usa como `<video src>`
/// (con `/play.mp4` como path para pasar por ffmpeg / transmux). Los
/// subtítulos los descarga el frontend por separado vía endpoints
/// dedicados.
///
/// `season`/`episode`: cuando el magnet es un season pack de una
/// serie, seleccionan el fichero del episodio pedido dentro del
/// torrent parseando nombres (§4 audit series). Ambos juntos o ninguno.
/// `file_hint`: cuando el provider ya resolvió el índice del fichero
/// (Torrentio.fileIdx), se pasa aquí y skipeamos el parseo. Tiene
/// prioridad sobre season/episode.
#[tauri::command]
async fn start_stream_html(
    magnet: String,
    season: Option<u16>,
    episode: Option<u16>,
    file_hint: Option<usize>,
    state: State<'_, AppState>,
) -> Result<StreamInfo, String> {
    let target = build_target(season, episode, file_hint);
    start_stream_inner(magnet, None, None, target, PlayerMode::Html, &state).await
}

/// Construye el `FileSelector` a partir de los inputs del frontend.
/// `file_hint` (índice pre-resuelto por Torrentio) gana a
/// `(season, episode)` porque es más preciso — el provider ya sabe
/// qué fichero es y no depende del parser de nombres.
fn build_target(
    season: Option<u16>,
    episode: Option<u16>,
    file_hint: Option<usize>,
) -> Option<torrents::FileSelector> {
    if let Some(idx) = file_hint {
        return Some(torrents::FileSelector::Index(idx));
    }
    season
        .zip(episode)
        .map(|(s, e)| torrents::FileSelector::Episode(s, e))
}

/// Lista los ficheros de un torrent multi-file sin arrancar streaming.
/// Devuelve nombre, tamaño y S/E parseados por fichero — para que la
/// UI ofrezca selección manual cuando la heurística de S+E no dé con
/// el episodio (packs con numeración absoluta de anime, encodings raros).
#[tauri::command]
async fn list_torrent_files(magnet: String) -> Result<Vec<stream::TorrentFileInfo>, String> {
    stream::list_files(magnet).await.map_err(|e| e.to_string())
}

/// Fuente de un torrent dropeado sobre la ventana. Puede ser un magnet
/// URI (drag de texto desde el navegador) o un fichero `.torrent`
/// local (drag desde el file manager — Tauri `onDragDropEvent` da el
/// path absoluto en `payload.paths`).
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DroppedTorrentSource {
    Magnet { uri: String },
    File { path: String },
}

/// Resultado de resolver un torrent dropeado. `magnet` es la URI
/// canónica (para .torrent files se construye a partir del sha1 del
/// info-dict + trackers de announce/announce-list), reutilizable
/// tal cual por `start_stream_html`. `name` es el nombre display
/// (para el header de la vista). `files` es la lista de ficheros
/// del torrent, ya con `is_video` marcado — misma forma que
/// `list_torrent_files` para que el frontend pueda reusar la UI.
#[derive(serde::Serialize)]
struct ResolvedDroppedTorrent {
    magnet: String,
    name: String,
    info_hash: Option<String>,
    files: Vec<stream::TorrentFileInfo>,
}

/// Resuelve un torrent dropeado (magnet URI o fichero `.torrent`) y
/// devuelve el magnet canónico + lista de ficheros. El frontend usa
/// esta respuesta para pintar una vista tipo "Torrents" con click
/// directo a Reproducir sin pasar por TMDB / providers.
#[tauri::command]
async fn resolve_dropped_torrent(
    source: DroppedTorrentSource,
) -> Result<ResolvedDroppedTorrent, String> {
    let (magnet, name) = match source {
        DroppedTorrentSource::Magnet { uri } => {
            let uri = uri.trim().to_string();
            if !uri.starts_with("magnet:") {
                return Err("El texto dropeado no es un magnet:? válido".to_string());
            }
            // display name = dn= param si existe, si no el infohash.
            let name = extract_dn_from_magnet(&uri)
                .or_else(|| stream::parse_infohash(&uri))
                .unwrap_or_else(|| "magnet".to_string());
            (uri, name)
        }
        DroppedTorrentSource::File { path } => {
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|e| format!("No se pudo leer {path}: {e}"))?;
            stream::torrent_bytes_to_magnet(&bytes).map_err(|e| format!("{e:#}"))?
        }
    };
    let info_hash = stream::parse_infohash(&magnet);
    let files = stream::list_files(magnet.clone())
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(ResolvedDroppedTorrent {
        magnet,
        name,
        info_hash,
        files,
    })
}

/// Extrae el parámetro `dn=` (display name) de un magnet URI.
/// Devuelve `None` si no está o falla el URL-decode. Es tolerante
/// con parámetros repetidos, orden arbitrario y magnets sin `?`.
fn extract_dn_from_magnet(uri: &str) -> Option<String> {
    let query = uri.split_once('?').map(|(_, q)| q)?;
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("dn=") {
            let decoded = urlencoding::decode(v).ok()?.into_owned();
            let trimmed = decoded.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[derive(Copy, Clone)]
enum PlayerMode {
    /// Spawnea VLC como proceso externo con `--sub-file` y `--start-time`.
    Vlc,
    /// Solo librqbit + HTTP server. El player vive en la webview.
    Html,
}

async fn start_stream_inner(
    magnet: String,
    sub_path: Option<PathBuf>,
    resume_seconds: Option<u32>,
    target: Option<torrents::FileSelector>,
    mode: PlayerMode,
    state: &State<'_, AppState>,
) -> Result<StreamInfo, String> {
    let handle = stream::start_with_target(magnet, target)
        .await
        .map_err(|e| e.to_string())?;

    let mut id_lock = state.next_stream_id.lock().await;
    *id_lock += 1;
    let id = *id_lock;
    drop(id_lock);

    let info = StreamInfo {
        id,
        url: handle.url.clone(),
        file_name: handle.file_name.clone(),
    };

    let player = match mode {
        PlayerMode::Vlc => Some(stream::open_in_vlc(
            &handle.url,
            sub_path.as_deref(),
            resume_seconds,
        )),
        PlayerMode::Html => None,
    };

    state
        .streams
        .lock()
        .await
        .insert(id, ActiveStream { handle, player });
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
    let mut streams = state.streams.lock().await;
    let active = streams
        .get(&id)
        .ok_or_else(|| format!("stream {id} no encontrado"))?;
    // En modo VLC (player = Some) `alive` refleja si el proceso VLC
    // sigue vivo. En modo HTML (player = None) el stream vive hasta
    // que el frontend llame explícitamente a `stop_stream`, así que
    // `alive` es `true` mientras el slot exista.
    let alive = active
        .player
        .as_ref()
        .map(|p| p.alive.load(Ordering::Relaxed))
        .unwrap_or(true);
    let StreamStats {
        progress_bytes,
        total_bytes,
        live_peers,
        down_mbps,
    } = active.handle.stats();

    // Si VLC murió, limpiamos el slot: así el frontend recibe un solo
    // stats con `alive=false` y después deja de pollear. El Drop del
    // handle apaga librqbit + libera el tempdir. En modo HTML no
    // aplica: solo `stop_stream` explícito puede quitar el slot.
    if !alive {
        streams.remove(&id);
    }

    Ok(StreamStatsDto {
        progress_bytes,
        total_bytes,
        live_peers,
        down_mbps,
        alive,
    })
}

#[tauri::command]
async fn stop_stream(id: u64, state: State<'_, AppState>) -> Result<(), String> {
    // Pulsar "Detener" en la UI SIEMPRE cierra VLC: sin esto quedaba
    // VLC vivo (macOS lo lanza vía LaunchServices, no como hijo
    // directo) y el user tenía que ir a cerrarlo a mano. `kill()`
    // dispara el `CancellationToken` que la tarea de espera del
    // PlayerHandle usa para invocar el quit nativo por SO. En modo
    // HTML no hay player externo → basta con quitar el slot; el Drop
    // del `StreamHandle` cierra la sesión BitTorrent.
    if let Some(active) = state.streams.lock().await.remove(&id) {
        if let Some(player) = active.player.as_ref() {
            player.kill();
        }
    }
    Ok(())
}

/// Devuelve la posición de resume guardada para un magnet, si la caché
/// tiene una entrada para su infohash. Puede venir en dos formas:
///
///   * `seconds` + `duration_seconds` — reportado por el player HTML
///     con `report_position`. Preferido: viene del `<video>.currentTime`
///     (exacto) y funciona sin runtime de TMDB → habilita resume en
///     modo direct y en búsquedas directas.
///   * `fraction` — byte-based, escrito por el Drop de `StreamHandle`
///     (fallback VLC y compatibilidad con caché legacy).
///
/// El frontend prefiere `seconds` cuando existe; si solo hay
/// `fraction`, multiplica por `runtime_minutes` de TMDB.
#[derive(Serialize)]
struct ResumeDto {
    fraction: f32,
    seconds: Option<f64>,
    duration_seconds: Option<f64>,
    updated_at: u64,
    season: Option<u16>,
    episode: Option<u16>,
    /// Sub que estaba activo la última vez que el user salió. `None`
    /// para entradas viejas (per-infohash resume) o cuando el user
    /// nunca activó subs. El frontend hidrata su `activeSub` con
    /// esto al montar el Player — feature "los subs se mantienen
    /// entre sesiones".
    last_sub: Option<stream::LastSub>,
}

#[tauri::command]
async fn get_resume(
    magnet: String,
    season: Option<u16>,
    episode: Option<u16>,
    tmdb_id: Option<u64>,
) -> Result<Option<ResumeDto>, String> {
    // Prioridad 1: store de PROGRESO POR PELÍCULA (`movie_progress.json`).
    // Es lo que el user espera cuando ha visto la peli con OTRO
    // torrent y ahora abre uno nuevo — mismo tmdb_id, misma posición.
    // Solo aplicable cuando el caller conoce el tmdb_id (flujo Recs,
    // Search TMDB, Series). En modo directo (búsqueda libre por
    // magnet sin TMDB match) caemos al per-infohash sin este atajo.
    if let Some(id) = tmdb_id {
        if let Some(mp) = stream::load_movie_progress(id, season, episode) {
            // Sanea `last_sub`: si es `openSubs` y la ruta ya no
            // existe en disco (macOS purga el TMPDIR entre sesiones,
            // o el user vien\u00eda de una versi\u00f3n que guardaba subs en
            // `$TMPDIR/videodrome-subs/` \u2014 ese dir se borra en el
            // startup wipe legacy), lo tratamos como \"sin sub\". Sin
            // esto el frontend hidrata `activeSub` con un path
            // muerto, `subtitle_to_vtt` falla, `<track>` no se
            // monta pero el panel sigue mostrando la pista como
            // activa \u2014 el bug real que reportaba el user.
            let last_sub = match mp.last_sub {
                Some(stream::LastSub::OpenSubs { ref path, .. })
                    if !std::path::Path::new(path).exists() =>
                {
                    tracing::info!(
                        target: "resume",
                        path = %path,
                        "descartando last_sub openSubs con ruta muerta"
                    );
                    None
                }
                other => other,
            };
            return Ok(Some(ResumeDto {
                fraction: 0.0,
                seconds: Some(mp.seconds),
                duration_seconds: if mp.duration_seconds > 0.0 {
                    Some(mp.duration_seconds)
                } else {
                    None
                },
                updated_at: mp.updated_at,
                season: mp.season,
                episode: mp.episode,
                last_sub,
            }));
        }
    }
    let Some(hash) = stream::parse_infohash(&magnet) else {
        return Ok(None);
    };
    // Fallback: resume per-infohash. Antes de start_stream no
    // conocemos el file_id → `load_resume_any`. Cuando el user
    // viene del flujo de serie pasa (season, episode) → filtra a la
    // entrada exacta y no devuelve el resume de otro episodio del
    // mismo pack.
    let target = season.zip(episode);
    Ok(stream::load_resume_any(&hash, target).map(|r| ResumeDto {
        fraction: r.fraction,
        seconds: r.seconds,
        duration_seconds: r.duration_seconds,
        updated_at: r.updated_at,
        season: r.episode.as_ref().map(|e| e.season),
        episode: r.episode.as_ref().map(|e| e.episode),
        last_sub: None,
    }))
}

/// El player HTML llama a este comando cada ~15s (y en el cleanup, y
/// al pulsar Volver) con la posición absoluta del `<video>` y la
/// duración detectada por ffprobe. Backend persiste ambos valores en
/// `resume.json` (merge-style, sin pisar `fraction`); si la posición
/// supera el 95% del runtime, borra el resume (peli terminada).
///
/// `season`/`episode`/`tmdb_id` (opcionales) se guardan como
/// metadata de episodio en la entrada del store — habilita
/// "continuar viendo" y "siguiente episodio" (§6 audit).
///
/// Además, cuando `tmdb_id` está presente, escribe TAMBIÉN al store
/// global `movie_progress.json` — la fuente de verdad para "seguir
/// viendo" y para que la posición se comparta entre distintos
/// torrents de la misma peli/episodio (el user reportaba que cada
/// release arrancaba el marker de cero).
///
/// Errores no bloquean al player: si el `stream_id` ya no está vivo
/// (stopStream previo, race con navigate away), devolvemos Ok sin más.
/// Si el magnet no tiene infohash reconocible (caché en tempdir, sin
/// persistencia), tampoco es error — simplemente no hay dónde escribir.
/// Payload aceptado por `report_position` para actualizar el sub
/// activo. Discriminated union por `source`: `openSubs`/`embedded`
/// son los estados normales, `none` significa "el user desactivó
/// los subs explícitamente" → borra el campo. `null` en el JSON
/// (Option outer ausente) significa "no toques el campo".
#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "camelCase")]
enum LastSubPayload {
    #[serde(rename_all = "camelCase")]
    OpenSubs {
        path: String,
        release: String,
        language: String,
    },
    #[serde(rename_all = "camelCase")]
    Embedded {
        idx: usize,
        release: String,
        language: String,
    },
    /// `{"source":"none"}` explícito → clear del campo.
    None,
}

impl LastSubPayload {
    fn into_update(self) -> stream::LastSubUpdate {
        match self {
            Self::OpenSubs {
                path,
                release,
                language,
            } => stream::LastSubUpdate::Set(stream::LastSub::OpenSubs {
                path,
                release,
                language,
            }),
            Self::Embedded {
                idx,
                release,
                language,
            } => stream::LastSubUpdate::Set(stream::LastSub::Embedded {
                idx,
                release,
                language,
            }),
            Self::None => stream::LastSubUpdate::Clear,
        }
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn report_position(
    stream_id: u64,
    seconds: f64,
    duration_seconds: f64,
    season: Option<u16>,
    episode: Option<u16>,
    tmdb_id: Option<u64>,
    title: Option<String>,
    imdb_id: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    year: Option<u16>,
    magnet: Option<String>,
    // Snapshot de la pista de subs activa. `Some(...)` para
    // actualizar; `None` significa "no toques el campo" (permite
    // que el timer periódico no tenga que re-enviar el sub en cada
    // ping). El frontend manda `Some({source:'none'})` como señal
    // explícita de "clear" — deserializamos como `Some(Clear)`.
    last_sub: Option<LastSubPayload>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Sanea entradas no finitas: HLS puede reportar `duration = Infinity`
    // en transiciones raras, y un `NaN` cualquiera reventaría el
    // `serde_json::to_string` dentro de `save_position` (JSON no
    // representa NaN/Infinity), acabando con una escritura silenciosa
    // fallida y el resume perdido. Descartamos el update en su lugar.
    if !seconds.is_finite() || !duration_seconds.is_finite() {
        return Ok(());
    }
    let handle_info = {
        let streams = state.streams.lock().await;
        streams
            .get(&stream_id)
            .map(|s| (s.handle.infohash.clone(), s.handle.file_id))
    };
    if let Some((Some(hash), file_id)) = handle_info.as_ref() {
        let ep = season.zip(episode).map(|(s, e)| stream::ResumeEpisode {
            season: s,
            episode: e,
            tmdb_id,
        });
        stream::save_position(hash, *file_id, seconds, duration_seconds, ep);
    }
    // Store movie-level: solo escribe cuando tenemos tmdb_id (sin
    // él la key sería ambigua, no habría forma de matchear un futuro
    // getResume/list_watch_progress con esta entrada).
    if let Some(id) = tmdb_id {
        let meta = stream::MovieProgressMeta {
            kind: if season.is_some() {
                "series".into()
            } else {
                "movie".into()
            },
            title: title.unwrap_or_default(),
            poster_path,
            backdrop_path,
            imdb_id,
            year,
            last_magnet: magnet,
            last_sub: last_sub.map(LastSubPayload::into_update),
        };
        stream::save_movie_progress(id, season, episode, seconds, duration_seconds, meta);
    }
    Ok(())
}

/// DTO expuesto al frontend con progreso de una peli/episodio en
/// curso. Se pinta en la sección "Seguir viendo" de Home. Los
/// campos son un mirror de `stream::MovieProgress` pero con serde
/// camelCase amigable al TS (Tauri ya hace snake→camel por defecto
/// en los args del comando; los tipos de retorno se sirven tal cual
/// están serializados por serde).
#[derive(Serialize)]
struct WatchProgressDto {
    tmdb_id: u64,
    kind: String,
    season: Option<u16>,
    episode: Option<u16>,
    title: String,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    imdb_id: Option<String>,
    year: Option<u16>,
    last_magnet: Option<String>,
    seconds: f64,
    duration_seconds: f64,
    updated_at: u64,
}

impl From<stream::MovieProgress> for WatchProgressDto {
    fn from(m: stream::MovieProgress) -> Self {
        Self {
            tmdb_id: m.tmdb_id,
            kind: m.kind,
            season: m.season,
            episode: m.episode,
            title: m.title,
            poster_path: m.poster_path,
            backdrop_path: m.backdrop_path,
            imdb_id: m.imdb_id,
            year: m.year,
            last_magnet: m.last_magnet,
            seconds: m.seconds,
            duration_seconds: m.duration_seconds,
            updated_at: m.updated_at,
        }
    }
}

/// Lista el catálogo "seguir viendo" — pelis y episodios que el
/// user dejó a mitad. Ordenado por `updated_at` DESC. Sirve para la
/// sección homónima de Home. Filtrado (`> 2%` y `< 95%`) hecho ya en
/// el módulo `stream::movie_progress`.
#[tauri::command]
async fn list_watch_progress() -> Result<Vec<WatchProgressDto>, String> {
    Ok(stream::list_movie_progress()
        .into_iter()
        .map(WatchProgressDto::from)
        .collect())
}

/// Borra una entrada del catálogo "seguir viendo". Ligado al icono
/// "quitar" que aparece al hover de cada card en Home. No devuelve
/// error si no existe (idempotente).
#[tauri::command]
async fn remove_watch_progress(
    tmdb_id: u64,
    season: Option<u16>,
    episode: Option<u16>,
) -> Result<(), String> {
    stream::remove_movie_progress(tmdb_id, season, episode);
    Ok(())
}

// ---------- Subtítulos ----------

#[tauri::command]
async fn subtitles_available() -> Result<bool, String> {
    Ok(subtitles::is_available())
}

/// Reporta si ffmpeg + ffprobe están en PATH. El frontend lo usa al
/// arrancar para decidir si el toggle "Reproductor HTML" en Preferences
/// puede activarse y para mostrar un aviso si el user tiene la
/// preferencia en `Html` pero no tiene ffmpeg instalado — en ese caso
/// cae a VLC automáticamente y le enseña las instrucciones de install.
#[tauri::command]
async fn ffmpeg_available() -> Result<bool, String> {
    Ok(crate::ffmpeg::is_available())
}

/// Registra las capacidades del cliente (audit §4). El frontend
/// llama esto al mount de `App.tsx` con lo que `canPlayType()`
/// reporta para cada MIME representativo (h264 / hevc / hevc10 /
/// av1 / aac / mp3 / ac3 / eac3 / opus / flac). El backend usa las
/// caps para decidir DIRECT vs COPY vs TRANSCODE en `spawn_hls` y
/// `compute_direct_playable`. Idempotente — llamar dos veces
/// sobreescribe con la última.
#[tauri::command]
async fn set_client_capabilities(caps: crate::ffmpeg::ClientCapabilities) -> Result<(), String> {
    stream::set_client_capabilities(caps);
    Ok(())
}

#[tauri::command]
async fn search_subtitles(
    stream_id: Option<u64>,
    imdb_id: Option<String>,
    query: Option<String>,
    season: Option<u16>,
    episode: Option<u16>,
    languages: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Subtitle>, String> {
    let langs = languages.unwrap_or_default();

    // Intento #1: hash del fichero de vídeo. Es el criterio más preciso
    // — OpenSubtitles indexa subs por hash de fichero y devuelve solo
    // los que fueron sync-verified con ESE release exacto (no importa
    // si el imdb_id que envió el frontend está mal por mismatch en el
    // catálogo TMDB: el hash desambigua al contenido real).
    //
    // Si el fichero no está descargado lo suficiente (< 128 KB por
    // ambos extremos), `compute_moviehash` devuelve None y caemos al
    // path clásico.
    let moviehash = if let Some(id) = stream_id {
        // Extraemos las 3 piezas necesarias para el hash mientras
        // tenemos el lock del map de streams; luego lo soltamos y
        // computamos el hash sin bloquear el map (compute_moviehash
        // puede tardar segundos leyendo del torrent).
        let extracted = {
            let streams = state.streams.lock().await;
            streams.get(&id).map(|active| {
                (
                    active.handle.torrent_arc(),
                    active.handle.file_id,
                    active.handle.file_len,
                )
            })
        };
        if let Some((mt, fid, flen)) = extracted {
            stream::compute_moviehash(mt, fid, flen).await
        } else {
            None
        }
    } else {
        None
    };

    tracing::info!(
        target: "subs",
        stream_id = ?stream_id,
        moviehash = ?moviehash,
        imdb_id = ?imdb_id,
        query = ?query,
        langs = ?langs,
        "search_subtitles"
    );

    // Estrategia Stremio-like: ejecutar EN PARALELO las dos búsquedas
    // (hash → sync perfecta; imdb/query → catálogo completo) y
    // fusionarlas. El corto-circuito anterior (return al primer hit
    // por hash) ocultaba el catálogo entero: una peli con hash
    // indexado se quedaba en 1-3 subs cuando Stremio mostraba 200+.
    //
    // La REST v1 de OpenSubtitles combina filtros con AND, así que
    // NO podemos mandar hash + imdb en la misma request (dejaría
    // fuera todos los subs cuyo release no matchee el fichero
    // exacto). Por eso son dos requests separadas + merge en cliente.
    //
    // Orden final: hash-matches primero (perfect sync arriba), luego
    // el resto ordenado como venga del `search` (trusted → downloads
    // por idioma). Dedup por `file_id` conservando la primera
    // aparición → si un sub aparece en ambos, se queda como
    // hash-match.
    let http_ref = &state.http;
    let hash_fut = async {
        if let Some(hash) = moviehash.as_deref() {
            match subtitles::search(http_ref, Some(hash), None, None, None, None, &langs).await {
                Ok(mut subs) => {
                    for s in &mut subs {
                        s.hash_match = true;
                    }
                    Ok(subs)
                }
                Err(e) => Err(e),
            }
        } else {
            Ok(Vec::new())
        }
    };
    let catalog_fut = async {
        if imdb_id.is_some() || query.is_some() {
            subtitles::search(
                http_ref,
                None,
                imdb_id.as_deref(),
                query.as_deref(),
                season,
                episode,
                &langs,
            )
            .await
        } else {
            Ok(Vec::new())
        }
    };
    let (hash_res, catalog_res) = tokio::join!(hash_fut, catalog_fut);

    let hash_subs = match hash_res {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(target: "subs", error = %e, "hash search failed");
            Vec::new()
        }
    };
    let catalog_subs = match catalog_res {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(target: "subs", error = %e, "catalog search failed");
            Vec::new()
        }
    };

    // Dedup estable por file_id: primero los hash-matches (mantienen
    // su flag), luego los del catálogo. Un HashSet basta porque
    // file_id es único en OpenSubtitles.
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut merged: Vec<Subtitle> = Vec::with_capacity(hash_subs.len() + catalog_subs.len());
    for s in hash_subs.into_iter().chain(catalog_subs) {
        if seen.insert(s.file_id) {
            merged.push(s);
        }
    }

    tracing::info!(
        target: "subs",
        results = merged.len(),
        hash_matched = merged.iter().filter(|s| s.hash_match).count(),
        "merged results"
    );
    Ok(merged)
}

/// Directorio persistente para los `.srt` descargados por
/// `download_subtitle`. Antes vivía en `$TMPDIR/videodrome-subs/`,
/// pero eso rompía el resume: al arrancar borrábamos el tempdir y
/// el `path` guardado en `movie_progress.json → last_sub` quedaba
/// muerto. Al re-entrar la peli el sub aparecía "seleccionado" en
/// el panel pero `subtitle_to_vtt(path)` fallaba y el `<track>` no
/// se pintaba.
///
/// Al ser cache (no config) el sistema puede limpiarlo si escasea
/// disco, pero NO entre sesiones: macOS/Linux/Windows respetan
/// `~/Library/Caches`, `~/.cache`, `%LOCALAPPDATA%`.
fn subs_cache_dir() -> Result<std::path::PathBuf, String> {
    let dir = dirs::cache_dir()
        .ok_or_else(|| "No se puede obtener el directorio de cach\u{e9} del sistema".to_string())?
        .join("videodrome")
        .join("subs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("crear {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Descarga un subtítulo y devuelve la ruta local. El frontend le pasa
/// esta ruta a `start_stream_with_sub` para arrancar el stream con el
/// `.srt` ya cargado en VLC.
#[tauri::command]
async fn download_subtitle(sub: Subtitle, state: State<'_, AppState>) -> Result<String, String> {
    let dest = subs_cache_dir()?;
    let path = subtitles::download(&state.http, &sub, &dest)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Lee un fichero de subtítulos local (path arbitrario: descargado
/// por `download_subtitle`, extraído por ffmpeg a temp, o **soltado
/// por el user via drag&drop** desde su file manager) y lo devuelve
/// como WebVTT en memoria. El player HTML lo consume vía
/// `URL.createObjectURL(new Blob([vtt], { type: 'text/vtt' }))` para
/// alimentar un `<track>` sin escribir un fichero temporal más.
///
/// Formato detectado por contenido, no por extensión (los `.srt` que
/// vienen de compresores raros a veces vienen sin extensión, y los
/// `.vtt` con extensión inventada existen):
///
///   * Si el fichero empieza por `WEBVTT` (opcionalmente con BOM
///     UTF-8), se devuelve tal cual — ya es un VTT válido.
///   * Cualquier otra cosa se asume SRT y se pasa por `srt_to_vtt`
///     (que ya sabe manejar mojibake, encodings raros, BOM…).
///
/// Casos NO soportados hoy: ASS/SSA (styling complejo, requiere
/// converter dedicado) y bitmap subs (PGS/DVBSUB, ya se filtran
/// arriba en la UI). Un `.ass` soltado se detectará como "no
/// WEBVTT" y se pasará a `srt_to_vtt`, que devolverá basura — el
/// user verá los cues raros pero no crashea.
#[tauri::command]
async fn subtitle_to_vtt(path: String) -> Result<String, String> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("No se pudo leer {path}: {e}"))?;
    // Detección WEBVTT: skip BOM UTF-8 (`EF BB BF`) si existe, y
    // comprueba el header. Case-sensitive: la spec obliga a
    // mayúsculas.
    let head = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    if head.starts_with(b"WEBVTT") {
        let text = String::from_utf8_lossy(head).into_owned();
        return Ok(text);
    }
    Ok(subtitles::srt_to_vtt(&bytes))
}

// Nota histórica: antes existía un mapa `pending_subs` que asociaba un
// `stream_id` a una ruta de subtítulo pre-descargada; era código muerto
// (el id se asignaba dentro de `start_stream`, así que el frontend
// nunca podía registrarlo con el id correcto). El flujo actual es
// exclusivamente `start_stream_with_sub(magnet, sub_path?)`.

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
        (
            "torrent_search",
            "Resultados de torrents (30 min / 5 min vacío)",
            TORRENT_CACHE_FILE,
        ),
        // Caches anti-caída de TMDB (extendidos en Opción 1+2):
        // sirven metadatos ya vistos cuando TMDB tiene un incidente.
        (
            "tmdb_search",
            "Búsquedas TMDB (títulos)",
            "tmdb_search_cache.json",
        ),
        ("tmdb_view", "Detalles TMDB (modal)", "tmdb_view_cache.json"),
        (
            "tmdb_details",
            "Detalles TMDB (torrents)",
            "tmdb_details_cache.json",
        ),
    ]
}

#[tauri::command]
async fn cache_info() -> Result<Vec<CacheEntry>, String> {
    let dir = config_dir().map_err(|e| e.to_string())?;
    let mut out: Vec<CacheEntry> = cache_files()
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
        .collect();

    // Entrada virtual para la caché de streams: agrega tamaño de todas
    // las carpetas `<hash>/`. El path apunta al directorio raíz para que
    // el user lo pueda inspeccionar (es donde el prune actúa).
    let streams_root = stream::cache_dir().map_err(|e| e.to_string())?;
    let streams_size = stream::total_size();
    let streams_modified = std::fs::metadata(&streams_root)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let streams_exists = streams_size > 0;
    out.push(CacheEntry {
        kind: "streams",
        label: "Streams (piezas de BitTorrent)",
        path: streams_root.display().to_string(),
        exists: streams_exists,
        size_bytes: streams_size,
        modified_at: streams_modified,
    });

    Ok(out)
}

/// Borra uno o todos los ficheros de caché. `kind = "all"` los borra
/// todos de golpe. El kind `"streams"` es especial: barre el
/// directorio persistente de streams (`~/.cache/videodrome/streams/`)
/// que usa librqbit para reanudar bajadas entre sesiones. Nunca borra
/// `token.json` — la sesión se cierra con `logout`, no aquí.
#[tauri::command]
async fn clear_cache(kind: String, state: State<'_, AppState>) -> Result<(), String> {
    let dir = config_dir().map_err(|e| e.to_string())?;
    let known = cache_files();

    let (files_to_delete, wipe_streams): (Vec<&'static str>, bool) = match kind.as_str() {
        "all" => (known.iter().map(|(_, _, f)| *f).collect(), true),
        "streams" => (vec![], true),
        other => {
            let f = known
                .iter()
                .find(|(k, _, _)| *k == other)
                .map(|(_, _, f)| *f)
                .ok_or_else(|| format!("caché desconocida: {other}"))?;
            (vec![f], false)
        }
    };

    for file in files_to_delete {
        let path = dir.join(file);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Error al borrar {}: {e}", path.display()))?;
        }
    }

    if wipe_streams {
        stream::clear_all().map_err(|e| e.to_string())?;
    }

    // El cache de búsqueda vive también en memoria: si lo borramos del
    // disco pero no del state, la siguiente consulta vuelve a escribirlo
    // con los datos viejos. Vaciar el mapa cuando corresponda.
    if kind == "all" || kind == "search" {
        state.search_cache.lock().await.clear();
    }
    if kind == "all" || kind == "torrent_search" {
        state.torrent_cache.lock().await.clear();
    }
    Ok(())
}

#[tauri::command]
async fn get_preferences() -> Result<Preferences, String> {
    Ok(preferences::load())
}

#[tauri::command]
async fn set_preferences(prefs: Preferences) -> Result<(), String> {
    // Invalidación de caches TMDB al cambiar `ui_language`: los
    // datos guardados (title, overview, genres) están en el idioma
    // ANTERIOR — si el user acaba de pasar de es→fr veríamos las
    // sinopsis en español hasta que expirara el TTL (24h). Como el
    // trigger es raro (cambio manual) y los caches son cheap-to-
    // rebuild, los borramos in-place antes de persistir la pref.
    let prev = preferences::load();
    let lang_changed = prev.ui_language != prefs.ui_language;
    preferences::save(&prefs).map_err(|e| e.to_string())?;
    if lang_changed {
        for kind in ["tmdb_view", "tmdb_search", "tmdb_details", "tmdb_recs"] {
            let _ = purge_cache_kind(kind);
        }
        // Series/season caches viven bajo nombres específicos —
        // los borramos por path directo para no acoplar `clear_cache`
        // a nuevas kinds.
        if let Ok(dir) = config_dir() {
            for f in ["tmdb_series_details_cache.json", "tmdb_season_cache.json"] {
                let _ = std::fs::remove_file(dir.join(f));
            }
        }
    }
    Ok(())
}

// ---------- About / logs ----------

/// Info sobre la app + capa de logging, consumida por la sección
/// "Acerca de" de Ajustes. `enabled=false` cuando el user forzó
/// `VIDEODROME_LOG=0`. El path puede no existir todavía si la app
/// arrancó hace segundos y aún no ha flusheado ninguna línea.
#[derive(Serialize)]
struct AppLogInfo {
    version: &'static str,
    enabled: bool,
    dir: Option<String>,
    file: Option<String>,
    explicit_path: bool,
}

#[tauri::command]
fn log_info() -> AppLogInfo {
    let info = crate::logging::log_info();
    AppLogInfo {
        version: env!("CARGO_PKG_VERSION"),
        enabled: info.as_ref().map(|i| i.enabled).unwrap_or(false),
        dir: info
            .as_ref()
            .and_then(|i| i.dir.as_ref())
            .map(|p| p.display().to_string()),
        file: info
            .as_ref()
            .and_then(|i| i.file.as_ref())
            .map(|p| p.display().to_string()),
        explicit_path: info.as_ref().map(|i| i.explicit_path).unwrap_or(false),
    }
}

/// Abre la carpeta de logs en el file manager nativo. En Windows
/// invocamos `explorer` (silencia el error si la carpeta no existe
/// todavía — el user acaba de arrancar la app sin haber escrito
/// nada); en macOS `open`; en Linux `xdg-open`. Espejo del patrón
/// usado por `open_magnet`.
#[tauri::command]
fn open_log_folder() -> Result<(), String> {
    let info = crate::logging::log_info().ok_or_else(|| "logging no inicializado".to_string())?;
    if !info.enabled {
        return Err("logging deshabilitado (VIDEODROME_LOG=0)".to_string());
    }
    let target = info
        .dir
        .clone()
        .or_else(|| {
            info.file
                .as_ref()
                .and_then(|f| f.parent().map(|p| p.to_path_buf()))
        })
        .ok_or_else(|| "no hay carpeta de logs".to_string())?;
    // Crear el directorio si el user pulsa el botón antes de que se
    // haya escrito ninguna línea (arranque en frío en Windows con la
    // capa fichero recién montada).
    std::fs::create_dir_all(&target).ok();
    #[cfg(target_os = "macos")]
    let out = std::process::Command::new("open").arg(&target).spawn();
    #[cfg(target_os = "linux")]
    let out = std::process::Command::new("xdg-open").arg(&target).spawn();
    #[cfg(target_os = "windows")]
    let out = std::process::Command::new("explorer").arg(&target).spawn();
    out.map(|_| ()).map_err(|e| e.to_string())
}

/// Borra un cache tipo `"tmdb_search"`, `"tmdb_view"`, etc. Existe
/// como helper aparte para poderlo llamar sin pasar por el comando
/// Tauri `clear_cache` (que además invalida más cosas y hace lock
/// del `AppState`).
fn purge_cache_kind(kind: &str) -> anyhow::Result<()> {
    let dir = config_dir()?;
    let file = match kind {
        "tmdb_view" => "tmdb_view_cache.json",
        "tmdb_search" => "tmdb_search_cache.json",
        "tmdb_details" => "tmdb_details_cache.json",
        "tmdb_recs" => "tmdb_recs_cache.json",
        _ => return Ok(()),
    };
    let path = dir.join(file);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

// ---------- Entry point ----------

pub fn run(config: Config, http: reqwest::Client) -> anyhow::Result<()> {
    let state = AppState {
        config: Arc::new(Mutex::new(config)),
        http,
        streams: Arc::new(Mutex::new(HashMap::new())),
        next_stream_id: Arc::new(Mutex::new(0)),
        search_cache: Arc::new(Mutex::new(load_search_cache())),
        torrent_cache: Arc::new(Mutex::new(load_torrent_cache())),
        recs_pool: Arc::new(Mutex::new(None)),
    };

    // Prune de la caché de streams al arrancar, en un hilo aparte para
    // no bloquear el splash: si el user tiene 40 GB de pelis viejas,
    // los `remove_dir_all` se pueden llevar unos segundos. El TTL se
    // lee de Preferences (default 7 días); un TTL de 0 se trata como 1
    // día dentro de `stream::prune` para evitar recoger entradas que
    // el drop de un StreamHandle acaba de tocar.
    std::thread::spawn(|| {
        let prefs = preferences::load();
        let _ = stream::prune(prefs.stream_cache_ttl_days);
        // Barrido one-shot del antiguo `$TMPDIR/videodrome-subs/`
        // (versiones <= 2026-07 lo usaban como destino de
        // `download_subtitle`). El destino nuevo es persistente
        // (`<cache>/videodrome/subs/`) — necesario para que el
        // resume pueda re-abrir el `.srt` guardado en `last_sub`.
        // Los subs viejos ya no se referencian, así que borrar el
        // tempdir viejo es seguro y libera unos KB.
        let legacy_subs = std::env::temp_dir().join("videodrome-subs");
        let _ = std::fs::remove_dir_all(&legacy_subs);
        // Y los tempdirs huérfanos de sesiones HLS/stream previas
        // (Fase F del audit Windows: en NTFS el `TempDir::drop` no
        // puede unlinkear ficheros con handles abiertos, así que
        // basura queda entre ejecuciones). El barrido es seguro
        // porque solo mira dirs con nuestros prefijos.
        let _ = stream::prune_orphan_tempdirs();
    });

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
            get_recommendations_page,
            dismiss_recommendation,
            undismiss_recommendation,
            list_dismissed,
            clear_dismissed,
            mark_watched,
            unmark_watched,
            list_watched,
            clear_watched,
            get_movie_view,
            get_series_view,
            get_series_season,
            search_movies_tmdb,
            search_torrents_by_tmdb,
            search_torrents_series,
            search_torrents_direct,
            open_magnet,
            start_stream,
            start_stream_with_sub,
            start_stream_html,
            list_torrent_files,
            resolve_dropped_torrent,
            ffmpeg_available,
            set_client_capabilities,
            stream_stats,
            stop_stream,
            get_resume,
            report_position,
            list_watch_progress,
            remove_watch_progress,
            subtitles_available,
            search_subtitles,
            download_subtitle,
            subtitle_to_vtt,
            cache_info,
            clear_cache,
            get_preferences,
            set_preferences,
            log_info,
            open_log_folder,
        ])
        .run(tauri::generate_context!())
        .context("Error al ejecutar la app Tauri")
}
