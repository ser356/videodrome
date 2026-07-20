//! `StreamHandle` — el objeto que devuelve `start_with_target` y
//! consume la GUI/TUI. Contiene:
//!
//! * el `Arc<ManagedTorrent>` de librqbit;
//! * el `TempDir` opcional (solo cuando no hay infohash y caemos a
//!   caché efímera);
//! * el `AppState` compartido con los handlers HTTP (fields
//!   `pub(super)` para que server/hls puedan mutar sin getters);
//! * la lógica de descubrimiento del fichero de vídeo dentro del
//!   torrent (`select_file`, `list_files`, `TorrentFileInfo`);
//! * el arranque del servidor HTTP local (`start_with_target`) que
//!   compone el `Router` de axum con los handlers de `server` y
//!   `hls`;
//! * el `Drop` que persiste la fracción de reproducción a
//!   `resume.json` antes de cancelar la sesión BitTorrent.
//!
//! Extraído de `stream.rs` en el refactor (commit paso 5). Sin
//! cambios de comportamiento.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::routing::get;
use axum::Router;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, SessionOptions,
    SessionPersistenceConfig,
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::cache::parse_infohash;
use super::cache::{cache_dir, now_unix, touch_last_used};
#[cfg(feature = "gui")]
use super::hls::{serve_hls_playlist, serve_hls_segment};
use super::resume::{read_store, write_store_atomic, ResumeParse, ResumeStore, RESUME_FILE};
use super::server::{add_cors_headers, serve_video};
#[cfg(feature = "gui")]
use super::server::{log_hls_requests, serve_embedded_subtitle, serve_probe, set_hls_audio};
use super::state::AppState;

/// Subdirectorio (dentro de `<cache>/streams/<infohash>/`) donde
/// librqbit persiste su estado por-torrent para fastresume: el
/// `session.json` (índice) + `<hash>.bitv` (bitfield de piezas
/// completadas) + `<hash>.torrent` (metainfo). Sin esto, cada
/// apertura del mismo torrent re-hashea el fichero entero antes de
/// hacer NADA (audit §1: ~20 s para 10.5 GiB, proporcional al
/// tamaño → ~2 min en un remux UHD de 60 GB).
///
/// Colocarlo DENTRO del dir del infohash es intencional:
/// `clear_all()` y `prune()` borran ese dir por completo, así que
/// el estado se limpia solo cuando limpiamos la caché. Sin trabajo
/// extra ni riesgo de fastresume apuntando a ficheros ya borrados.
const LIBRQBIT_SESSION_SUBDIR: &str = ".session";

/// Handle de una sesión de streaming activa. `Drop` cancela el servidor
/// HTTP, detiene la sesión BitTorrent y — si tenemos infohash — persiste
/// el `resume.json` con la fracción de reproducción alcanzada. Los
/// ficheros de vídeo se conservan en la caché (`streams/<infohash>/`)
/// para acelerar la siguiente reproducción. Solo se borran cuando el
/// magnet no tenía infohash (fallback a tempdir) o cuando el prune por
/// TTL los recoge.
pub struct StreamHandle {
    pub url: String,
    pub file_name: String,
    pub file_len: u64,
    /// Índice del fichero de vídeo dentro del torrent multi-file. Se
    /// usa para llamar `handle.stream(file_id)` desde fuera del
    /// módulo (p.ej. `compute_moviehash`). Solo consumido con feature
    /// `gui`; en CLI/TUI el streaming va por VLC directo y no hace
    /// falta.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub file_id: usize,
    /// Infohash (hex-lowercase o base32) extraído del magnet, si se
    /// pudo parsear. Los callers lo usan para llamar a `load_resume`.
    pub infohash: Option<String>,
    handle: Arc<ManagedTorrent>,
    cancel: CancellationToken,
    /// Mayor `start` (bytes) visto en un Range HTTP con start explícito.
    /// Los suffix ranges (índice al final del MP4) no lo tocan.
    max_seek: Arc<AtomicU64>,
    /// Directorio de datos del torrent. Persistente cuando hay infohash;
    /// tempdir cuando no.
    data_dir: PathBuf,
    _session: Arc<Session>,
    /// `Some` cuando el magnet no tenía infohash y caemos a tempdir
    /// efímero. `None` cuando usamos caché persistente.
    _tempdir: Option<TempDir>,
    _server_task: JoinHandle<()>,
}

/// Snapshot del progreso de un stream en curso.
pub struct StreamStats {
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub live_peers: u32,
    pub down_mbps: f64,
}

impl StreamHandle {
    pub fn stats(&self) -> StreamStats {
        let s = self.handle.stats();
        let down_mbps = self
            .handle
            .live()
            .map(|l| l.down_speed_estimator().mbps())
            .unwrap_or(0.0);
        let live_peers = s
            .live
            .as_ref()
            .map(|l| l.snapshot.peer_stats.live as u32)
            .unwrap_or(0);
        StreamStats {
            progress_bytes: s.progress_bytes,
            total_bytes: s.total_bytes,
            live_peers,
            down_mbps,
        }
    }

    /// Clona el `Arc<ManagedTorrent>` interno. Los callers que quieran
    /// hacer `compute_moviehash` (free function del módulo) sin
    /// retener el `MutexGuard` del map de streams (para no bloquear
    /// stats/stop) extraen las 3 piezas dentro del lock y ejecutan el
    /// cómputo fuera. Solo se usa desde la GUI.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn torrent_arc(&self) -> Arc<ManagedTorrent> {
        self.handle.clone()
    }
}

/// Free-function variante de `StreamHandle::compute_moviehash`: útil
/// cuando el caller ya ha soltado el lock del map de streams pero
/// conserva las 3 piezas necesarias (Arc del ManagedTorrent + file id
/// + file len). Evita retener el `MutexGuard` durante el await, que
///   bloquearía otras operaciones sobre el map de streams (stats, stop).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub async fn compute_moviehash(
    handle: Arc<ManagedTorrent>,
    file_id: usize,
    file_len: u64,
) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
    const CHUNK: u64 = 65536;
    if file_len < CHUNK * 2 {
        return None;
    }
    let fut = async move {
        let mut stream = handle.stream(file_id).ok()?;
        let mut first = vec![0u8; CHUNK as usize];
        stream.read_exact(&mut first).await.ok()?;
        stream.seek(SeekFrom::Start(file_len - CHUNK)).await.ok()?;
        let mut last = vec![0u8; CHUNK as usize];
        stream.read_exact(&mut last).await.ok()?;
        crate::subtitles::compute_moviehash(file_len, &first, &last)
    };
    match tokio::time::timeout(std::time::Duration::from_secs(10), fut).await {
        Ok(res) => res,
        Err(_) => {
            tracing::warn!(target: "subs", "compute_moviehash timeout at 10s (peers lentos)");
            None
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Persistir resume ANTES de cancelar la sesión — la escritura es
        // síncrona y solo toca `<data_dir>/resume.json`, que no depende
        // del motor de librqbit.
        //
        // Merge-style con resiliencia a corrupción: si el player HTML
        // llamó a `save_position`, tendrá `seconds`+`duration_seconds`
        // que NO queremos pisar. Si el fichero existe pero no parsea
        // (write parcial anterior), NO lo sobreescribimos — mejor
        // dejar el corrupto que reemplazarlo por un default limpio
        // que pierde toda la info previa. Solo escribimos si podemos
        // hacer un merge honesto.
        //
        // Multi-file (§6 audit): escribimos SOLO la entrada
        // `files["<file_id>"]` del store — otras entradas del mismo
        // torrent (otros episodios) sobreviven intactas.
        if let Some(hash) = self.infohash.as_deref() {
            let max = self.max_seek.load(Ordering::Relaxed);
            if self.file_len > 0 {
                let fraction = (max as f32 / self.file_len as f32).clamp(0.0, 1.0);
                let path = self.data_dir.join(RESUME_FILE);
                let existing = match read_store(&path) {
                    ResumeParse::Store(s) => Some(s),
                    ResumeParse::Absent => Some(ResumeStore::default()),
                    ResumeParse::Corrupt => None,
                };
                if let Some(mut store) = existing {
                    let key = self.file_id.to_string();
                    let mut entry_r = store.files.remove(&key).unwrap_or_default();
                    entry_r.fraction = fraction;
                    entry_r.updated_at = now_unix();
                    store.files.insert(key, entry_r);
                    if let Err(e) = write_store_atomic(&path, &store) {
                        tracing::warn!(target: "resume", error = %e, "Drop: atomic write failed");
                    }
                }
            }
            // Tocar el sentinel para que el prune vea "usado ahora".
            let _ = touch_last_used(&self.data_dir);
            let _ = hash; // solo lo usamos para saber que la caché es persistente
        }
        self.cancel.cancel();
    }
}

/// Lista de trackers públicos que se inyectan en cada torrent. Muchos
/// magnets vienen con lista de `tr=` casi vacía (o solo con trackers
/// caídos), y sin trackers ni DHT rápido el motor se queda esperando peers
/// para siempre. Estos son de la lista comunitaria "trackerslist" (los más
/// vivos y con más torrents anunciados).
const EXTRA_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.moeking.me:6969/announce",
    "udp://explodie.org:6969/announce",
    "udp://ipv4.tracker.harry.lu:80/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://p4p.arenabg.com:1337/announce",
    "udp://tracker.tiny-vps.com:6969/announce",
    "udp://retracker.lanta-net.ru:2710/announce",
    "http://tracker.opentrackr.org:1337/announce",
];

/// Cuánto esperamos a que el magnet resuelva metadata antes de rendirnos.
const METADATA_TIMEOUT_SECS: u64 = 45;

/// Extensiones consideradas "vídeo" a la hora de elegir fichero
/// dentro de un torrent multi-file. El resto se ignora (samples,
/// extras, nfo).
const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "avi", "m4v", "ts", "webm", "mov", "wmv"];

/// Tamaño mínimo para considerar un fichero "de contenido" y no
/// sample. 50 MB es el umbral que la scene usa históricamente.
const MIN_VIDEO_SIZE_BYTES: u64 = 50 * 1024 * 1024;

/// Info por-fichero devuelta al frontend por `list_files` para que
/// pueda ofrecer selección manual (packs con numeración absoluta de
/// anime, encoding raro, etc.). Serialized snake_case para el
/// consumidor JS.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TorrentFileInfo {
    pub file_id: usize,
    pub name: String,
    pub size: u64,
    pub season: Option<u16>,
    pub episode: Option<u16>,
    /// Es candidato realista a "el vídeo del episodio" (extensión
    /// vídeo + tamaño > sample). El frontend puede filtrar por esto.
    pub is_video: bool,
}

/// Elige el fichero a servir dentro de la lista de un torrent.
///
/// * `target = None` → el vídeo más grande (comportamiento pre-audit,
///   correcto para películas y torrents mono-fichero).
/// * `target = Some(Episode(S, E))` → parsea cada nombre con
///   `release_name::parse` y elige el que matchee S+E. Si varios
///   matchean (mismo episodio en calidades duplicadas), el más
///   grande de ellos. Si ninguno matchea, cae al mayor — así una
///   heurística de S/E fallida no bloquea el arranque.
/// * `target = Some(Index(i))` → devuelve directo `files[i]` (con
///   bounds check). Se usa cuando el provider ya resolvió el índice
///   (Torrentio.fileIdx) y saltarnos el parser evita el edge case de
///   packs con numeración absoluta de anime.
///
/// Filtra ficheros de tamaño < 50 MB para no picar samples/extras.
pub fn select_file(
    files: &[(usize, String, u64)],
    target: Option<crate::torrents::FileSelector>,
) -> Option<(usize, String, u64)> {
    use crate::torrents::FileSelector;

    // Índice directo: el provider ya nos dijo cuál. Bypass del
    // filtro de samples porque el proveedor sabe mejor que la
    // heurística "> 50 MB" cuando el fichero elegido es válido.
    if let Some(FileSelector::Index(i)) = target {
        if let Some(f) = files.iter().find(|(id, _, _)| *id == i) {
            return Some(f.clone());
        }
        // Fuera de rango: cae al mayor. Mejor un fichero incorrecto
        // que un error duro.
    }

    // Vídeos "reales" (ext conocida + tamaño > sample). Si el filtro
    // deja lista vacía (torrent con nombres no estándar), volvemos al
    // set completo antes de descartar.
    let is_video = |name: &str, size: u64| {
        size >= MIN_VIDEO_SIZE_BYTES
            && std::path::Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| VIDEO_EXTS.contains(&e))
                .unwrap_or(false)
    };
    let candidates: Vec<&(usize, String, u64)> =
        files.iter().filter(|(_, n, s)| is_video(n, *s)).collect();
    let pool: Vec<&(usize, String, u64)> = if candidates.is_empty() {
        files.iter().collect()
    } else {
        candidates
    };

    if let Some(FileSelector::Episode(qs, qe)) = target {
        let matches: Vec<&&(usize, String, u64)> = pool
            .iter()
            .filter(|(_, n, _)| {
                let p = crate::torrents::release_name::parse(n);
                matches!((p.season, p.episode), (Some(ps), Some(pe)) if ps == qs && pe == qe)
            })
            .collect();
        if let Some(best) = matches.iter().max_by_key(|(_, _, s)| *s) {
            return Some((***best).clone());
        }
        // Sin match exacto: fallback al mayor del pool (mismo que sin
        // target). Mejor un fichero incorrecto que un error duro —
        // el user puede seleccionar manual con `list_files`.
    }

    pool.iter()
        .max_by_key(|(_, _, s)| *s)
        .map(|f| (**f).clone())
}

/// Lista los ficheros del torrent (resolviendo metadata) sin
/// arrancar servidor HTTP ni empezar a bajar contenido. Útil para
/// que la UI ofrezca selección manual en packs con nombres raros.
///
/// La sesión se dropea al retornar — no deja recursos vivos. Usa la
/// misma caché persistente que `start` (mismo `<cache>/streams/<hash>/`),
/// así que si el user llama a esto y después a `start` sobre el
/// mismo magnet, librqbit reutiliza lo bajado.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub async fn list_files(magnet: String) -> Result<Vec<TorrentFileInfo>> {
    let infohash = parse_infohash(&magnet);
    let (data_dir, _tempdir_guard) = match infohash.as_deref() {
        Some(hash) => {
            let dir = cache_dir()?.join(hash);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("No se pudo crear {}", dir.display()))?;
            (dir, None)
        }
        None => {
            let td = tempfile::Builder::new()
                .prefix("videodrome-listfiles-")
                .tempdir()
                .context("No se pudo crear directorio temporal")?;
            (td.path().to_path_buf(), Some(td))
        }
    };

    let cancel = CancellationToken::new();
    let session = Session::new_with_opts(
        data_dir.clone(),
        SessionOptions {
            disable_dht_persistence: true,
            persistence: None,
            cancellation_token: Some(cancel.clone()),
            ..Default::default()
        },
    )
    .await
    .context("Error inicializando la sesión de librqbit")?;

    let response = session
        .add_torrent(
            AddTorrent::from_url(&magnet),
            Some(AddTorrentOptions {
                overwrite: true,
                // Modo list-only: no arranca la descarga, solo pide
                // metadata. Al drop de `session`, no queda nada
                // corriendo. `paused: true` sería otra opción pero
                // reutilizamos la ruta normal para simplicidad.
                paused: true,
                trackers: Some(EXTRA_TRACKERS.iter().map(|s| s.to_string()).collect()),
                ..Default::default()
            }),
        )
        .await
        .context("Error al añadir el torrent")?;

    let handle: Arc<ManagedTorrent> = match response {
        AddTorrentResponse::Added(_, h) => h,
        AddTorrentResponse::AlreadyManaged(_, h) => h,
        AddTorrentResponse::ListOnly(_) => anyhow::bail!("Torrent en modo list-only"),
    };

    tokio::time::timeout(
        std::time::Duration::from_secs(METADATA_TIMEOUT_SECS),
        handle.wait_until_initialized(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "Sin peers en {METADATA_TIMEOUT_SECS}s (magnet muerto o sin seeders reales)."
        )
    })?
    .context("Error resolviendo metadata del torrent")?;

    let out = handle
        .with_metadata(|md| {
            md.file_infos
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let name = f.relative_filename.to_string_lossy().into_owned();
                    let parsed = crate::torrents::release_name::parse(&name);
                    let ext = std::path::Path::new(&name)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_ascii_lowercase());
                    let is_video = f.len >= MIN_VIDEO_SIZE_BYTES
                        && ext
                            .as_deref()
                            .map(|e| VIDEO_EXTS.contains(&e))
                            .unwrap_or(false);
                    TorrentFileInfo {
                        file_id: i,
                        name,
                        size: f.len,
                        season: parsed.season,
                        episode: parsed.episode,
                        is_video,
                    }
                })
                .collect::<Vec<_>>()
        })
        .context("No se pudo leer metadata del torrent")?;

    // Dropear la sesión explícitamente antes de retornar — el
    // `_tempdir_guard` se dropea al retornar y no queremos que la
    // sesión aún esté abriendo ficheros dentro cuando se borre.
    cancel.cancel();
    drop(session);

    Ok(out)
}

/// Convierte los bytes de un fichero `.torrent` en un magnet URI
/// reutilizable por el pipeline existente (`list_files`, `start`,
/// `start_stream_html`). Se usa para el drop de ficheros `.torrent`
/// sobre la ventana (Fase drop-to-play).
///
/// `librqbit::torrent_from_bytes` ya deserializa el bencode Y
/// calcula el `info_hash` (sha1 del sub-diccionario `info`) — no
/// hace falta hashear a mano ni añadir el crate `sha1`. Los
/// trackers vienen de `announce` (single-tracker) + `announce-list`
/// (multi-tier, BEP12), deduplicados.
///
/// Devuelve `(magnet_uri, display_name)`. `display_name` sale de
/// `info.name` (nombre del torrent scene, ej.
/// `Movie.2024.1080p.BluRay.x264-GROUP`), con fallback al infohash
/// hex si el fichero no lleva name.
///
/// **No** valida piezas ni descarga nada — solo produce el string.
/// La resolución real (list_files → start_stream_html) se hace fuera.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn torrent_bytes_to_magnet(bytes: &[u8]) -> Result<(String, String)> {
    let meta = librqbit::torrent_from_bytes::<librqbit::ByteBufOwned>(bytes)
        .context("Fichero .torrent inválido")?;

    let mut trackers: Vec<String> = Vec::new();
    let push_tracker = |raw: &[u8], trackers: &mut Vec<String>| {
        if let Ok(s) = std::str::from_utf8(raw) {
            let s = s.trim();
            if !s.is_empty() && !trackers.iter().any(|t| t == s) {
                trackers.push(s.to_string());
            }
        }
    };
    if let Some(a) = &meta.announce {
        push_tracker(a.as_ref(), &mut trackers);
    }
    for tier in &meta.announce_list {
        for t in tier {
            push_tracker(t.as_ref(), &mut trackers);
        }
    }

    let magnet = librqbit::Magnet::from_id20(meta.info_hash, trackers, None).to_string();
    let display_name = meta
        .info
        .name
        .as_ref()
        .and_then(|b| std::str::from_utf8(b.as_ref()).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| meta.info_hash.as_string());
    Ok((magnet, display_name))
}

/// Arranca una sesión BitTorrent para el magnet dado, sirve el fichero
/// principal (el más grande) por HTTP en `127.0.0.1:PORT` y devuelve la
/// URL para el reproductor.
///
/// Si el magnet expone infohash, los datos se guardan en la caché
/// persistente (`<cache>/videodrome/streams/<infohash>/`) — la próxima
/// vez que se abra esta misma peli, librqbit reutiliza los ficheros y
/// arranca casi al instante. Sin infohash, se cae a un tempdir efímero.
///
/// `target`: ver `select_file`. `None` = fichero de vídeo más grande.
pub async fn start(magnet: String) -> Result<StreamHandle> {
    start_with_target(magnet, None).await
}

/// Variante con selección explícita de fichero. Ver `start` y
/// `select_file`.
pub async fn start_with_target(
    magnet: String,
    target: Option<crate::torrents::FileSelector>,
) -> Result<StreamHandle> {
    let infohash = parse_infohash(&magnet);

    // Directorio de datos: caché persistente si hay infohash, tempdir si
    // no. `tempdir_guard` mantiene vivo el `TempDir` en el segundo caso;
    // cuando es `None`, el directorio persiste y solo lo limpia el
    // `prune` por TTL.
    let (data_dir, tempdir_guard) = match infohash.as_deref() {
        Some(hash) => {
            let dir = cache_dir()?.join(hash);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("No se pudo crear {}", dir.display()))?;
            // Tocamos el sentinel ya para que un prune concurrente no lo
            // borre justo antes de servir.
            let _ = touch_last_used(&dir);
            (dir, None)
        }
        None => {
            let td = tempfile::Builder::new()
                .prefix("videodrome-stream-")
                .tempdir()
                .context("No se pudo crear directorio temporal")?;
            (td.path().to_path_buf(), Some(td))
        }
    };

    // Un solo cancellation token para toda la sesión: se propaga al motor
    // librqbit (DHT, listeners TCP/UDP, tareas de fondo) y al servidor axum.
    // Sin esto, al hacer Drop del StreamHandle el DHT persistía en un
    // puerto UDP fijo y el siguiente `Session::new` fallaba con "address
    // already in use" hasta que el proceso se reiniciaba.
    let cancel = CancellationToken::new();

    // Persistencia por-torrent (audit §1): solo cuando tenemos
    // infohash y por tanto caché en disco. Sin esto, cada apertura
    // re-hashea el fichero entero antes de servir nada (~20 s por
    // 10.5 GiB, proporcional al tamaño). Con esto + `fastresume:
    // true`, librqbit reutiliza el `.bitv` de la sesión anterior y
    // salta el re-check. En magnets efímeros (sin infohash → tempdir)
    // no tiene sentido: al drop se borra todo igual.
    //
    // El folder vive DENTRO del dir del infohash → `clear_all` y
    // `prune` lo limpian con el resto de la entrada sin trabajo
    // extra ni riesgo de fastresume huérfano.
    let persistence = if infohash.is_some() {
        let folder = data_dir.join(LIBRQBIT_SESSION_SUBDIR);
        if let Err(e) = std::fs::create_dir_all(&folder) {
            tracing::warn!(
                target: "torrent",
                error = %e,
                dir = %folder.display(),
                "no se pudo crear el dir de persistencia; fallback a re-check completo"
            );
            None
        } else {
            Some(SessionPersistenceConfig::Json {
                folder: Some(folder),
            })
        }
    } else {
        None
    };
    let fastresume = persistence.is_some();

    let session = Session::new_with_opts(
        data_dir.clone(),
        SessionOptions {
            // No queremos que la sesión reutilice puertos DHT/estado entre
            // arranques — cada stream es efímero.
            disable_dht_persistence: true,
            persistence,
            fastresume,
            cancellation_token: Some(cancel.clone()),
            ..Default::default()
        },
    )
    .await
    .context("Error inicializando la sesión de librqbit")?;

    let response = session
        .add_torrent(
            AddTorrent::from_url(&magnet),
            Some(AddTorrentOptions {
                // Con caché persistente los ficheros ya existen; librqbit
                // los re-verifica pieza a pieza y solo baja lo que falta.
                overwrite: true,
                trackers: Some(EXTRA_TRACKERS.iter().map(|s| s.to_string()).collect()),
                ..Default::default()
            }),
        )
        .await
        .context("Error al añadir el torrent")?;

    let handle: Arc<ManagedTorrent> = match response {
        AddTorrentResponse::Added(_, h) => h,
        AddTorrentResponse::AlreadyManaged(_, h) => h,
        AddTorrentResponse::ListOnly(_) => anyhow::bail!("Torrent en modo list-only"),
    };

    // Timeout explícito: si el magnet no resuelve metadata en 45s
    // probablemente no hay peers vivos con el infohash. Mejor error claro
    // que "buscando…" para siempre.
    tokio::time::timeout(
        std::time::Duration::from_secs(METADATA_TIMEOUT_SECS),
        handle.wait_until_initialized(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "Sin peers en {METADATA_TIMEOUT_SECS}s (magnet muerto o sin seeders reales). \
             Prueba otro torrent con más seeders."
        )
    })?
    .context("Error resolviendo metadata del torrent")?;

    // Selección del fichero de vídeo a servir. Por defecto el más
    // grande (heurística estándar para películas mono-fichero). Si el
    // caller pidió un episodio concreto (season pack de serie), se
    // busca el fichero que matchee esa S+E parseando el nombre.
    let files: Vec<(usize, String, u64)> = handle
        .with_metadata(|md| {
            md.file_infos
                .iter()
                .enumerate()
                .map(|(i, f)| (i, f.relative_filename.to_string_lossy().into_owned(), f.len))
                .collect::<Vec<_>>()
        })
        .context("No se pudo leer metadata del torrent")?;

    let (file_id, file_name, file_len) =
        select_file(&files, target).context("Torrent sin ficheros")?;

    // Servidor HTTP local
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .context("No se pudo abrir puerto local")?;
    let addr = listener.local_addr()?;

    let state = AppState {
        handle: handle.clone(),
        file_id,
        file_len,
        active_request: Arc::new(tokio::sync::Mutex::new(None)),
        request_counter: Arc::new(AtomicU64::new(0)),
        max_seek: Arc::new(AtomicU64::new(0)),
        local_addr: addr,
        #[cfg(feature = "gui")]
        cached_probe: Arc::new(tokio::sync::Mutex::new(None)),
        #[cfg(feature = "gui")]
        hls: Arc::new(tokio::sync::Mutex::new(None)),
    };
    let max_seek = state.max_seek.clone();
    #[cfg(feature = "gui")]
    let app = Router::new()
        .route("/video", get(serve_video))
        .route("/probe.json", get(serve_probe))
        .route("/hls/playlist.m3u8", get(serve_hls_playlist))
        .route("/hls/{file}", get(serve_hls_segment))
        .route("/hls/audio", axum::routing::post(set_hls_audio))
        .route("/subs/embedded/{idx}", get(serve_embedded_subtitle))
        .layer(axum::middleware::from_fn(log_hls_requests))
        .layer(axum::middleware::from_fn(add_cors_headers))
        .with_state(state);
    #[cfg(not(feature = "gui"))]
    let app = Router::new()
        .route("/video", get(serve_video))
        .layer(axum::middleware::from_fn(add_cors_headers))
        .with_state(state);

    let cancel_task = cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel_task.cancelled().await })
            .await;
    });

    // Telemetría periódica al log (audit): cada 5 s, mientras el stream
    // esté vivo, emitimos progreso + velocidad de librqbit + peers +
    // playhead. Firma esperada del bug (probe atascado con descarga
    // activa): `down_mbps > 0` sostenido mientras `req#N` no llega a
    // su `done`.
    //
    // NIVEL `debug`: 12 líneas/min ≈ 720 líneas/hora reventarían el
    // presupuesto de <200 líneas info de una reproducción típica. El
    // audit da explícitamente esta escape hatch ("si se supera,
    // degradar telemetría a `debug`"). Para reproducir el bug del
    // probe, el usuario ejecuta con `VIDEODROME_LOG_LEVEL=debug`.
    // La tarea se apaga cuando `cancel` se dispara al drop del
    // `StreamHandle`.
    let telemetry_handle = handle.clone();
    let telemetry_max_seek = max_seek.clone();
    let telemetry_cancel = cancel.clone();
    let telemetry_file_len = file_len;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Primer tick es inmediato; lo consumimos para que el primer
        // log llegue a los 5 s reales, no al startup (evita ruido en
        // el arranque donde librqbit aún no tiene stats).
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = telemetry_cancel.cancelled() => return,
                _ = ticker.tick() => {}
            }
            let stats = telemetry_handle.stats();
            let down_mbps = telemetry_handle
                .live()
                .map(|l| l.down_speed_estimator().mbps())
                .unwrap_or(0.0);
            let live_peers = stats
                .live
                .as_ref()
                .map(|l| l.snapshot.peer_stats.live as u32)
                .unwrap_or(0);
            let progress_pct = if stats.total_bytes > 0 {
                (stats.progress_bytes as f64 / stats.total_bytes as f64) * 100.0
            } else {
                0.0
            };
            let playhead = telemetry_max_seek.load(Ordering::Relaxed);
            let playhead_pct = if telemetry_file_len > 0 {
                (playhead as f64 / telemetry_file_len as f64) * 100.0
            } else {
                0.0
            };
            tracing::debug!(
                target: "torrent",
                down_mbps = format!("{down_mbps:.2}"),
                peers = live_peers,
                progress_mb = stats.progress_bytes / 1_048_576,
                total_mb = stats.total_bytes / 1_048_576,
                progress_pct = format!("{progress_pct:.1}"),
                playhead_mb = playhead / 1_048_576,
                playhead_pct = format!("{playhead_pct:.1}"),
                "telemetry"
            );
        }
    });

    let url = format!("http://{addr}/video");

    Ok(StreamHandle {
        url,
        file_name,
        file_len,
        file_id,
        infohash,
        handle,
        cancel,
        max_seek,
        data_dir,
        _session: session,
        _tempdir: tempdir_guard,
        _server_task: server_task,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkfiles(items: &[(&str, u64)]) -> Vec<(usize, String, u64)> {
        items
            .iter()
            .enumerate()
            .map(|(i, (n, s))| (i, (*n).to_string(), *s))
            .collect()
    }

    #[test]
    fn select_file_default_picks_largest_video() {
        // Sin target: mayor vídeo. El README (2 MB, ni vídeo) se
        // ignora aunque sea único fichero .txt.
        let files = mkfiles(&[
            ("README.txt", 2 * 1024 * 1024),
            ("Movie.2019.1080p.mkv", 1500 * 1024 * 1024),
            ("sample.mkv", 30 * 1024 * 1024),
        ]);
        let (id, name, _) = select_file(&files, None).unwrap();
        assert_eq!(id, 1);
        assert!(name.contains("Movie.2019"));
    }

    #[test]
    fn select_file_target_matches_episode_in_pack() {
        use crate::torrents::FileSelector;
        let files = mkfiles(&[
            ("Fargo.S02E01.1080p.WEB-DL.x264-GRP.mkv", 900 * 1024 * 1024),
            ("Fargo.S02E02.1080p.WEB-DL.x264-GRP.mkv", 950 * 1024 * 1024),
            ("Fargo.S02E03.1080p.WEB-DL.x264-GRP.mkv", 800 * 1024 * 1024),
        ]);
        let (id, name, _) = select_file(&files, Some(FileSelector::Episode(2, 3))).unwrap();
        assert_eq!(id, 2);
        assert!(name.contains("S02E03"));
    }

    #[test]
    fn select_file_target_prefers_largest_of_dup_episodes() {
        use crate::torrents::FileSelector;
        // Pack con 720p y 1080p del mismo E03: gana el mayor.
        let files = mkfiles(&[
            ("Fargo.S02E03.720p.WEB-DL.x264.mkv", 400 * 1024 * 1024),
            ("Fargo.S02E03.1080p.WEB-DL.x264.mkv", 900 * 1024 * 1024),
        ]);
        let (id, _, _) = select_file(&files, Some(FileSelector::Episode(2, 3))).unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn select_file_target_falls_back_to_largest_when_no_match() {
        use crate::torrents::FileSelector;
        // Pedimos S05E01 pero el pack solo tiene S02. En vez de
        // devolver None, cae al mayor — mejor un fichero incorrecto
        // que un error duro; el user puede corregir con list_files.
        let files = mkfiles(&[
            ("Fargo.S02E01.mkv", 900 * 1024 * 1024),
            ("Fargo.S02E02.mkv", 950 * 1024 * 1024),
        ]);
        let (id, _, _) = select_file(&files, Some(FileSelector::Episode(5, 1))).unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn select_file_index_bypasses_heuristics() {
        // Con FileSelector::Index(i), el file elegido es el que dice
        // el provider — se salta hasta el filtro de samples porque
        // el provider (Torrentio) sabe mejor cuál es el bueno.
        use crate::torrents::FileSelector;
        let files = mkfiles(&[
            ("episode.mkv", 900 * 1024 * 1024),
            ("tiny.mkv", 10 * 1024 * 1024), // < 50 MB, normalmente sample
            ("huge.mkv", 3000 * 1024 * 1024),
        ]);
        let (id, _, _) = select_file(&files, Some(FileSelector::Index(1))).unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn select_file_index_out_of_range_falls_back_to_largest() {
        use crate::torrents::FileSelector;
        let files = mkfiles(&[
            ("small.mkv", 100 * 1024 * 1024),
            ("big.mkv", 900 * 1024 * 1024),
        ]);
        let (id, _, _) = select_file(&files, Some(FileSelector::Index(99))).unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn select_file_ignores_samples_under_50mb() {
        let files = mkfiles(&[
            ("Movie.sample.mkv", 40 * 1024 * 1024),
            ("Movie.1080p.mkv", 700 * 1024 * 1024),
        ]);
        let (id, _, _) = select_file(&files, None).unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn select_file_falls_back_to_full_pool_when_all_filtered() {
        // Torrent con nombres no estándar (sin extensión conocida)
        // NO debe devolver None — se procesa el pool entero.
        let files = mkfiles(&[("videofile1", 1_000_000_000), ("videofile2", 500_000_000)]);
        let (id, _, _) = select_file(&files, None).unwrap();
        assert_eq!(id, 0);
    }

    #[test]
    fn select_file_empty_returns_none() {
        let files: Vec<(usize, String, u64)> = vec![];
        assert!(select_file(&files, None).is_none());
    }
}
