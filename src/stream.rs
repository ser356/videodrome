//! Streaming BitTorrent al estilo Stremio (rudimentario): mientras se
//! descarga el fichero, se sirve por HTTP con soporte de Range para que
//! VLC (u otro reproductor) lo pueda reproducir progresivamente.
//!
//! Bajo el capó usa `librqbit` como motor BitTorrent embebido:
//! `handle.stream(file_id)` devuelve un `FileStream` que implementa
//! `AsyncRead + AsyncSeek`. Cada `read()` bloquea hasta que la pieza
//! necesaria está descargada, y registra el rango deseado con el piece
//! picker, que prioriza esas piezas — de facto es "descarga secuencial +
//! primera/última pieza primero" cuando VLC pide byte 0 (cabecera) y luego
//! byte final (para índice `mp4`/`mkv` en algunos casos).
//!
//! ## Caché persistente
//!
//! El fichero se escribe bajo `<cache>/videodrome/streams/<infohash>/` en
//! lugar de un tempdir efímero. Al re-abrir la misma peli, librqbit
//! verifica las piezas ya presentes en disco y arranca casi al instante
//! (sin re-bajar). Si el magnet no expone infohash (raro), se cae a un
//! tempdir tradicional que sí se borra al salir.
//!
//! Cada entrada guarda un fichero `.last_used` que se toca al start y al
//! drop del `StreamHandle`; el módulo `prune` borra las entradas cuyo
//! mtime supere el TTL (configurable en Preferences, default 7 días).
//!
//! ## Resume position
//!
//! El handler HTTP registra el mayor `start` de cada Range con start
//! explícito (los suffix ranges de índice se ignoran) en un `AtomicU64`.
//! Al hacer Drop del `StreamHandle`, se persiste `resume.json` con la
//! fracción `max_seek / file_len`. El caller (GUI) puede leerla con
//! `load_resume(infohash)` y pasar `start_seconds` a `open_in_vlc` para
//! que VLC arranque con `--start-time=<seg>`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(feature = "gui")]
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, SessionOptions,
};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::io::{AsyncSeekExt, SeekFrom};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// El trait solo se usa en spawns Windows y en helpers gui-only
// (spawn_hls, serve_embedded_subtitle). En la build CLI/TUI puro
// para macOS/Linux queda sin call sites y warnaría `unused_imports`.
#[allow(unused_imports)]
use crate::winutil::HideConsoleExt;

const LAST_USED_FILE: &str = ".last_used";
const RESUME_FILE: &str = "resume.json";

// ── Client capabilities (audit §4) ────────────────────────────
//
// Store global (static) para las capacidades del WebView reportadas
// por el frontend al arrancar. Es global porque hay UN solo WebView
// por proceso (Tauri single-window) y las caps no cambian en runtime
// — se leen una vez al mount de React. Los handlers HTTP (que viven
// en `stream::AppState` per-stream, sin acceso al `tauri::AppState`)
// las consultan vía `current_client_capabilities()`.
//
// Antes de que el frontend registre nada, se devuelve
// `ClientCapabilities::safe_default()` (H.264+AAC+MP3) — la matriz
// más restrictiva, equivalente al comportamiento pre-§4.
//
// Todo el bloque vive tras `#[cfg(feature = "gui")]` porque
// `crate::ffmpeg` está gateado a esa feature. Sin gui la CLI/TUI no
// tiene ni WebView ni caps que reportar; los helpers HLS que
// consumen `current_client_capabilities` también son gui-only.

#[cfg(feature = "gui")]
static CLIENT_CAPABILITIES: OnceLock<RwLock<crate::ffmpeg::ClientCapabilities>> = OnceLock::new();

#[cfg(feature = "gui")]
fn client_caps_slot() -> &'static RwLock<crate::ffmpeg::ClientCapabilities> {
    CLIENT_CAPABILITIES.get_or_init(|| RwLock::new(crate::ffmpeg::ClientCapabilities::default()))
}

/// Registra las capacidades del cliente. Idempotente y thread-safe;
/// llamado desde el comando Tauri `set_client_capabilities` con lo
/// que `canPlayType()` reporta al arranque del frontend. Sobreescribe
/// el valor anterior (una sola WebView, no hay ambigüedad).
#[cfg(feature = "gui")]
pub fn set_client_capabilities(caps: crate::ffmpeg::ClientCapabilities) {
    if let Ok(mut w) = client_caps_slot().write() {
        *w = caps;
    }
}

/// Snapshot actual de las caps. Si el frontend aún no ha reportado
/// (codecs vacío), devuelve el safe_default en su lugar — así los
/// consumidores nunca ven "cero códecs" (que sería equivalente a
/// "el cliente no puede reproducir nada").
#[cfg(feature = "gui")]
pub fn current_client_capabilities() -> crate::ffmpeg::ClientCapabilities {
    let caps = client_caps_slot().read().ok().map(|g| g.clone());
    match caps {
        Some(c) if !c.codecs.is_empty() => c,
        _ => crate::ffmpeg::ClientCapabilities::safe_default(),
    }
}

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

#[derive(Clone)]
struct AppState {
    handle: Arc<ManagedTorrent>,
    file_id: usize,
    file_len: u64,
    /// Token de la petición HTTP en curso para este stream + `Instant`
    /// en que arrancó. Cuando llega una nueva Range request (típicamente
    /// porque VLC ha hecho seek), cancelamos la anterior aquí antes de
    /// crear la nueva. Sin esto el FileStream antiguo sigue vivo dentro
    /// del `body` de axum — y librqbit intercala pieces de todos los
    /// FileStreams activos, con lo que el nuevo (el que VLC está
    /// esperando) solo se lleva la mitad del ancho de banda. Resultado:
    /// buffering infinito tras cada seek.
    ///
    /// El `Instant` sirve para detectar bursts concurrentes (WKWebView
    /// en modo DIRECT emite dos Range GET casi simultáneos para
    /// paralelizar la carga inicial): si la request anterior arrancó
    /// hace <150ms, asumimos que es parte del mismo burst y NO la
    /// cancelamos. Para consumidores secuenciales (VLC, ffmpeg-HLS) el
    /// intervalo entre seeks reales es de segundos, muy por encima del
    /// umbral.
    active_request: Arc<tokio::sync::Mutex<Option<(u64, CancellationToken, tokio::time::Instant)>>>,
    /// Contador atómico de peticiones a `/video` en la vida del
    /// stream. Se incrementa una vez por request y el valor se usa
    /// como `req_id` en el log (`req#N`) y en el slot
    /// `active_request` para poder loguear `cancelled_prev=<id>` sin
    /// pasar el id de forma explícita entre handlers. Overflow real
    /// después de 2^64 peticiones — ~584 años a 1e9 req/s.
    request_counter: Arc<AtomicU64>,
    /// Compartido con `StreamHandle`. Se actualiza en cada Range con
    /// start explícito (fetch_max) para trackear la posición de
    /// reproducción alcanzada — usada para persistir `resume.json`.
    max_seek: Arc<AtomicU64>,
    /// Addr del listener local — necesario para que los handlers del
    /// player HTML (`/probe.json`, `/play.mp4`) construyan la URL que
    /// pasan a ffprobe/ffmpeg como input (`http://127.0.0.1:PORT/video`).
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    local_addr: SocketAddr,
    /// Caché de `ffprobe` sobre el input. Se popula la primera vez que
    /// se pide `/probe.json` o `/play.mp4` y se reutiliza — ffprobe
    /// tarda 1-3s con Range requests sobre el stream de librqbit, no
    /// queremos pagarlo en cada seek.
    #[cfg(feature = "gui")]
    cached_probe: Arc<tokio::sync::Mutex<Option<crate::ffmpeg::MediaInfo>>>,
    /// Estado HLS del stream: tempdir compartido donde vive TODA la
    /// caché de segmentos transcodeados durante la vida del stream,
    /// más el `Child` del ffmpeg activo (opcional; hay ffmpeg solo
    /// cuando algún segmento está bajo demanda). Se crea
    /// perezosamente en la primera petición HLS y se libera al Drop
    /// del `StreamHandle` — el ffmpeg activo muere con
    /// `kill_on_drop=true` y el `TempDir` limpia disco.
    ///
    /// Modelo "VOD virtual" (estilo Stremio/Jellyfin): el playlist
    /// es una función pura de la duración (todos los segmentos
    /// enumerados desde el arranque, `#EXT-X-ENDLIST` incluido) y
    /// ffmpeg materializa segmentos bajo demanda cuando el player
    /// los pide. Un seek fuera de buffer se decide POR SEGMENTO
    /// pedido: si el índice cae fuera de la ventana del job actual,
    /// se reinicia; si cae dentro (o si el segmento ya existe en
    /// disco de una pasada anterior), se sirve sin tocar ffmpeg.
    #[cfg(feature = "gui")]
    hls: Arc<tokio::sync::Mutex<Option<HlsState>>>,
}

/// Estado HLS compartido durante la vida del stream. El `dir` /
/// `_tempdir` viven aquí (NO en `HlsJob`) porque queremos que los
/// segmentos producidos por un job sigan siendo cache válido para
/// el resto del stream — un seek hacia atrás a zona ya transcodeada
/// se sirve del disco sin respawn de ffmpeg.
#[cfg(feature = "gui")]
struct HlsState {
    /// Tempdir compartido. Todos los segmentos `seg-NNNNN.ts` viven
    /// aquí, producidos por cualquier job durante la vida del
    /// stream.
    dir: PathBuf,
    _tempdir: tempfile::TempDir,
    /// Job ffmpeg activo, si lo hay. `None` cuando no hay ninguna
    /// transcodificación en curso (todos los segmentos pedidos
    /// están ya en disco).
    job: Option<HlsJob>,
    /// Índice de stream de audio del INPUT que ffmpeg mapea a la
    /// salida. `None` = ffmpeg auto-selecciona (0:a:0 por defecto).
    /// Cuando el user cambia de pista vía `POST /hls/audio`, matamos
    /// el job activo, purgamos segmentos y guardamos aquí la nueva
    /// selección; el próximo respawn usa `-map 0:v:0 -map 0:a:<idx>`.
    audio_idx: Option<usize>,
    /// Estrategia decidida al init: Copy (remux -c:v copy, cero
    /// pérdida) o Transcode (libx264 CRF 18). Audit §2/§7. La
    /// decisión mira el probe + client caps + preferences y se
    /// congela para toda la vida del stream — un cambio de
    /// preferencia NO afecta a un stream ya arrancado.
    mode: HlsMode,
    /// Rejilla de segmentos: para cada idx, `(start_seconds,
    /// duration_seconds)`. En modo Transcode todos duran
    /// `HLS_SEG_SECS`; en modo Copy la rejilla es variable y
    /// viene del `KeyframeIndex.variable_segments()` — los cortes
    /// caen en keyframes reales del archivo (audit §2b).
    segments: Vec<(f64, f64)>,
    /// Último idx pedido por `serve_hls_segment`. La tarea de
    /// eviction LRU lo usa como playhead para decidir qué
    /// segmentos son "lejanos" y candidatos a borrar (audit §6).
    /// Inicializa a 0 (arranque) y avanza monótono con seek
    /// forward + oscila con scrubbing. Cero coste de sincronía
    /// (atomic).
    last_requested_idx: Arc<AtomicU64>,
    /// Handle a la tarea de eviction para poder abortarla al drop
    /// del stream. `None` si el budget es 0 (evicción desactivada).
    _evictor: Option<tokio::task::JoinHandle<()>>,
    /// Sticky failure: si algún spawn de ffmpeg murió en <500ms
    /// con exit code != 0, guardamos aquí el mensaje del último
    /// error. Todos los `serve_hls_segment` siguientes devuelven
    /// 500 con ese mensaje SIN respawnear ffmpeg, hasta que el
    /// user cierre el player. Necesario para no entrar en loop
    /// infinito cuando el argv es inválido (filter missing,
    /// codec sin soporte, etc.).
    fatal_error: Option<String>,
}

#[cfg(feature = "gui")]
impl Drop for HlsState {
    fn drop(&mut self) {
        // Aborta la tarea de eviction. Sin esto, el loop seguiría
        // corriendo tras cerrar el player (el `dir` que escanea
        // desaparece con `_tempdir`, así que fallaría en silencio,
        // pero es limpio abortarlo). El tempdir + cualquier ffmpeg
        // hijo se limpian por su propio Drop (`kill_on_drop`).
        if let Some(h) = self._evictor.take() {
            h.abort();
        }
    }
}

/// Estrategia de encoding decidida por `decide_hls_mode` al init.
#[cfg(feature = "gui")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HlsMode {
    /// `-c:v copy`: sin pérdida en vídeo. Los cortes de segmento
    /// caen en keyframes REALES del archivo (rejilla de
    /// `HlsState.segments` construida desde el `KeyframeIndex`).
    Copy,
    /// `-c:v libx264 -crf 18 -preset veryfast`: transcode con
    /// preset de calidad alta (audit §5). Los cortes de segmento
    /// caen en múltiplos exactos de `HLS_SEG_SECS` porque el
    /// encoder fuerza keyframes ahí (`-force_key_frames`).
    Transcode,
}

/// Job ffmpeg activo: proceso corriendo con `-ss <idx*4>` +
/// `-start_number <idx>` + `-output_ts_offset <idx*4>`, produciendo
/// segmentos `seg-<idx>.ts`, `seg-<idx+1>.ts`, … con timestamps
/// absolutos (PTS = tiempo real de la peli). `kill_on_drop = true`
/// es un safety net; al reemplazar job llamamos
/// `child.kill().await` + `child.wait().await` explícitamente para
/// que el ffmpeg viejo suelte su Range GET contra `/video` antes
/// de que el nuevo pida bytes — si no, ambos compiten por librqbit
/// y ninguno avanza.
#[cfg(feature = "gui")]
struct HlsJob {
    child: tokio::process::Child,
    /// Primer índice de segmento que produce este job. Los ficheros
    /// que emite son `seg-<start_idx>.ts`, `seg-<start_idx+1>.ts`,
    /// etc. Se compara con el idx pedido en cada request para
    /// decidir si el job actual puede servirlo (dentro de la
    /// ventana) o hay que reiniciar en el idx pedido.
    start_idx: u64,
}

/// Duración fija de cada segmento HLS, en segundos. Debe coincidir
/// con `-hls_time` y con `-force_key_frames expr:gte(t,n_forced*4)`
/// del spawn de ffmpeg — el conjunto es lo que garantiza que dos
/// jobs distintos produzcan segmentos intercambiables en las
/// mismas fronteras temporales.
#[cfg(feature = "gui")]
const HLS_SEG_SECS: f64 = 4.0;

/// Cuántos segmentos por delante del último producido tolera el job
/// activo antes de considerar la petición un seek hacia adelante y
/// reiniciar en el idx pedido. `6 × 4s = 24s` de headroom: un
/// scrubbing rápido dentro de esa ventana espera al job actual (ya
/// está transcodeando cerca), un salto mayor respawnea.
#[cfg(feature = "gui")]
const HLS_LOOKAHEAD: u64 = 6;

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

    let session = Session::new_with_opts(
        data_dir.clone(),
        SessionOptions {
            // No queremos que la sesión reutilice puertos DHT/estado entre
            // arranques — cada stream es efímero.
            disable_dht_persistence: true,
            // Tampoco queremos que persista la lista de torrents.
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

/// Handler HTTP. Soporta `Range: bytes=X-Y` (200/206). Sin Range devuelve
/// el fichero entero como 200 OK.
async fn serve_video(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range);

    // Rango vacío: fichero de tamaño cero — nada que servir.
    if state.file_len == 0 {
        return Err((
            StatusCode::RANGE_NOT_SATISFIABLE,
            "Fichero vac\u{ed}o".to_string(),
        ));
    }

    let (start, end) = match range {
        Some((Some(s), Some(e))) => {
            // Rango con start y end explícitos. Rechaza `bytes=5-3`.
            if e < s {
                return Err((
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    format!("Rango malformado: {s}-{e}"),
                ));
            }
            (s, e.min(state.file_len - 1))
        }
        Some((Some(s), None)) => (s, state.file_len - 1),
        Some((None, Some(suffix))) => {
            // Suffix range (`bytes=-500`): los últimos N bytes del fichero.
            // Algunos players lo usan para leer el índice al final del MP4.
            let n = suffix.min(state.file_len);
            (state.file_len - n, state.file_len - 1)
        }
        // `parse_range` rechaza el caso ambos-None (`bytes=-`) hoy, pero
        // no queremos panicar en producción si alguien relaja esa
        // validación sin actualizar este site. `debug_assert!` casca en
        // tests y builds de dev; en release caemos a servir el fichero
        // completo, que es la interpretación más conservadora del rango
        // "todo".
        Some((None, None)) => {
            debug_assert!(false, "parse_range should reject both-None ranges");
            (0, state.file_len - 1)
        }
        None => (0, state.file_len - 1),
    };

    if start >= state.file_len {
        return Err((
            StatusCode::RANGE_NOT_SATISFIABLE,
            format!("Range {start} >= {}", state.file_len),
        ));
    }

    // Trackear la posición de reproducción SOLO para Ranges con start
    // explícito. Los suffix ranges (`bytes=-N`) los usa VLC para leer el
    // índice al final del MP4 y no reflejan la playhead — si los
    // usáramos, `max_seek` saltaría al 99% al abrir cualquier peli.
    let is_explicit_start = matches!(range, Some((Some(_), _)));
    if is_explicit_start {
        state.max_seek.fetch_max(start, Ordering::Relaxed);
    }

    let content_length = end - start + 1;
    // Asigna un id monótono a esta request. Se usa como campo `req`
    // en TODOS los logs de `/video` para poder correlacionar (a) qué
    // request cancela a qué otra, y (b) cuántos bytes llegó a
    // entregar cada una antes de morir vs. cerrarse por EOF.
    let req_id = state.request_counter.fetch_add(1, Ordering::Relaxed);
    let range_desc = match range {
        Some((Some(s), Some(e))) => format!("{s}-{e}"),
        Some((Some(s), None)) => format!("{s}-"),
        Some((None, Some(n))) => format!("-{n}"),
        _ => "full".to_string(),
    };
    tracing::info!(
        target: "video",
        req = req_id,
        range = %range_desc,
        start,
        end,
        bytes = content_length,
        pct = format!("{:.1}", (start as f64 / state.file_len as f64) * 100.0),
        "range in"
    );

    // Cancela la petición HTTP anterior antes de arrancar la nueva. Así
    // el FileStream viejo se dropea y librqbit deja de repartir ancho de
    // banda con él — véase el comentario de `active_request` en `AppState`.
    //
    // Dos excepciones al cancel:
    //
    //   * `is_suffix_range` (`bytes=-N`): WKWebView los usa para leer
    //     el moov al final del MP4. No son la playhead y no se
    //     comparan con VLC/ffmpeg-HLS — no cancelamos por ellos ni les
    //     cancelamos a nadie.
    //
    //   * `burst_window`: en modo DIRECT, WKWebView emite un
    //     start-range para el moov y otro para los datos casi al
    //     mismo tiempo (dentro de ~30-80ms). Cancelar la request
    //     previa provocaría re-intentos y stalls. Si la request activa
    //     arrancó hace <BURST_WINDOW_MS, asumimos que es del mismo
    //     burst y coexistimos. Los seeks reales de VLC/ffmpeg vienen
    //     con segundos entre medias, muy por encima del umbral.
    const BURST_WINDOW_MS: u128 = 150;
    let is_suffix_range = matches!(range, Some((None, Some(_))));
    let request_token = CancellationToken::new();
    if !is_suffix_range {
        let mut guard = state.active_request.lock().await;
        let now = tokio::time::Instant::now();
        let decision: &'static str;
        let mut cancelled_prev: Option<u64> = None;
        let should_cancel_prev = guard
            .as_ref()
            .map(|(_, _, started)| started.elapsed().as_millis() >= BURST_WINDOW_MS)
            .unwrap_or(false);
        if should_cancel_prev {
            if let Some((prev_id, prev, _)) = guard.replace((req_id, request_token.clone(), now)) {
                prev.cancel();
                cancelled_prev = Some(prev_id);
                decision = "cancelled_prev";
            } else {
                decision = "slot_empty";
            }
        } else if guard.is_some() {
            // Coexistimos con el burst. Sobrescribimos el slot con el
            // nuestro para que la SIGUIENTE cancele a esta si llega
            // después del burst window.
            *guard = Some((req_id, request_token.clone(), now));
            decision = "coexist_burst";
        } else {
            *guard = Some((req_id, request_token.clone(), now));
            decision = "slot_empty";
        }
        tracing::info!(
            target: "video",
            req = req_id,
            decision,
            cancelled_prev,
            "active_request"
        );
    } else {
        tracing::info!(
            target: "video",
            req = req_id,
            decision = "suffix_skip",
            "active_request"
        );
    }

    // Crea un stream nuevo por request (librqbit gestiona la prioridad de
    // piezas por stream registrado).
    let mut file_stream = state
        .handle
        .clone()
        .stream(state.file_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if start > 0 {
        file_stream
            .seek(SeekFrom::Start(start))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    // Convierte AsyncRead en un Stream<Item=Bytes> con límite y con
    // corte al cancelar el token de esta request. `take_until` deja de
    // yield-ear en cuanto la petición siguiente sobrescriba el token.
    let limited = LimitedRead {
        inner: file_stream,
        remaining: content_length,
    };
    let raw = tokio_util::io::ReaderStream::with_capacity(limited, 64 * 1024);
    let cancel_fut = async move { request_token.cancelled().await };
    let cut = futures::stream::StreamExt::take_until(raw, Box::pin(cancel_fut));
    // Instrumentación: envolvemos el stream para contar bytes
    // entregados y loguear una línea al final que distingue
    // "fin natural (EOF)" de "cancelado por otra request". El log
    // es el emparejamiento del `range in` de arriba: sin él no se
    // puede reconstruir del debug.log si una request colgada llegó
    // a entregar algo o murió en seco.
    let stream = TracedResponseStream::new(cut, req_id, content_length);
    let body = Body::from_stream(stream);

    let status = if range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };

    let mut resp = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "video/mp4") // best-effort; VLC autodetecta
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, content_length.to_string());

    if range.is_some() {
        resp = resp.header(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{}", state.file_len))
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
    }

    resp.body(body)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Wrapper AsyncRead que limita el número de bytes a leer (para respetar
/// el `end` del Range).
struct LimitedRead<R> {
    inner: R,
    remaining: u64,
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for LimitedRead<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.remaining == 0 {
            return std::task::Poll::Ready(Ok(()));
        }
        let max = (self.remaining as usize).min(buf.remaining());
        let mut limited = buf.take(max);
        let before = limited.filled().len();
        let poll = std::pin::Pin::new(&mut self.inner).poll_read(cx, &mut limited);
        let read = limited.filled().len() - before;
        // SAFETY: bytes escritos en `limited` también están en `buf` porque
        // `buf.take()` comparte el buffer.
        unsafe {
            buf.assume_init(read);
        }
        buf.advance(read);
        self.remaining -= read as u64;
        poll
    }
}

/// Parsea `Range: bytes=START-END`, `bytes=START-` o `bytes=-SUFFIX`.
/// Devuelve `(Option<start>, Option<end>)`: si `start` es `None` se
/// trata como suffix range (los últimos N bytes). Solo se soporta UN
/// rango — los multipart se rechazan por caller.
fn parse_range(v: &str) -> Option<(Option<u64>, Option<u64>)> {
    let rest = v.strip_prefix("bytes=")?;
    let (start, end) = rest.split_once('-')?;
    let start = start.trim();
    let end = end.trim();
    let start_val: Option<u64> = if start.is_empty() {
        None
    } else {
        Some(start.parse().ok()?)
    };
    let end_val: Option<u64> = if end.is_empty() {
        None
    } else {
        Some(end.parse().ok()?)
    };
    // Al menos uno de los dos debe estar presente.
    if start_val.is_none() && end_val.is_none() {
        return None;
    }
    Some((start_val, end_val))
}

/// Wrapper de stream de respuesta que cuenta bytes entregados y loguea
/// UNA línea al final: `done` (EOF natural, alcanzó `content_length`)
/// o `cancelled` (`take_until` cortó por token o el cliente cerró la
/// conexión).
///
/// Instrumentación del audit: sin esto no se puede saber, del
/// `debug.log`, si una request `/video` que quedó colgada llegó a
/// entregar algo antes de morir. Empareja con el `range in` que emite
/// `serve_video` al entrar.
struct TracedResponseStream<S> {
    inner: S,
    req_id: u64,
    delivered: u64,
    expected: u64,
    finished: bool,
}

impl<S> TracedResponseStream<S> {
    fn new(inner: S, req_id: u64, expected: u64) -> Self {
        Self {
            inner,
            req_id,
            delivered: 0,
            expected,
            finished: false,
        }
    }
}

impl<S, E> futures::stream::Stream for TracedResponseStream<S>
where
    S: futures::stream::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
{
    type Item = Result<bytes::Bytes, E>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let poll = std::pin::Pin::new(&mut self.inner).poll_next(cx);
        if let std::task::Poll::Ready(ref item) = poll {
            match item {
                Some(Ok(b)) => {
                    self.delivered += b.len() as u64;
                }
                Some(Err(_)) => {
                    // Error del stream (IO, etc.). Se loguea en Drop
                    // como cancelled — no distinguimos IO error de
                    // cancelación aquí, la firma en el log es la misma
                    // "no llegó a servir todo".
                }
                None => {
                    self.finished = true;
                    let complete = self.delivered >= self.expected;
                    tracing::info!(
                        target: "video",
                        req = self.req_id,
                        bytes = self.delivered,
                        expected = self.expected,
                        outcome = if complete { "eof" } else { "eof_short" },
                        "request done"
                    );
                }
            }
        }
        poll
    }
}

impl<S> Drop for TracedResponseStream<S> {
    fn drop(&mut self) {
        if !self.finished {
            // Se dropea sin haber emitido `Ready(None)`: el stream fue
            // cortado por `take_until` (cancelación de request) o el
            // cliente cerró la conexión antes del EOF. Esta es la firma
            // del bug del audit: request que se queda colgada sin haber
            // llegado al final.
            tracing::info!(
                target: "video",
                req = self.req_id,
                bytes = self.delivered,
                expected = self.expected,
                outcome = "cancelled",
                "request done"
            );
        }
    }
}

// ── HLS: playlist estático + segmentos on-demand ──────────────────────────
//
// Modelo "VOD virtual" al estilo Stremio hlsv2 / Jellyfin / Plex:
//
//   * `/hls/playlist.m3u8` es una función pura de la duración de la
//     peli (probe cacheado). Enumera TODOS los segmentos desde
//     arranque (`seg-00000.ts`, `seg-00001.ts`, …, `seg-<n-1>.ts`)
//     con `#EXT-X-ENDLIST`. Safari lo trata como VOD puro: barra
//     de progreso completa desde el primer ms y seek nativo a
//     cualquier punto sin tocar `<video src>`.
//
//   * `/hls/seg-NNNNN.ts` los materializa ffmpeg BAJO DEMANDA. El
//     handler consulta la caché en disco (tempdir compartido por
//     todo el stream); si el segmento existe, se sirve; si no,
//     decide si el job ffmpeg activo puede producirlo pronto
//     (dentro de la ventana `[start_idx, produced_max + LOOKAHEAD]`)
//     o si hay que reiniciar ffmpeg en el idx pedido (seek fuera
//     de ventana).
//
//   * Cada job arranca con `-ss <idx*4>` + `-start_number <idx>` +
//     `-output_ts_offset <idx*4>`. La combinación garantiza que
//     los ficheros producidos se numeran desde el índice global
//     correcto Y que los PTS del MPEG-TS son tiempos absolutos de
//     la peli — sin esto, `currentTime`, subtítulos y timeline
//     quedarían desplazados tras cada reinicio de ffmpeg.
//
//   * `-hls_flags temp_file` hace que ffmpeg escriba primero
//     `seg-NNNNN.ts.tmp` y renombre a `.ts` al cerrar. Así "el
//     fichero .ts existe" ⇒ "está completo": el handler sirve sin
//     heurísticas de tamaño/mtime.
//
// Ventajas vs. el modelo anterior (playlist crece conforme ffmpeg
// produce, con `?start=<t>` que reemplazaba sesión y reasignaba
// `<video src>` en cada seek grande):
//
//   - Seek grande = spinner nativo + reproducción arranca en
//     cuanto llega el primer segmento del nuevo job. No más
//     504/timeout ni `MediaError code 4` de WKWebView por
//     reasignación de src.
//   - Seek hacia atrás a zona ya vista = instantáneo (segmentos
//     cacheados en disco durante toda la vida del stream).
//   - Subtítulos y `currentTime` siempre sincronizados con el
//     contenido: los PTS del TS son tiempo absoluto, no relativo
//     al último `-ss`.

#[cfg(feature = "gui")]
async fn serve_hls_playlist(
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, String)> {
    if !crate::ffmpeg::is_available() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "ffmpeg no est\u{e1} en PATH".to_string(),
        ));
    }
    // Garantiza HlsState (probe + modo + rejilla de segmentos ya
    // decididos, congelados para toda la vida del stream). Es
    // idempotente y thread-safe: la primera llamada paga probe +
    // keyframe index; las siguientes son un lock check.
    ensure_hls_dir(&state).await?;
    let (segments, mode) = {
        let guard = state.hls.lock().await;
        let hls = guard.as_ref().expect("ensure_hls_dir just populated");
        (hls.segments.clone(), hls.mode)
    };
    if segments.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "rejilla de segmentos vacía".to_string(),
        ));
    }
    // TARGETDURATION = ceil del segmento más largo (spec HLS). En
    // modo Copy con GOPs irregulares puede ser mayor que
    // HLS_SEG_SECS; en Transcode es HLS_SEG_SECS exacto.
    let target_duration = segments
        .iter()
        .map(|(_, d)| d.ceil() as u64)
        .max()
        .unwrap_or_else(|| HLS_SEG_SECS.ceil() as u64);
    let mut playlist = String::with_capacity(96 + segments.len() * 32);
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:3\n");
    playlist.push_str(&format!("#EXT-X-TARGETDURATION:{target_duration}\n"));
    playlist.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    playlist.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    playlist.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
    for (i, (_start, dur)) in segments.iter().enumerate() {
        // EXTINF con precisión al ms — Safari/hls.js son estrictos
        // con truncados que superen la duración real.
        playlist.push_str(&format!("#EXTINF:{dur:.5},\nseg-{i:05}.ts\n"));
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    tracing::debug!(
        target: "hls",
        mode = ?mode,
        segments = segments.len(),
        target_duration,
        "playlist emitted"
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        // Estable durante la vida del stream (mismo modo/segments ⇒
        // mismo playlist). Dejamos que el WebView lo cachee.
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(Body::from(playlist.into_bytes()))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Parsea `seg-NNNNN.ts` → `NNNNN` como u64. `None` si el nombre no
/// respeta el formato exacto (validación fuerte, path traversal-safe).
#[cfg(feature = "gui")]
fn parse_seg_idx(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("seg-")?;
    let idx = rest.strip_suffix(".ts")?;
    if idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    idx.parse().ok()
}

/// Whitelist para `/hls/{file}`. Solo acepta `seg-<digits>.ts` con
/// nombre de longitud sensata. Rechaza separadores (`/` y `\` — este
/// último es válido en Windows y `dir.join()` lo interpretaría como
/// sub-path), `..`, NUL y cualquier char no numérico. `playlist.m3u8`
/// no entra aquí: se sirve en una ruta separada registrada antes.
#[cfg(feature = "gui")]
fn is_valid_hls_filename(name: &str) -> bool {
    parse_seg_idx(name).is_some() && name.len() <= 32
}

/// Escanea el tempdir compartido buscando el máximo idx de segmento
/// ya producido por el job activo (idx >= `floor`, que es
/// `job.start_idx`). Si aún no hay ninguno producido devuelve
/// `floor - 1` — de forma que el chequeo `idx > produced + LOOKAHEAD`
/// solo dispare restart cuando el idx pedido está muy por delante,
/// no por defecto.
///
/// Sync `std::fs::read_dir` a propósito: los tempdirs de HLS tienen
/// pocos miles de entradas y la operación es de <5ms típico; evita
/// la maquinaria async y el context switch. Solo se llama al decidir
/// si spawnear un job — no en el fast path (fichero existe).
#[cfg(feature = "gui")]
fn max_produced_idx(dir: &Path, floor: u64) -> u64 {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return floor.saturating_sub(1),
    };
    let mut max: Option<u64> = None;
    for entry in entries.flatten() {
        let name_os = entry.file_name();
        let name = match name_os.to_str() {
            Some(s) => s,
            None => continue,
        };
        if let Some(idx) = parse_seg_idx(name) {
            if idx >= floor && max.map(|m| idx > m).unwrap_or(true) {
                max = Some(idx);
            }
        }
    }
    match max {
        Some(m) => m,
        None => floor.saturating_sub(1),
    }
}

/// Garantiza que existe el tempdir compartido del stream HLS Y la
/// rejilla de segmentos + modo decididos. Se crea perezosamente en
/// la primera petición HLS (playlist o segmento); sobrevive a
/// reinicios de ffmpeg (todos los jobs del stream escriben aquí,
/// los segmentos son cache para toda la vida del stream).
///
/// Al ser el primer punto donde tenemos probe + client caps +
/// preferencias, aquí es donde se decide `HlsMode`. La decisión se
/// congela para toda la vida del stream — un cambio de preferencia
/// mientras se está reproduciendo NO afecta al stream en curso
/// (ver `HlsState.mode`).
#[cfg(feature = "gui")]
async fn ensure_hls_dir(state: &AppState) -> Result<PathBuf, (StatusCode, String)> {
    {
        let guard = state.hls.lock().await;
        if let Some(hls) = guard.as_ref() {
            return Ok(hls.dir.clone());
        }
    }
    // Probe primero (fuera del lock — puede tardar 1-3s con Range
    // requests). Necesario para conocer duración, container y códecs.
    let info = ensure_probe(state)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, format!("probe: {e}")))?;
    let duration = info.duration_seconds.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "duración desconocida (probe sin moov accesible)".to_string(),
    ))?;
    if duration <= 0.0 {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!("duración inválida ({duration}s)"),
        ));
    }
    let prefs = crate::preferences::load();
    let caps = current_client_capabilities();
    let url = format!("http://{}/video", state.local_addr);

    let (mode, segments) = decide_mode_and_segments(&info, &caps, prefs.quality_mode, &url).await;

    if segments.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "peli demasiado corta para HLS".to_string(),
        ));
    }

    let tempdir = tempfile::Builder::new()
        .prefix("videodrome-hls-")
        .tempdir()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tempdir: {e}")))?;
    let dir = tempdir.path().to_path_buf();
    tracing::info!(
        target: "hls",
        mode = ?mode,
        segments = segments.len(),
        duration_s = format!("{duration:.2}"),
        dir = %dir.display(),
        "init"
    );
    let mut guard = state.hls.lock().await;
    // Doble check: si otra request lo llenó mientras estábamos
    // haciendo probe/keyframes, respetamos ese estado.
    if let Some(hls) = guard.as_ref() {
        return Ok(hls.dir.clone());
    }
    let last_requested_idx = Arc::new(AtomicU64::new(0));
    // Evictor LRU (audit §6): spawnea una tarea que barre el dir
    // cada 10s y borra segmentos alejados del playhead cuando el
    // total pisa el budget. Deshabilitado si el user pone 0.
    let evictor = if prefs.hls_disk_budget_gb > 0 {
        let budget_bytes: u64 = (prefs.hls_disk_budget_gb as u64) * 1024 * 1024 * 1024;
        Some(spawn_lru_evictor(
            dir.clone(),
            budget_bytes,
            last_requested_idx.clone(),
        ))
    } else {
        None
    };
    *guard = Some(HlsState {
        dir: dir.clone(),
        _tempdir: tempdir,
        job: None,
        audio_idx: None,
        mode,
        segments,
        last_requested_idx,
        _evictor: evictor,
        fatal_error: None,
    });
    Ok(dir)
}

/// Decide `HlsMode` + construye la rejilla de segmentos para el
/// playlist. Audit §2/§7:
///
///   * Preferencia `Transcode` → siempre transcode con rejilla fija.
///   * Preferencia `Copy` → intentar copy; si falla, ERROR (el user
///     lo pidió expresamente, no cambiamos de modo bajo sus pies).
///   * Preferencia `Auto` (default): copy si (1) el códec de vídeo
///     es compatible con el cliente vía DIRECT-eligible codec set,
///     (2) el `KeyframeIndex` se puede leer, (3) el max GOP ≤ 10s.
///     Si algo falla → transcode.
#[cfg(feature = "gui")]
async fn decide_mode_and_segments(
    info: &crate::ffmpeg::MediaInfo,
    caps: &crate::ffmpeg::ClientCapabilities,
    pref: crate::preferences::QualityMode,
    url: &str,
) -> (HlsMode, Vec<(f64, f64)>) {
    use crate::preferences::QualityMode;
    let duration = info.duration_seconds.unwrap_or(0.0);
    let transcode_grid = build_transcode_grid(duration);

    match pref {
        QualityMode::Transcode => (HlsMode::Transcode, transcode_grid),
        QualityMode::Copy => match try_build_copy_grid(info, caps, url).await {
            Ok(grid) if !grid.is_empty() => (HlsMode::Copy, grid),
            Ok(_) => {
                tracing::info!(target: "hls", "pref=Copy pero grid vacía → fallback transcode");
                (HlsMode::Transcode, transcode_grid)
            }
            Err(e) => {
                tracing::info!(target: "hls", error = %e, "pref=Copy falló → fallback transcode");
                (HlsMode::Transcode, transcode_grid)
            }
        },
        QualityMode::Auto => match try_build_copy_grid(info, caps, url).await {
            Ok(grid) if !grid.is_empty() => {
                tracing::info!(target: "hls", segments = grid.len(), "auto → COPY viable");
                (HlsMode::Copy, grid)
            }
            Ok(_) => {
                tracing::info!(target: "hls", "auto → grid vacía, transcode");
                (HlsMode::Transcode, transcode_grid)
            }
            Err(e) => {
                tracing::info!(target: "hls", error = %e, "auto → copy no viable, transcode");
                (HlsMode::Transcode, transcode_grid)
            }
        },
    }
}

/// Construye la rejilla fija de segmentos de `HLS_SEG_SECS`. El
/// último puede ser más corto para no exceder la duración total.
#[cfg(feature = "gui")]
fn build_transcode_grid(duration: f64) -> Vec<(f64, f64)> {
    if duration <= 0.0 {
        return Vec::new();
    }
    let n = (duration / HLS_SEG_SECS).ceil() as usize;
    (0..n)
        .map(|i| {
            let start = i as f64 * HLS_SEG_SECS;
            let len = if i + 1 == n {
                (duration - start).max(0.001)
            } else {
                HLS_SEG_SECS
            };
            (start, len)
        })
        .collect()
}

/// `true` si el stream de vídeo declara transfer characteristics
/// HDR: SMPTE 2084 (PQ, típico en BluRay UHD) o ARIB STD-B67 (HLG,
/// broadcast). Audit §8.
#[cfg(feature = "gui")]
fn is_hdr_stream(video: &crate::ffmpeg::StreamInfo) -> bool {
    video
        .color_transfer
        .as_deref()
        .map(|t| {
            let t = t.to_ascii_lowercase();
            t.contains("smpte2084") || t.contains("arib-std-b67") || t.contains("bt2020-10")
        })
        .unwrap_or(false)
}

/// Intenta construir la rejilla de segmentos para modo COPY:
/// fetchea el keyframe index del contenedor y agrupa keyframes en
/// segmentos ≥ `HLS_SEG_SECS`. Devuelve error si el códec no es
/// compatible con el cliente, si el índice no se puede leer, o si
/// el max GOP > 10s (audit §2d — con GOPs enormes el seek en copy
/// sería inaceptable).
#[cfg(feature = "gui")]
async fn try_build_copy_grid(
    info: &crate::ffmpeg::MediaInfo,
    caps: &crate::ffmpeg::ClientCapabilities,
    url: &str,
) -> anyhow::Result<Vec<(f64, f64)>> {
    use anyhow::bail;
    let video = info
        .streams
        .iter()
        .find(|s| s.kind == crate::ffmpeg::StreamKind::Video)
        .ok_or_else(|| anyhow::anyhow!("sin stream de vídeo"))?;
    // Códec debe ser algo que el cliente pueda reproducir vía TS
    // sin transcode. H.264 universal; HEVC 8-bit solo si el cliente
    // declara `hevc`; HEVC 10-bit necesita `hevc10` Y salir de HDR
    // (dejado a §6/§8 futuros).
    let codec_ok = match video.codec.as_str() {
        "h264" => caps.supports("h264"),
        "hevc" | "h265" => {
            let is_10bit = video
                .pix_fmt
                .as_deref()
                .map(|p| {
                    let p = p.to_ascii_lowercase();
                    p.contains("10le") || p.contains("10be")
                })
                .unwrap_or(false);
            if is_10bit {
                caps.supports("hevc10")
            } else {
                caps.supports("hevc")
            }
        }
        _ => false,
    };
    if !codec_ok {
        bail!(
            "cliente no soporta '{}' vía TS copy (pix_fmt={:?})",
            video.codec,
            video.pix_fmt
        );
    }
    // Audit §8: HDR (SMPTE 2084 / arib-std-b67) es incompatible
    // con TS-copy incluso si el cliente soporta HEVC 10-bit. La
    // ausencia de tone-map + metadata deja los colores lavados en
    // pantallas SDR. Bailamos → el caller cae a transcode con la
    // cadena zscale+tonemap.
    if is_hdr_stream(video) {
        bail!(
            "HDR (color_transfer={:?}) → transcode+tonemap",
            video.color_transfer
        );
    }
    // Fetch keyframe index. Cliente HTTP reutilizable: creamos uno
    // simple aquí (localhost, sin cookies ni auth).
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_default();
    let idx =
        crate::keyframes::fetch_keyframe_index(&client, url, info.container.as_deref()).await?;
    let max_gap = idx.max_gap_seconds();
    const MAX_GOP_SECONDS: f64 = 10.0;
    if max_gap > MAX_GOP_SECONDS {
        bail!("GOP máximo {max_gap:.1}s > {MAX_GOP_SECONDS}s (seek en copy sería inaceptable)");
    }
    Ok(idx.variable_segments(HLS_SEG_SECS))
}

#[cfg(feature = "gui")]
async fn serve_hls_segment(
    State(state): State<AppState>,
    axum::extract::Path(file): axum::extract::Path<String>,
) -> Result<Response, (StatusCode, String)> {
    if !is_valid_hls_filename(&file) {
        return Err((StatusCode::BAD_REQUEST, "path inv\u{e1}lido".to_string()));
    }
    let idx =
        parse_seg_idx(&file).ok_or((StatusCode::BAD_REQUEST, "idx inv\u{e1}lido".to_string()))?;
    let dir = ensure_hls_dir(&state).await?;
    let path = dir.join(&file);

    // Trackear playhead para el evictor LRU (audit §6): cada
    // request pinta la posición actual del cliente. El evictor
    // usa este valor para decidir qué segmentos son "lejanos" y
    // por tanto candidatos a borrar. Usamos `store` (no fetch_max):
    // si el user hace scrubbing hacia atrás, el playhead debe
    // reflejar la posición REAL, aunque implique evictar
    // segmentos cercanos al highwatermark previo (esos son ahora
    // los "lejanos"; los podemos re-materializar bajo demanda).
    //
    // ALSO: chequear si hay un fatal_error registrado (spawn
    // repetidamente muerto) → cortar el loop y devolver 500 al
    // cliente. Sin esto, cualquier fallo persistente de ffmpeg
    // (filter missing, codec sin soporte, PATH roto) provoca
    // respawn cada 150ms hasta cerrar el player.
    {
        let guard = state.hls.lock().await;
        if let Some(hls) = guard.as_ref() {
            hls.last_requested_idx.store(idx, Ordering::Relaxed);
            if let Some(msg) = &hls.fatal_error {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("HLS pipeline fatal: {msg}"),
                ));
            }
        }
    }

    // Deadline generoso: el spawn en frío de un job en offset alto
    // puede tardar decenas de segundos (librqbit tiene que bajar las
    // piezas correspondientes al `-ss` con peers regulares). 60s
    // deja margen sin dejar al user esperando eternamente.
    let started_at = tokio::time::Instant::now();
    let deadline = started_at + std::time::Duration::from_secs(60);
    let mut logged_wait = false;
    loop {
        // Fast path: el fichero .ts existe. Con `-hls_flags temp_file`,
        // existir ⇒ estar cerrado y completo (ffmpeg escribió en .tmp
        // y renombró al terminar). Nunca servimos escritura en curso.
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            break;
        }

        // Decidir si hay que spawnear un job (o reiniciar el actual)
        // para producir este idx. Snapshot rápido del estado bajo lock;
        // decisión y respawn fuera del lock.
        enum Action {
            Spawn,
            Wait,
        }
        let action = {
            let mut guard = state.hls.lock().await;
            let hls = guard.as_mut().expect("dir ensured above");
            let dir_ref = hls.dir.clone();
            match hls.job.as_mut() {
                None => Action::Spawn,
                Some(job) => {
                    // ffmpeg vivo?
                    let alive = matches!(job.child.try_wait(), Ok(None));
                    if !alive {
                        Action::Spawn
                    } else if idx < job.start_idx {
                        // Seek hacia atrás fuera de la ventana del
                        // job. Como el fichero no existe aún, o bien
                        // nunca se produjo en esta sesión o el user
                        // borró el tempdir por debajo — en cualquier
                        // caso, reiniciar en idx pedido.
                        Action::Spawn
                    } else {
                        let produced = max_produced_idx(&dir_ref, job.start_idx);
                        if idx > produced.saturating_add(HLS_LOOKAHEAD) {
                            // Seek hacia adelante muy lejos del último
                            // producido: reiniciar en idx pedido.
                            Action::Spawn
                        } else {
                            Action::Wait
                        }
                    }
                }
            }
        };
        if matches!(action, Action::Spawn) {
            ensure_hls_job(&state, idx).await?;
        }

        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                target: "hls",
                file = %file,
                idx,
                "TIMEOUT: segmento no disponible tras 60s"
            );
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                format!("segmento {file} no disponible tras 60s"),
            ));
        }
        // Log de progreso una única vez tras 5s de espera para
        // detectar spawns lentos sin ensuciar la consola en el caso
        // rápido.
        if !logged_wait && started_at.elapsed().as_secs() >= 5 {
            tracing::info!(
                target: "hls",
                idx,
                elapsed_s = started_at.elapsed().as_secs(),
                "waiting for segment"
            );
            logged_wait = true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("read {file}: {e}"),
        )
    })?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp2t")
        // Cada .ts es contenido determinista para toda la vida del
        // stream (mismo idx ⇒ mismo rango temporal). Cachear reduce
        // re-fetches de Safari en scrubbing.
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Reinicia el job ffmpeg activo (si lo hay) y arranca uno nuevo
/// que empiece a producir desde `idx` inclusive. El job viejo se
/// mata SÍNCRONAMENTE (`kill().await` + `wait().await`) antes de
/// spawnear el nuevo — sin esto, ambos ffmpegs pedirían bytes de
/// `/video` a la vez y librqbit no serviría suficiente ancho de
/// banda al nuevo (dos consumidores concurrentes ⇒ ninguno avanza
/// rápido). Además cancelamos la `active_request` para que
/// librqbit libere el FileStream del viejo inmediatamente.
#[cfg(feature = "gui")]
async fn ensure_hls_job(state: &AppState, idx: u64) -> Result<(), (StatusCode, String)> {
    // Sacamos el job existente del guard con `.take()` para no
    // bloquear el mutex durante el kill (puede tardar decenas de ms).
    // Además copiamos el modo + start_seconds del segmento pedido
    // — la rejilla congelada al init es la fuente de verdad para el
    // tiempo absoluto en el que ffmpeg debe arrancar (audit §2b).
    let (old_job, dir, audio_idx, mode, start_seconds) = {
        let mut guard = state.hls.lock().await;
        let hls = guard
            .as_mut()
            .expect("dir must be ensured before ensure_hls_job");
        let start = hls
            .segments
            .get(idx as usize)
            .map(|(s, _)| *s)
            .unwrap_or_else(|| idx as f64 * HLS_SEG_SECS);
        (
            hls.job.take(),
            hls.dir.clone(),
            hls.audio_idx,
            hls.mode,
            start,
        )
    };
    if let Some(mut old) = old_job {
        // Cancelar la Range GET del ffmpeg viejo contra `/video`:
        // axum cierra el body → librqbit libera el FileStream → las
        // piezas priorizadas se liberan para el nuevo.
        {
            let mut req_guard = state.active_request.lock().await;
            if let Some((prev_id, token, _)) = req_guard.take() {
                token.cancel();
                tracing::info!(
                    target: "hls",
                    reason = "replaced",
                    cancelled_prev = prev_id,
                    "cancelling /video active_request before killing old ffmpeg"
                );
            }
        }
        let kill_started = tokio::time::Instant::now();
        let _ = old.child.kill().await;
        let _ = old.child.wait().await;
        tracing::info!(
            target: "hls",
            start_idx = old.start_idx,
            elapsed_ms = kill_started.elapsed().as_millis() as u64,
            reason = "replaced",
            "killed old ffmpeg job"
        );
    }

    // Warm-up de librqbit: si el idx pedido corresponde a un offset
    // alto y las piezas están frías, priorizamos su descarga ANTES
    // de que ffmpeg haga su primer read. Reduce el tiempo hasta el
    // primer segmento típico de 60s → 15-30s en pelis pesadas.
    if start_seconds > 5.0 {
        warmup_librqbit_for_offset(state, start_seconds).await;
    }

    let child = spawn_hls(state, &dir, idx, audio_idx, mode, start_seconds).await?;
    // Detección de fallo temprano: si el argv es inválido
    // (filter missing, codec sin soporte, PATH roto…) ffmpeg
    // muere en <100 ms con exit != 0. Sin este check el loop de
    // `serve_hls_segment` respawnearía indefinidamente cada
    // 150 ms. Damos 500 ms de gracia — un spawn "sano" tarda
    // decenas de ms en abrir el input pero no exita nunca; uno
    // "malo" muere casi al instante.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let mut child = child;
    match child.try_wait() {
        Ok(Some(status)) if !status.success() => {
            // Muerte inmediata con error. Marcamos el HlsState
            // como fatal para que las siguientes requests devuelvan
            // 500 sin respawnear.
            let msg = format!(
                "ffmpeg exited with {} in <500ms (probablemente filter/codec no soportado)",
                status
            );
            tracing::error!(target: "hls", error = %msg, "FATAL");
            let mut guard = state.hls.lock().await;
            if let Some(hls) = guard.as_mut() {
                hls.fatal_error = Some(msg.clone());
            }
            return Err((StatusCode::INTERNAL_SERVER_ERROR, msg));
        }
        Ok(Some(_)) => {
            // Salió con éxito antes de producir nada — raro,
            // dejar el flujo normal seguir (`serve_hls_segment`
            // hará timeout de 60s si no aparece el .ts).
        }
        Ok(None) | Err(_) => {
            // Sigue vivo → todo OK.
        }
    }
    let mut guard = state.hls.lock().await;
    let hls = guard.as_mut().expect("dir");
    hls.job = Some(HlsJob {
        child,
        start_idx: idx,
    });
    Ok(())
}

/// Fuerza a librqbit a priorizar las piezas del torrent que ffmpeg
/// va a necesitar para arrancar en `start_seconds`. Sin esto,
/// librqbit solo prioriza cuando ffmpeg hace la Range GET real —
/// pero para entonces ya llevamos segundos perdidos.
///
/// Estrategia: estimar el byte offset como `start_seconds * bytes/s`
/// (donde `bytes/s = file_len / duration`), abrir un stream de
/// librqbit, hacer seek al offset y leer 1 byte. La lectura fuerza
/// a librqbit a descargar la pieza correspondiente; al drop del
/// stream la prioridad se mantiene un rato (librqbit no la baja
/// instantáneamente cuando cierra un consumer).
///
/// Si no hay probe cacheado (no conocemos duration), no hacemos
/// warm-up — el primer segment quizás tarde más pero al menos no
/// hacemos daño.
#[cfg(feature = "gui")]
async fn warmup_librqbit_for_offset(state: &AppState, start_seconds: f64) {
    let duration = {
        let guard = state.cached_probe.lock().await;
        guard.as_ref().and_then(|p| p.duration_seconds)
    };
    let Some(duration) = duration else {
        tracing::info!(target: "warmup", "skip: no duration cached yet");
        return;
    };
    if duration <= 0.0 {
        return;
    }
    let byte_offset = ((start_seconds / duration) * state.file_len as f64) as u64;
    let byte_offset = byte_offset.min(state.file_len.saturating_sub(1));
    let started = tokio::time::Instant::now();
    tracing::info!(
        target: "warmup",
        byte_offset,
        pct = format!("{:.1}", (byte_offset as f64 / state.file_len as f64) * 100.0),
        start_seconds,
        "priming librqbit"
    );
    let mut file_stream = match state.handle.clone().stream(state.file_id) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(target: "warmup", error = %e, "librqbit stream failed");
            return;
        }
    };
    if let Err(e) = file_stream.seek(SeekFrom::Start(byte_offset)).await {
        tracing::warn!(target: "warmup", error = %e, "seek failed");
        return;
    }
    // Read 1 byte para señalar a librqbit "prioriza esta pieza YA".
    // Timeout defensivo: si tarda >3s, seguimos igualmente (ffmpeg lo
    // volverá a intentar, no dejamos al user esperando sin logs).
    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::io::AsyncReadExt::read(&mut file_stream, &mut buf),
    )
    .await;
    match read {
        Ok(Ok(n)) => tracing::info!(
            target: "warmup",
            bytes = n,
            byte_offset,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "primed"
        ),
        Ok(Err(e)) => tracing::warn!(target: "warmup", error = %e, "read err"),
        Err(_) => tracing::warn!(
            target: "warmup",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "read timeout at 3s (piezas frías, seguimos)"
        ),
    }
    // Al salir de la función, `file_stream` se dropea explícitamente.
    // Crítico para el bug del audit: si el warmup mantuviera el stream
    // vivo mientras ffprobe/ffmpeg piden otras piezas, la priorización
    // de librqbit se repartiría en dos consumidores. Loguearlo para
    // poder confirmar la hipótesis en el debug.log.
    drop(file_stream);
    tracing::info!(
        target: "warmup",
        elapsed_ms = started.elapsed().as_millis() as u64,
        "stream released"
    );
    // NB: NO tocamos `state.max_seek` aquí. Antes lo hacíamos
    // "para que la próxima Range GET real no resetee la prioridad",
    // pero `max_seek` NO influye en la priorización de piezas de
    // librqbit — solo se usa para persistir `resume.json` al drop.
    // Contaminarlo desde un warm-up estimado provocaba que un peek
    // al 90% dejara el resume ahí para siempre, o que el resume
    // avanzase sin que el usuario reprodujese realmente ese offset.
}

// ── LRU eviction de segmentos .ts (audit §6) ──────────────────
//
// Modelo COPY = disco crece con bitrate ORIGINAL: un remux UHD
// visto entero deja ~60 GB en el tempdir. La evicción por
// presupuesto es NECESARIA (no opcional) para no llenar disco.
//
// Estrategia: cada `EVICT_INTERVAL_SECS` sumamos tamaños de
// `seg-*.ts`; si el total supera `budget_bytes`, borramos los más
// alejados del `last_requested_idx` (playhead) hasta bajar a 90%
// del budget (10% de headroom para no evictar en cada ciclo).
//
// Safety window: nunca borramos idx en
// `[playhead-2, playhead+HLS_LOOKAHEAD+2]`. Ese margen cubre el
// segmento que se está reproduciendo, los ya buffered por el
// player (típ. 2-3 hacia adelante), y el que ffmpeg está
// produciendo justo ahora.
//
// Priorización: entre segmentos igual de lejanos, borramos primero
// los que están POR DETRÁS del playhead — "rewind" es menos
// común que "keep watching forward", y evictar-luego-rehacer
// atrás es más barato (el ffmpeg respawn desde un keyframe atrás
// solo cuesta lo que tarde librqbit en re-servir esas piezas, ya
// cacheadas por libraría).

#[cfg(feature = "gui")]
const EVICT_INTERVAL_SECS: u64 = 10;
#[cfg(feature = "gui")]
const EVICT_SAFETY_WINDOW: u64 = HLS_LOOKAHEAD + 2;
#[cfg(feature = "gui")]
const EVICT_TARGET_RATIO: f64 = 0.9;

/// Spawnea la tarea de eviction. El JoinHandle se guarda en
/// `HlsState._evictor` para que `Drop` la aborte al cerrar el
/// stream (si no, seguiría escaneando un dir borrado).
#[cfg(feature = "gui")]
fn spawn_lru_evictor(
    dir: PathBuf,
    budget_bytes: u64,
    playhead: Arc<AtomicU64>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(EVICT_INTERVAL_SECS);
        loop {
            tokio::time::sleep(interval).await;
            // El dir puede haber desaparecido si el stream cerró
            // entre ticks — abortamos silenciosamente.
            if !dir.exists() {
                return;
            }
            let head = playhead.load(Ordering::Relaxed);
            if let Err(e) = evict_once(&dir, budget_bytes, head).await {
                tracing::warn!(target: "hls-evict", error = %e, "cycle error");
            }
        }
    })
}

/// Un ciclo del evictor. Async solo por conveniencia (usa
/// `spawn_blocking` para el I/O — read_dir puede ser lento en
/// tempdirs con miles de entradas).
#[cfg(feature = "gui")]
async fn evict_once(dir: &Path, budget_bytes: u64, playhead_idx: u64) -> Result<()> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || evict_once_sync(&dir, budget_bytes, playhead_idx))
        .await
        .context("evict spawn_blocking join")?
}

#[cfg(feature = "gui")]
fn evict_once_sync(dir: &Path, budget_bytes: u64, playhead_idx: u64) -> Result<()> {
    let entries = std::fs::read_dir(dir).context("read_dir tempdir")?;
    // (idx, path, size). Solo consideramos `.ts` estables (no
    // `.ts.tmp` — esos son de ffmpeg escribiendo y borrarlos
    // rompería el job en curso).
    let mut segs: Vec<(u64, PathBuf, u64)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let name_os = entry.file_name();
        let name = match name_os.to_str() {
            Some(s) => s,
            None => continue,
        };
        if !name.ends_with(".ts") || name.ends_with(".ts.tmp") {
            continue;
        }
        let Some(idx) = parse_seg_idx(name) else {
            continue;
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        total += size;
        segs.push((idx, entry.path(), size));
    }
    if total <= budget_bytes {
        return Ok(());
    }
    // Sobrepasado. Objetivo: bajar a 90% del budget.
    let target = (budget_bytes as f64 * EVICT_TARGET_RATIO) as u64;
    // Orden por prioridad de eviction: distancia al playhead,
    // con penalty para "atrás" (borra atrás antes que adelante).
    // El score menor se evicta primero.
    // score = (idx > playhead ? distance*2 : distance)
    let head = playhead_idx;
    segs.sort_by_key(|(idx, _, _)| {
        let dist = (*idx).abs_diff(head);
        // Penalizar segmentos ADELANTE (los queremos conservar
        // porque el user probablemente sigue viendo): score alto
        // → se evictan más tarde.
        if *idx > head {
            u64::MAX - dist.saturating_mul(2)
        } else {
            u64::MAX - dist
        }
    });
    // Después del sort, los primeros son los más "cerca" en el
    // sentido de nuestro score → NO queremos borrarlos. Los del
    // final son los más lejanos → los borramos.
    let mut freed: u64 = 0;
    let mut removed: usize = 0;
    while total.saturating_sub(freed) > target {
        let Some((idx, path, size)) = segs.pop() else {
            break;
        };
        // Safety window: nunca borramos idx en
        // [head - safety, head + safety].
        let in_safe_window = idx.abs_diff(head) <= EVICT_SAFETY_WINDOW;
        if in_safe_window {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            freed += size;
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(
            target: "hls-evict",
            freed_mb = freed / 1_048_576,
            segments = removed,
            head,
            budget_mb = budget_bytes / 1_048_576,
            total_before_mb = total / 1_048_576,
            "evicted"
        );
    }
    Ok(())
}

// ── helpers para elegir codec/bitrate de audio en spawn_hls ───

/// Devuelve `(channels, codec)` del stream de audio que ffmpeg va
/// a mapear en `spawn_hls` (`audio_idx` explícito o el primero por
/// defecto). Consulta `cached_probe`; si no hay probe cacheado
/// devuelve `(None, None)`.
///
/// Se usa para elegir bitrate AAC y — en el futuro — decidir
/// `-c:a copy` cuando la fuente ya es AAC/MP3 (audit §3).
///
/// `audio_idx` es el índice contando SÓLO streams de audio (igual
/// que el argv `-map 0:a:<n>`).
#[cfg(feature = "gui")]
async fn probe_selected_audio(
    state: &AppState,
    audio_idx: Option<usize>,
) -> (Option<u32>, Option<String>) {
    let guard = state.cached_probe.lock().await;
    let Some(info) = guard.as_ref() else {
        return (None, None);
    };
    let mut audios = info
        .streams
        .iter()
        .filter(|s| s.kind == crate::ffmpeg::StreamKind::Audio);
    let target = match audio_idx {
        Some(n) => audios.nth(n),
        None => audios.next(),
    };
    match target {
        Some(a) => (a.channels, Some(a.codec.clone())),
        None => (None, None),
    }
}

/// Bitrate AAC transparente-perceptual escalado por canales.
/// `≤2ch` o desconocido → 256k. `3-6ch` (5.1) → 384k. `7+ch`
/// (7.1+) → 512k. AAC LC a ~64k/canal es transparente para
/// material típico. Sin canales conocidos, 256k es el suelo seguro
/// (nunca peor que el 192k anterior). Audit §5.
#[cfg(feature = "gui")]
fn aac_bitrate_for_channels(channels: Option<u32>) -> &'static str {
    match channels {
        Some(n) if n >= 7 => "512k",
        Some(n) if n >= 3 => "384k",
        _ => "256k",
    }
}

/// `true` si el stream de vídeo principal del `cached_probe` es
/// HDR (SMPTE 2084 / arib-std-b67 / bt2020-10). Se consulta en
/// `spawn_hls` (rama Transcode) para meter la cadena
/// zscale+tonemap y evitar colores lavados en SDR. Audit §8.
#[cfg(feature = "gui")]
async fn probe_is_hdr_video(state: &AppState) -> bool {
    let guard = state.cached_probe.lock().await;
    let Some(info) = guard.as_ref() else {
        return false;
    };
    info.streams
        .iter()
        .find(|s| s.kind == crate::ffmpeg::StreamKind::Video)
        .map(is_hdr_stream)
        .unwrap_or(false)
}

/// Spawnea un ffmpeg que producirá `seg-<idx>.ts`, `seg-<idx+1>.ts`,
/// … en `dir` (tempdir compartido). Argv clave:
///
///   * `-ss <start_seconds>` antes de `-i`: fast seek por demuxer
///     (keyframe ≤ t). En modo Transcode combinado con
///     `-force_key_frames expr:gte(t,n_forced*4)`. En modo Copy
///     `start_seconds` es EXACTAMENTE el timestamp de un keyframe
///     real (viene de `HlsState.segments`, construido desde el
///     `KeyframeIndex`), así que el primer segmento arranca sin
///     drop de frames — sin `-force_key_frames` (irrelevante con
///     `-c:v copy`).
///
///   * `-start_number <idx>`: los ficheros se numeran desde el
///     índice global, coincidiendo con los URIs del playlist
///     estático (`seg-<idx>.ts`).
///
///   * `-output_ts_offset <start_seconds>`: los PTS del MPEG-TS de
///     salida arrancan en el tiempo absoluto del segmento, no en 0.
///     Sin esto, `currentTime`, subtítulos y timeline se
///     desplazarían tras cada reinicio de ffmpeg.
///
///   * `-hls_flags independent_segments+temp_file+omit_endlist`:
///     `temp_file` es la clave — ffmpeg escribe `seg-NNNNN.ts.tmp`
///     y renombra atómicamente a `.ts` al cerrar.
///
/// Dos ramas de encoding según `mode`:
///
///   * `Transcode`: libx264 CRF 18 High + AAC (audit §5). Cortes
///     de segmento en múltiplos de `HLS_SEG_SECS` forzados por el
///     encoder.
///
///   * `Copy`: `-c:v copy` (audit §2). Cero pérdida en vídeo. Los
///     cortes caen donde el archivo YA tiene keyframes. `-hls_time`
///     recibe la duración del segmento actual del grid, para que
///     ffmpeg cierre el `.ts` en el siguiente keyframe cercano.
#[cfg(feature = "gui")]
async fn spawn_hls(
    state: &AppState,
    dir: &Path,
    idx: u64,
    audio_idx: Option<usize>,
    mode: HlsMode,
    start_seconds: f64,
) -> Result<tokio::process::Child, (StatusCode, String)> {
    let bin = crate::ffmpeg::ffmpeg_binary().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "ffmpeg no encontrado".to_string(),
        )
    })?;
    let seg_pattern = dir.join("seg-%05d.ts");
    let live_playlist = dir.join("live.m3u8");
    let input_url = format!("http://{}/video", state.local_addr);

    let mut cmd = tokio::process::Command::new(bin);
    // Windows: sin `CREATE_NO_WINDOW`, cada spawn de ffmpeg abriría
    // una ventana `conhost.exe` visible mientras dure el transmux
    // (y otra por cada respawn de segmento). No-op fuera de Windows.
    cmd.hide_console();
    // Loglevel: `error` por defecto para no ensuciar la consola en
    // uso normal. Activable con `VIDEODROME_DEBUG=1` para ver
    // headers/decisiones de ffmpeg cuando hay que reproducir un bug.
    let loglevel = if std::env::var("VIDEODROME_DEBUG").is_ok() {
        "info"
    } else {
        "error"
    };
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg(loglevel)
        .arg("-nostdin")
        // Normalización de timestamps del input: reconstruimos
        // PTS/DTS desde 0 en el input. El `-output_ts_offset` de
        // más abajo re-aplica el timestamp absoluto al mux de salida.
        .arg("-fflags")
        .arg("+genpts");
    if start_seconds > 0.0 {
        cmd.arg("-ss").arg(format!("{start_seconds}"));
    }
    cmd.arg("-i").arg(&input_url);
    // Stream mapping: video default (0:v:0) + audio configurable.
    // Sin `-map`, ffmpeg elige "best" según sus heurísticas (que
    // en muchos MKV se traducen a picar la primera pista de audio,
    // que a menudo NO es el idioma que el user quiere). Con `-map`
    // explícito el user controla desde el panel de audio del player;
    // sin selección, matcheamos el comportamiento previo (0:a:0).
    cmd.arg("-map").arg("0:v:0");
    match audio_idx {
        Some(idx) => {
            cmd.arg("-map").arg(format!("0:a:{idx}"));
        }
        None => {
            cmd.arg("-map").arg("0:a:0?");
        }
    }
    // Video: rama COPY vs TRANSCODE.
    match mode {
        HlsMode::Copy => {
            // Audit §2: remux sin pérdida. Con -c:v copy no se
            // puede forzar keyframes; los cortes de segmento caen
            // donde el archivo YA los tiene (por eso construimos
            // el grid desde el KeyframeIndex).
            //
            // `-copyts` conserva los timestamps del input (críticos
            // para que los PTS del TS caigan alineados con el grid).
            // Combinado con `-output_ts_offset` reproducimos el
            // tiempo absoluto sin drift.
            cmd.arg("-c:v")
                .arg("copy")
                // Sin `-avoid_negative_ts make_zero` (rompería el
                // offset absoluto en modo copy). Sin `-fflags
                // +genpts` (los PTS del input SON la fuente de
                // verdad para el corte de segmento por keyframe).
                //
                // NB: overridamos el +genpts anterior — ffmpeg
                // acepta múltiples -fflags y aplica el último.
                .arg("-fflags")
                .arg("+discardcorrupt");
        }
        HlsMode::Transcode => {
            // Audit §5: CRF 18 High 5.2 + veryfast.
            // Audit §8: si el input es HDR (SMPTE 2084 / HLG),
            // hay que tonemap → SDR BT.709. La receta canónica
            // (Hable) requiere `zscale` (libzimg). Homebrew core
            // NO lo compila desde ffmpeg 8.x; hay que instalar
            // desde el tap `homebrew-ffmpeg/ffmpeg`.
            //
            // Sin zscale, `colorspace` solo cambia primaries (no
            // tonemap) y `tonemap` sin linealización previa
            // produce basura → mejor NO poner filter chain y
            // dejar que ffmpeg haga naive 10→8-bit downcast:
            // HDR queda visualmente lavado pero al menos
            // reproduce a resolución nativa sin pérdida
            // espacial.
            if probe_is_hdr_video(state).await {
                if crate::ffmpeg::ffmpeg_has_filter("zscale") {
                    // Cadena canónica FFmpeg wiki HDR10 → SDR:
                    // linearize PQ → gamut BT.709 → tonemap Hable
                    // → codificar en YUV 4:2:0 8-bit.
                    let vf = "zscale=t=linear:npl=100,format=gbrpf32le,\
                              zscale=p=bt709,tonemap=tonemap=hable:desat=0,\
                              zscale=t=bt709:m=bt709:r=tv,format=yuv420p";
                    cmd.arg("-vf").arg(vf);
                    tracing::info!(target: "hls", "HDR → zscale+tonemap Hable (calidad máxima)");
                } else {
                    // Sin zscale: naive downcast. `-pix_fmt
                    // yuv420p` (que ya está más abajo en el argv)
                    // hace el 10→8-bit sin tonemap. No metemos
                    // `-vf` porque cualquier cadena intermedia
                    // sin linealización produce peor resultado
                    // que la conversión directa.
                    tracing::warn!(
                        target: "hls",
                        "HDR sin `zscale` (ffmpeg sin libzimg) — reproduzco en SDR sin \
                         tonemap (colores lavados). Para calidad HDR→SDR real: \
                         `brew tap homebrew-ffmpeg/ffmpeg && brew install \
                         homebrew-ffmpeg/ffmpeg/ffmpeg` (compila con libzimg)."
                    );
                }
            }
            cmd.arg("-c:v")
                .arg("libx264")
                .arg("-preset")
                .arg("veryfast")
                .arg("-crf")
                .arg("18")
                .arg("-profile:v")
                .arg("high")
                // Level 5.2 en vez de 4.1: 4.1 topa a 1080p@30 y
                // libx264 con input 4K emite un stream "fuera de
                // spec" que algunos players rechazan. 5.2 cubre
                // 4K@60fps y todo H.264 razonable — WKWebView,
                // WebView2 y WebKitGTK lo aceptan sin problema.
                .arg("-level:v")
                .arg("5.2")
                .arg("-pix_fmt")
                .arg("yuv420p")
                .arg("-bf")
                .arg("0")
                // Keyframes forzados en múltiplos exactos de 4s (0,
                // 4, 8, …). Requisito para que dos jobs distintos
                // (uno desde 0, otro desde `-ss 1728`) corten
                // segmentos en las mismas fronteras temporales, y
                // por tanto sean intercambiables.
                .arg("-force_key_frames")
                .arg("expr:gte(t,n_forced*4)")
                .arg("-x264-params")
                .arg("scenecut=0:slices=1:sliced-threads=0")
                // Reset de timestamps al mínimo tras el input
                // (combina con `+genpts`). El `-output_ts_offset`
                // de abajo reintroduce el tiempo absoluto en el
                // mux de salida.
                .arg("-avoid_negative_ts")
                .arg("make_zero");
        }
    }
    // Audio: rama COPY (AAC/MP3 sin recodificar, audit §3) vs
    // TRANSCODE AAC. Copy es cero pérdida y ahorra CPU; solo se
    // usa para códecs que el mux MPEG-TS acepta directamente sin
    // BSF complicados.
    //
    //   * AAC / MP3    → copy universalmente (todos los WebView
    //                    decodifican, TS los acepta directo).
    //   * AC-3 / E-AC-3 → copy SOLO si el cliente declara soporte
    //                    (WKWebView macOS sí; WebView2 depende).
    //                    Preserva Dolby Digital 5.1/7.1 original
    //                    en cero pérdida.
    //   * DTS / TrueHD → los WebView no los decodifican vía
    //                    <video>; siempre transcode a AAC.
    let (audio_channels, in_audio_codec) = probe_selected_audio(state, audio_idx).await;
    let caps = current_client_capabilities();
    let audio_copy_ok = match in_audio_codec.as_deref() {
        Some("aac") | Some("mp3") => true,
        Some("ac3") => caps.supports("ac3"),
        Some("eac3") => caps.supports("eac3"),
        _ => false,
    };
    if audio_copy_ok {
        cmd.arg("-c:a").arg("copy");
        // AAC en MPEG-TS: ffmpeg añade ADTS headers automáticamente
        // al copiar desde MP4/MKV. AC-3 / E-AC-3 / MP3 van directo.
        tracing::debug!(
            target: "hls",
            src = ?in_audio_codec,
            channels = ?audio_channels,
            mode = ?mode,
            "audio: -c:a copy"
        );
    } else {
        // Audit §5: AAC con bitrate escalado por canales, SIN
        // forzar downmix. Bitrates elegidos para transparencia
        // perceptual (~64k/canal AAC LC):
        //   * ≤2ch o desconocido: 256k
        //   * 3-6ch (5.1):        384k
        //   * 7+ch (7.1):         512k
        let aac_bitrate = aac_bitrate_for_channels(audio_channels);
        tracing::debug!(
            target: "hls",
            src = ?in_audio_codec,
            channels = ?audio_channels,
            mode = ?mode,
            bitrate = %aac_bitrate,
            "audio: transcode aac"
        );
        cmd.arg("-c:a").arg("aac").arg("-b:a").arg(aac_bitrate);
    }
    // Sin subs, sin data.
    cmd.arg("-sn").arg("-dn");
    // HLS output. `temp_file` es crítico para que solo veamos .ts
    // completos. `omit_endlist` evita que ffmpeg escriba ENDLIST en
    // el `live.m3u8` que ignoramos.
    cmd.arg("-f")
        .arg("hls")
        .arg("-hls_time")
        .arg(HLS_SEG_SECS.to_string())
        .arg("-hls_list_size")
        .arg("0")
        .arg("-hls_segment_type")
        .arg("mpegts")
        .arg("-hls_flags")
        .arg("independent_segments+temp_file+omit_endlist")
        // Numeración desde el idx global.
        .arg("-start_number")
        .arg(idx.to_string())
        // PTS absolutos en el mux de salida.
        .arg("-output_ts_offset")
        .arg(format!("{start_seconds}"))
        .arg("-hls_segment_filename")
        .arg(&seg_pattern)
        .arg(&live_playlist);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("spawn ffmpeg (hls): {e}"),
        )
    })?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // ffmpeg stderr → nivel `debug` (con `-loglevel error`
                // solo emite lo importante). Al operar con
                // `VIDEODROME_LOG_LEVEL=debug` el usuario ve el argv
                // completo + errores por consola.
                tracing::debug!(target: "ffmpeg-hls", "{line}");
            }
        });
    }
    tracing::info!(
        target: "hls",
        event = "spawn",
        mode = ?mode,
        idx,
        start_seconds,
        dir = %dir.display(),
        "ffmpeg spawned"
    );
    Ok(child)
}

/// Middleware que añade cabeceras CORS permisivas a toda respuesta del
/// servidor local de streaming. Necesario porque el WebView de Tauri
/// vive en `http://127.0.0.1:1420` (dev) o `tauri://localhost` (prod),
/// mientras que este servidor bind-ea a un puerto aleatorio de
/// `127.0.0.1` → distinto origen a ojos del navegador. Sin CORS:
///
///   * `fetch()` a `/probe.json` desde React falla con "not allowed by
///     Access-Control-Allow-Origin" y devuelve `NotSupportedError`.
///   * `<video src="…/play.mp4">` cross-origin dispara un preflight
///     opaco y en algunas versiones de WKWebView aborta la carga
///     silenciosamente (MediaError code 4 sin mensaje).
///
/// El servidor solo escucha en localhost y su vida está atada al
/// StreamHandle, así que abrirlo con `*` no expone nada externo.
async fn add_cors_headers(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    // OPTIONS preflight: devolvemos 204 con los headers antes de que
    // el router intente rutar (algunas versiones de WKWebView los
    // mandan aunque nuestros GET son "simple requests").
    if req.method() == axum::http::Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS")
            .header("Access-Control-Allow-Headers", "Range, Content-Type")
            .header(
                "Access-Control-Expose-Headers",
                "Content-Length, Content-Range, Accept-Ranges",
            )
            .header("Access-Control-Max-Age", "86400")
            .body(Body::empty())
            .unwrap_or_else(|_| Response::new(Body::empty()));
    }
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
    headers.insert(
        "Access-Control-Expose-Headers",
        HeaderValue::from_static("Content-Length, Content-Range, Accept-Ranges"),
    );
    resp
}

// ── HTML player: probe + HLS transmux ────────────────────────────────────
//
// Endpoints usados por la view `Player.tsx`:
//
//   GET /probe.json           → JSON con codec info (ffprobe cacheado)
//   GET /hls/playlist.m3u8    → playlist VOD estático (duración del
//                                probe → N segmentos enumerados)
//   GET /hls/seg-NNNNN.ts     → segmento transcodeado bajo demanda
//                                (ffmpeg arranca desde el idx pedido
//                                cuando el fichero no existe aún)
//
// El path fMP4 (`/play.mp4`) existió durante la fase inicial del player
// pero WKWebView rechaza fMP4 vía `<video src>` incluso con H.264 High
// estándar (solo lo acepta vía MSE con JS), así que se eliminó. Todo
// lo que no es `direct_playable` pasa por HLS.
//
// Todos leen la misma URL interna `http://127.0.0.1:PORT/video` que sirve
// el fichero raw del torrent con soporte Range — ffmpeg/ffprobe ya
// hablan HTTP nativamente. Con esto no duplicamos código de piece
// picking: librqbit sigue viendo un solo consumidor secuencial.

#[cfg(feature = "gui")]
async fn serve_probe(
    State(state): State<AppState>,
) -> Result<axum::Json<crate::ffmpeg::MediaInfo>, (StatusCode, String)> {
    let mut info = ensure_probe(&state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Audit §4: `direct_playable` se calcula por request con las
    // caps del cliente EN VIGOR (no las que había cuando se pobló
    // el `cached_probe`). Si el frontend registra caps DESPUÉS del
    // primer probe, el próximo `/probe.json` ya refleja el cambio.
    let caps = current_client_capabilities();
    info.direct_playable = crate::ffmpeg::compute_direct_playable(&info, &caps);
    Ok(axum::Json(info))
}

/// Devuelve el `MediaInfo` cacheado; si no está, lo genera con
/// `ffprobe` sobre el endpoint `/video` local. Idempotente y
/// thread-safe: si dos requests concurrentes piden probe la primera
/// coge el lock y las siguientes reusan el resultado.
#[cfg(feature = "gui")]
async fn ensure_probe(state: &AppState) -> Result<crate::ffmpeg::MediaInfo> {
    let mut guard = state.cached_probe.lock().await;
    if let Some(info) = guard.as_ref() {
        return Ok(info.clone());
    }
    let url = format!("http://{}/video", state.local_addr);
    let info = crate::ffmpeg::probe(&url).await?;
    *guard = Some(info.clone());
    Ok(info)
}

/// `POST /hls/audio?idx=<N>` — cambia la pista de audio activa del
/// stream HLS transmux. `N` es el índice del stream de audio en el
/// input tal cual lo reporta ffprobe (`MediaInfo.streams` filtrado
/// por `kind == "audio"`, orden original).
///
/// Semántica: mata el ffmpeg job actual (si lo hay), purga los
/// segmentos `.ts` producidos con la pista anterior, y guarda la
/// nueva selección en `HlsState.audio_idx`. La próxima petición de
/// segmento respawnea ffmpeg con `-map 0:v:0 -map 0:a:<idx>`.
///
/// El frontend debe:
///   1. Guardar `currentTime` antes del POST.
///   2. Esperar el 204.
///   3. `hls.destroy()` + `new Hls().loadSource(playlist)` de nuevo,
///      y hacer seek al `currentTime` guardado en `onCanPlay`.
///
/// Si se pide un idx igual al actual, es no-op (retorna 204 sin
/// tocar nada).
#[cfg(feature = "gui")]
#[derive(serde::Deserialize)]
struct AudioSwitchQuery {
    idx: usize,
}

#[cfg(feature = "gui")]
async fn set_hls_audio(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<AudioSwitchQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Asegura que el HlsState existe (aunque no haya empezado el
    // playback aún: el user puede abrir el panel de audio y cambiar
    // antes de darle a play).
    let _ = ensure_hls_dir(&state).await?;

    let (old_job, dir, changed) = {
        let mut guard = state.hls.lock().await;
        let hls = guard.as_mut().expect("hls state ensured");
        let changed = hls.audio_idx != Some(q.idx);
        if !changed {
            return Ok(StatusCode::NO_CONTENT);
        }
        hls.audio_idx = Some(q.idx);
        (hls.job.take(), hls.dir.clone(), changed)
    };

    if let Some(mut old) = old_job {
        // Igual que en `ensure_hls_job` — cancelar la Range GET del
        // ffmpeg viejo antes de matarlo, para que librqbit libere
        // el FileStream inmediatamente.
        {
            let mut req_guard = state.active_request.lock().await;
            if let Some((prev_id, token, _)) = req_guard.take() {
                token.cancel();
                tracing::info!(
                    target: "hls",
                    reason = "audio_switch",
                    cancelled_prev = prev_id,
                    "cancelling /video active_request before killing old ffmpeg"
                );
            }
        }
        let _ = old.child.kill().await;
        let _ = old.child.wait().await;
        tracing::info!(
            target: "hls",
            start_idx = old.start_idx,
            reason = "audio_switch",
            "killed old ffmpeg job"
        );
    }

    // Purgar los `.ts` producidos con la pista anterior. Si no lo
    // hacemos, hls.js pediría un segmento que existe en disco (con
    // audio viejo) → mezcla de audios entre segmentos consecutivos.
    if changed {
        if let Ok(iter) = std::fs::read_dir(&dir) {
            for entry in iter.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s.starts_with("seg-") && (s.ends_with(".ts") || s.ends_with(".ts.tmp")) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /subs/embedded/<idx>` — extrae la pista de subtítulos
/// `<idx>` del contenedor y la devuelve como WebVTT text/plain UTF-8.
///
/// (Sin extensión `.vtt` en el path porque axum no permite mezclar
/// literal + capture en el mismo segmento; el `Content-Type: text/vtt`
/// del response identifica el formato.)
///
/// Solo funciona con subs "de texto" (SRT/ASS/SSA). Los subs de
/// imagen (PGS/DVBSUB/VobSub) NO se pueden convertir a VTT sin OCR;
/// ffmpeg falla y devolvemos 415 Unsupported Media Type para que el
/// frontend los oculte del panel de subs.
///
/// El `idx` es el índice del stream de subs en el input tal cual lo
/// reporta ffprobe (0..N-1 dentro del filter `-map 0:s:<idx>`).
///
/// Spawn one-shot (no persistente): abre input, extrae el stream,
/// pipea a stdout, muere. Coste ≈ 200-500ms para subs de peli
/// completa. El player cachea el VTT en un Blob del navegador, así
/// que solo se llama una vez por selección.
#[cfg(feature = "gui")]
async fn serve_embedded_subtitle(
    State(state): State<AppState>,
    axum::extract::Path(idx): axum::extract::Path<usize>,
) -> Result<Response, (StatusCode, String)> {
    let bin = crate::ffmpeg::ffmpeg_binary().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "ffmpeg no encontrado".to_string(),
    ))?;
    let input_url = format!("http://{}/video", state.local_addr);

    let output = {
        let mut cmd = tokio::process::Command::new(bin);
        cmd.arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-nostdin")
            .arg("-i")
            .arg(&input_url)
            // El input `/video` puede tardar en dar los primeros bytes
            // si el torrent está frío; `-analyzeduration` alto ayuda a
            // que ffmpeg no se rinda antes de encontrar la pista.
            .arg("-analyzeduration")
            .arg("60M")
            .arg("-probesize")
            .arg("50M")
            .arg("-map")
            .arg(format!("0:s:{idx}"))
            .arg("-c:s")
            .arg("webvtt")
            .arg("-f")
            .arg("webvtt")
            .arg("-")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        // Windows: sin `CREATE_NO_WINDOW` este spawn one-shot
        // parpadearía una consola cada vez que el user selecciona un
        // sub embebido. No-op fuera de Windows.
        cmd.hide_console();
        cmd.output().await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("spawn ffmpeg: {e}"),
            )
        })?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Bitmap subs → ffmpeg da "Subtitle encoding currently only
        // possible from text to text or bitmap to bitmap". Distinguir
        // con un 415 al frontend para que oculte esta pista.
        let unsupported = stderr.contains("only possible")
            || stderr.contains("bitmap")
            || stderr.contains("Filter graph");
        let code = if unsupported {
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        return Err((code, format!("ffmpeg extraction failed: {stderr}")));
    }

    // Sanidad: el output debe empezar por `WEBVTT` (o \ufeff+WEBVTT)
    // para ser un track válido. Si no, ffmpeg devolvió algo raro
    // aunque saliese con status 0.
    let body = output.stdout;
    let head: String = body.iter().take(16).map(|&b| b as char).collect();
    if !head.contains("WEBVTT") {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "output no es WebVTT".to_string(),
        ));
    }

    let mut resp = Response::new(body.into());
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        "text/vtt; charset=utf-8".parse().unwrap(),
    );
    Ok(resp)
}

/// Handle del reproductor externo (VLC). Contiene:
///
/// * `alive` — flag compartido que pasa a `false` cuando VLC termina
///   (por cierre del user o por `kill()`). El caller lo consulta en
///   loop para saber cuándo liberar el stream.
/// * `kill_token` — cancellation token que, al ser disparado, cierra
///   VLC de forma activa. Necesario porque en macOS spawneamos VLC vía
///   `open -W -a VLC` (LaunchServices lanza VLC en su propio proceso);
///   matar el proceso hijo `open` NO cierra VLC. Idem en Windows con
///   `cmd /C start /wait vlc`. Por eso el kill efectivo lo hace
///   `quit_vlc()` invocando el mecanismo nativo (`osascript` /
///   `pkill` / `taskkill`) sobre el proceso VLC por nombre.
pub struct PlayerHandle {
    pub alive: Arc<std::sync::atomic::AtomicBool>,
    kill_token: CancellationToken,
}

impl PlayerHandle {
    /// Cierra VLC de forma activa. Idempotente: si VLC ya no está
    /// corriendo, `quit_vlc()` es un no-op silencioso.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn kill(&self) {
        self.kill_token.cancel();
    }
}

/// Abre la URL con VLC y devuelve un [`PlayerHandle`] con el flag
/// `alive` (pasa a `false` cuando VLC termina) y un token para cerrar
/// VLC activamente vía `PlayerHandle::kill`.
///
/// Si `sub_path` está informado, se le pasa a VLC como `--sub-file=…` para
/// que arranque con los subtítulos ya cargados.
///
/// Si `start_seconds` está informado, se pasa como `--start-time=<seg>`
/// para reanudar desde una posición concreta (feature "resume desde
/// donde lo dejaste"). Solo tiene efecto en la reproducción actual — VLC
/// hace seek dentro del stream HTTP igual que un seek de usuario.
///
/// * macOS: `open -W -a VLC --args <url> [--sub-file=<path>] [--start-time=N]`
///   — `-W` hace que `open` bloquee hasta que VLC salga del todo (⌘Q).
///   Cerrar solo la ventana no cuenta (patrón estándar macOS). Si VLC no
///   está instalado, cae a `open <url>` (sin `-W`; el flag queda `false`
///   inmediatamente).
/// * Linux: `vlc <url> [--sub-file=<path>] [--start-time=N]` directo.
/// * Windows: `start /wait vlc <url> [--sub-file=<path>] [--start-time=N]`
///   — bloquea hasta que VLC cierre.
///
/// Si no se puede lanzar ningún reproductor, el flag queda en `false` para
/// que la TUI limpie el stream inmediatamente en lugar de dejarlo colgando.
pub fn open_in_vlc(
    url: &str,
    sub_path: Option<&std::path::Path>,
    start_seconds: Option<u32>,
) -> PlayerHandle {
    use std::sync::atomic::{AtomicBool, Ordering};

    let alive = Arc::new(AtomicBool::new(true));
    let kill_token = CancellationToken::new();

    // Preparamos el arg de sub UNA sola vez para no repetir la lógica en
    // cada rama de SO. VLC acepta `--sub-file=/ruta/absoluta.srt`.
    let sub_arg: Option<String> = sub_path.map(|p| format!("--sub-file={}", p.display()));
    // `--start-time=` en segundos (VLC acepta decimales, pero aquí no los
    // necesitamos: el resume viene de una fracción sobre file_len que ya
    // se redondea en el frontend).
    let start_arg: Option<String> = start_seconds
        .filter(|n| *n > 0)
        .map(|n| format!("--start-time={n}"));

    let child_result: std::io::Result<tokio::process::Child> = {
        #[cfg(target_os = "macos")]
        {
            // `open` con -a spawnea el subproceso aunque VLC no esté
            // instalado (el error viene después en tiempo de ejecución),
            // así que el fallback Err(_) NUNCA se ejecutaba. Comprobamos
            // la existencia primero con `mdfind` / rutas estándar.
            if !macos_vlc_installed() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "VLC no est\u{e1} instalado",
                ))
            } else {
                let mut cmd = tokio::process::Command::new("open");
                cmd.args(["-W", "-a", "VLC", "--args", url]);
                if let Some(a) = sub_arg.as_deref() {
                    cmd.arg(a);
                }
                if let Some(a) = start_arg.as_deref() {
                    cmd.arg(a);
                }
                cmd.spawn()
            }
        }
        #[cfg(target_os = "linux")]
        {
            // Sin VLC en Linux no podemos abrir un stream local: xdg-open
            // sobre http://127.0.0.1:... abriría el navegador, no un
            // reproductor. Propagamos el error de spawn tal cual.
            let mut cmd = tokio::process::Command::new("vlc");
            cmd.arg(url);
            if let Some(a) = sub_arg.as_deref() {
                cmd.arg(a);
            }
            if let Some(a) = start_arg.as_deref() {
                cmd.arg(a);
            }
            cmd.spawn()
        }
        #[cfg(target_os = "windows")]
        {
            // Windows: VLC NO se añade al PATH por el instalador
            // oficial. `cmd /C start vlc` fallaría silenciosamente
            // en la mayoría de instalaciones (cmd retorna 0 aunque
            // start no encuentre el binario). Y `start` tiene un
            // parseo de comillas frágil que puede destrozar el
            // `--sub-file=` cuando la ruta lleva espacios.
            //
            // Solución: localizar `vlc.exe` con el mismo patrón que
            // en macOS (PATH → rutas estándar → registro) y
            // spawnearlo DIRECTAMENTE. Sin `start`, args como
            // elementos separados → sin quoting a mano.
            match windows_vlc_path() {
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "VLC no est\u{e1} instalado. Descargalo de https://videolan.org o inst\u{e1}lalo con `winget install VideoLAN.VLC`.",
                )),
                Some(vlc_exe) => {
                    let mut cmd = tokio::process::Command::new(vlc_exe);
                    cmd.arg(url);
                    if let Some(a) = sub_arg.as_deref() {
                        cmd.arg(a);
                    }
                    if let Some(a) = start_arg.as_deref() {
                        cmd.arg(a);
                    }
                    // VLC.exe es subsistema GUI (no debería crear
                    // consola), pero pasamos `CREATE_NO_WINDOW` por
                    // consistencia con el resto de spawns Windows —
                    // así si Windows cambia el subsistema en una
                    // futura versión no nos pilla desprevenidos.
                    cmd.hide_console();
                    cmd.spawn()
                }
            }
        }
    };

    match child_result {
        Ok(mut child) => {
            let alive_task = alive.clone();
            let kill_token_task = kill_token.clone();
            tokio::spawn(async move {
                // Race entre "VLC terminó por su cuenta" (⌘Q, cierre de
                // ventana en Linux/Windows) y "el user pulsó Detener"
                // (kill_token). En el segundo caso llamamos a
                // `quit_vlc()` — que sí sabe cerrar VLC por nombre —
                // y después esperamos a `child.wait` para no dejar
                // procesos zombies.
                tokio::select! {
                    _ = child.wait() => {}
                    _ = kill_token_task.cancelled() => {
                        quit_vlc().await;
                        let _ = child.wait().await;
                    }
                }
                alive_task.store(false, Ordering::Relaxed);
            });
        }
        Err(_) => {
            // Nada se pudo lanzar → marca como no vivo para que el caller no
            // se quede pensando que está streamando eternamente.
            alive.store(false, Ordering::Relaxed);
        }
    }

    PlayerHandle { alive, kill_token }
}

/// Cierra VLC de forma activa usando el mecanismo nativo de cada SO.
/// Idempotente: si VLC no está corriendo, cada plataforma devuelve un
/// error que ignoramos silenciosamente.
///
/// * macOS: `osascript -e 'tell application "VLC" to quit'` — envía el
///   Apple Event `quit` a VLC, que cierra limpiamente. Matar el
///   proceso hijo `open` no serviría (VLC lo lanza LaunchServices en
///   un PID independiente).
/// * Linux: `pkill -TERM -x vlc` — mata el binario `vlc` por nombre
///   exacto. Coincide con lo que spawneamos en `open_in_vlc`.
/// * Windows: `taskkill /IM vlc.exe /T` — sin `/F` para dar chance a
///   VLC de guardar estado; `/T` recoge subprocesos del árbol.
///
/// Nota: el método es "cierra CUALQUIER VLC abierto en el sistema".
/// Si el user tiene una segunda ventana de VLC con contenido no
/// relacionado, también se cerrará. Es el trade-off por no poder
/// rastrear el PID exacto (macOS/Windows lo esconden detrás del
/// launcher). En una app de streaming es el comportamiento esperado.
async fn quit_vlc() {
    #[cfg(target_os = "macos")]
    {
        let _ = tokio::process::Command::new("osascript")
            .args(["-e", "tell application \"VLC\" to quit"])
            .status()
            .await;
    }
    #[cfg(target_os = "linux")]
    {
        let _ = tokio::process::Command::new("pkill")
            .args(["-TERM", "-x", "vlc"])
            .status()
            .await;
    }
    #[cfg(target_os = "windows")]
    {
        // Sin `CREATE_NO_WINDOW` el propio `taskkill` parpadearía
        // una consola cada vez que el user pulsa Detener con VLC
        // como player.
        let mut cmd = tokio::process::Command::new("taskkill");
        cmd.args(["/IM", "vlc.exe", "/T"]);
        cmd.hide_console();
        let _ = cmd.status().await;
    }
}

/// Chequea si VLC.app está instalado en el Mac. Necesario porque
/// `open -a VLC` spawnea el proceso `open` con éxito aunque VLC no
/// exista — el error real solo aparece en tiempo de ejecución, cuando
/// `open` ya devolvió Ok. Sin este check el `Err(_)` fallback nunca
/// se disparaba.
#[cfg(target_os = "macos")]
fn macos_vlc_installed() -> bool {
    use std::path::Path;
    for path in ["/Applications/VLC.app", "/System/Applications/VLC.app"] {
        if Path::new(path).exists() {
            return true;
        }
    }
    // Fallback por si el user tiene VLC en ~/Applications u otra ruta.
    if let Some(home) = dirs::home_dir() {
        if home.join("Applications/VLC.app").exists() {
            return true;
        }
    }
    false
}

/// Localiza `vlc.exe` en Windows con la misma estrategia que
/// `macos_vlc_installed` — primero PATH (usuarios de scoop/choco),
/// luego rutas estándar (`Program Files\VideoLAN\VLC`), finalmente
/// registro (`HKLM\SOFTWARE\VideoLAN\VLC` = ruta al binario). El
/// instalador oficial de VideoLAN NO añade VLC al PATH, así que sin
/// las dos últimas ramas el `Err(NotFound)` se disparaba en el 90%
/// de las instalaciones típicas.
///
/// Devuelve `None` si VLC no está instalado — el caller propaga un
/// error explicativo con el comando de winget.
#[cfg(target_os = "windows")]
fn windows_vlc_path() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    // 1. PATH.
    if let Ok(p) = which::which("vlc.exe") {
        return Some(p);
    }

    // 2. Rutas estándar de instalación (Program Files x64 + x86).
    // Usamos las variables de entorno para respetar instalaciones
    // en volúmenes distintos de C:.
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Ok(base) = std::env::var(var) {
            let candidate = PathBuf::from(base)
                .join("VideoLAN")
                .join("VLC")
                .join("vlc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // 3. Registro. `HKLM\SOFTWARE\VideoLAN\VLC` tiene un valor por
    // defecto (`""`) que es la ruta absoluta al `vlc.exe`. Lo pone
    // el instalador oficial. Silenciamos errores de lectura — es
    // solo el último cartucho.
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        for subkey in [
            r"SOFTWARE\VideoLAN\VLC",
            r"SOFTWARE\WOW6432Node\VideoLAN\VLC",
        ] {
            if let Ok(key) = hklm.open_subkey(subkey) {
                if let Ok(path) = key.get_value::<String, _>("") {
                    let p = PathBuf::from(path);
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }

    None
}

// ============================================================================
// Caché persistente de streams
// ============================================================================

/// Estado de resume persistido en `<data_dir>/resume.json`.
///
/// Dos fuentes lo escriben:
///
///   * El player HTML llama a `save_position(seconds, duration)` cada
///     ~15s mientras reproduce. Es la fuente PREFERIDA: viene del
///     `<video>.currentTime` (posición exacta) y funciona en modo
///     direct y en búsquedas sin TMDB (no necesita `runtime_minutes`
///     para convertir bytes a segundos).
///
///   * El Drop de `StreamHandle` escribe `fraction` (byte-based) como
///     fallback para el path VLC, que no puede reportar posición
///     porque el frontend no sabe qué tiempo lleva el spawn de VLC.
///     Es la aproximación vieja: `max_seek_bytes / file_len`, con la
///     precisión que te da suponer bitrate constante.
///
/// Frontend consume: si `seconds` está presente lo usa directo; si no,
/// cae al camino viejo (`fraction × runtime_minutes × 60`).
///
/// Las escrituras se HACEN merge-style (leer, mutar, escribir) para
/// que un save del player no borre el `fraction` del Drop previo y
/// viceversa.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resume {
    /// Fracción byte-based en [0.0, 1.0]. `0.0` si no se ha escrito
    /// (raro; Drop siempre la actualiza). Fallback histórico.
    #[serde(default)]
    pub fraction: f32,
    /// Segundos absolutos reportados por el player HTML. `None`
    /// cuando la última sesión fue VLC (que no reporta) o cuando
    /// llegamos al Drop antes del primer `report_position`.
    #[serde(default)]
    pub seconds: Option<f64>,
    /// Duración total conocida al momento del último report.
    /// Necesaria para calcular "% completado" en la regla de
    /// borrado y para pintar la barra sin depender de TMDB.
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    pub updated_at: u64,
    /// Metadata de episodio si el resume es de una serie (§6 audit).
    /// Habilita "continuar viendo" y la lógica de "siguiente episodio"
    /// sin re-parsear el nombre del fichero cada vez.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<ResumeEpisode>,
}

/// Metadata mínima para identificar un episodio en el resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeEpisode {
    pub season: u16,
    pub episode: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<u64>,
}

/// Wire-format del `resume.json` en disco (§6 audit). Antes había un
/// único `Resume` plano por infohash: dos episodios del mismo pack
/// compartían infohash y el resume de E03 machacaba el de E02.
/// Ahora un mapa por `file_id` (como string, por compatibilidad JSON).
///
/// Formato legacy (v1) se sigue leyendo — ver `load_store`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ResumeStore {
    #[serde(default)]
    files: HashMap<String, Resume>,
}

/// Discriminated read: primero intenta parsear la v2
/// (`{"files":{...}}`); si el fichero es legacy (`{"fraction":...}`
/// plano) cae al parser antiguo y lo migra a v2 in-memory bajo la
/// clave `"0"`. La migración se persiste en el siguiente `save_*`
/// (write-through) — no reescribimos aquí para mantener la ruta de
/// lectura pura.
///
/// Racional del audit §6: adoptar la entrada vieja para file_id=0 es
/// correcto para torrents mono-fichero (la única elección posible).
/// Para packs multi-fichero, la primera lectura devuelve el resume
/// bajo "0" — quizás no sea el fichero real que reproducía el user,
/// pero un mismatch puntual es mejor que perder la posición.
///
/// Distingue tres estados: fichero ausente (nueva entrada válida
/// vacía), store parseado, o corrupto (write parcial de una sesión
/// previa que murió a mitad — NO reescribir para preservar la
/// posibilidad de recuperación manual).
enum ResumeParse {
    Absent,
    Store(ResumeStore),
    Corrupt,
}

fn read_store(path: &Path) -> ResumeParse {
    let data = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return ResumeParse::Absent,
    };
    let value: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "resume",
                path = %path.display(),
                error = %e,
                "unparseable as JSON; preserving"
            );
            return ResumeParse::Corrupt;
        }
    };
    if value.get("files").is_some() {
        match serde_json::from_value::<ResumeStore>(value) {
            Ok(store) => ResumeParse::Store(store),
            Err(e) => {
                tracing::warn!(
                    target: "resume",
                    path = %path.display(),
                    error = %e,
                    "v2 parse failed; preserving"
                );
                ResumeParse::Corrupt
            }
        }
    } else {
        match serde_json::from_value::<Resume>(value) {
            Ok(legacy) => {
                let mut files = HashMap::new();
                files.insert("0".to_string(), legacy);
                ResumeParse::Store(ResumeStore { files })
            }
            Err(e) => {
                tracing::warn!(
                    target: "resume",
                    path = %path.display(),
                    error = %e,
                    "legacy parse failed; preserving"
                );
                ResumeParse::Corrupt
            }
        }
    }
}

/// Conveniencia: colapsa `Absent | Corrupt` a un store vacío. Solo
/// para lecturas puras (`load_resume*`) donde perder acceso al
/// corrupto es aceptable — el corrupto sigue en disco y el próximo
/// write lo respetará.
fn load_store(path: &Path) -> ResumeStore {
    match read_store(path) {
        ResumeParse::Store(s) => s,
        _ => ResumeStore::default(),
    }
}

/// Umbral de "peli terminada". Si el player reporta posición pasado
/// este porcentaje del runtime, borramos el `resume.json` para que la
/// próxima apertura no ofrezca reanudar los créditos.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
const COMPLETION_THRESHOLD: f64 = 0.95;

/// Directorio raíz de la caché de streams:
/// `<dirs::cache_dir>/videodrome/streams/`. Se crea si no existe.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn cache_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .context("No se puede obtener el directorio de caché del sistema")?
        .join("videodrome")
        .join("streams");
    std::fs::create_dir_all(&dir).with_context(|| format!("No se pudo crear {}", dir.display()))?;
    Ok(dir)
}

/// Re-export delgado: la implementación real (con validación de
/// formato) vive en `torrents::parse_infohash`. Existía una copia
/// aquí antes; se unificó para que el cache persistente y el dedupe
/// de providers usen exactamente la misma normalización.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn parse_infohash(magnet: &str) -> Option<String> {
    crate::torrents::parse_infohash(magnet)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Actualiza el mtime del sentinel `.last_used` dentro de `dir`. Si no
/// existe lo crea. El prune usa este mtime como "última vez usado".
fn touch_last_used(dir: &Path) -> std::io::Result<()> {
    let path = dir.join(LAST_USED_FILE);
    // `File::create` trunca a 0 bytes y actualiza mtime en el proceso.
    std::fs::File::create(&path).map(|_| ())
}

fn entry_last_used(dir: &Path) -> u64 {
    let sentinel = dir.join(LAST_USED_FILE);
    let meta = std::fs::metadata(&sentinel)
        .or_else(|_| std::fs::metadata(dir))
        .ok();
    meta.and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dir_size_bytes(dir: &Path) -> u64 {
    // Recorrido shallow: los torrents de librqbit ponen los ficheros
    // directamente en `dir/`, sin subcarpetas anidadas profundas más
    // allá de una posible carpeta del propio torrent. Un walk iterativo
    // simple sobra.
    let mut total = 0u64;
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(iter) = std::fs::read_dir(&p) else {
            continue;
        };
        for entry in iter.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
            } else if let Ok(m) = entry.metadata() {
                total = total.saturating_add(m.len());
            }
        }
    }
    total
}

/// Lee el `resume.json` de una entrada, para un `file_id` concreto.
/// Devuelve `None` si no hay entrada para ese file_id (o el fichero
/// no existe / está corrupto). Ver `load_store` para detalles del
/// wire format y la migración de v1 legacy.
///
/// Callers que no saben el file_id (dialog de resume ANTES del start)
/// pueden usar `load_resume_any`, que devuelve la entrada más
/// reciente del store.
#[allow(dead_code)]
pub fn load_resume(infohash: &str, file_id: usize) -> Option<Resume> {
    load_resume_in(&cache_dir().ok()?, infohash, file_id)
}

/// Devuelve la entrada de resume más reciente para el infohash, sin
/// especificar file_id. Útil para el dialog pre-start del player
/// (aún no se ha resuelto metadata → no hay file_id todavía). Si el
/// caller pasa `episode`, filtra a entradas cuya `episode` matchee
/// exactamente (S+E); útil para el flujo de serie donde el user ya
/// sabe qué episodio va a ver.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn load_resume_any(infohash: &str, episode: Option<(u16, u16)>) -> Option<Resume> {
    load_resume_any_in(&cache_dir().ok()?, infohash, episode)
}

/// Variante testeable: opera sobre un directorio base explícito
/// (`<base>/<infohash>/resume.json`) en vez de resolver `cache_dir()`.
#[allow(dead_code)]
fn load_resume_in(base: &Path, infohash: &str, file_id: usize) -> Option<Resume> {
    let path = base.join(infohash).join(RESUME_FILE);
    let store = load_store(&path);
    store.files.get(&file_id.to_string()).cloned()
}

fn load_resume_any_in(base: &Path, infohash: &str, episode: Option<(u16, u16)>) -> Option<Resume> {
    let path = base.join(infohash).join(RESUME_FILE);
    let store = load_store(&path);
    let mut candidates: Vec<&Resume> = if let Some((s, e)) = episode {
        store
            .files
            .values()
            .filter(|r| matches!(&r.episode, Some(ep) if ep.season == s && ep.episode == e))
            .collect()
    } else {
        store.files.values().collect()
    };
    candidates.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    candidates.first().map(|r| (*r).clone())
}

/// Persiste una posición reportada por el player HTML. Merge-style:
/// si ya existe un `resume.json` (con `fraction` puesto por el Drop
/// anterior), preservamos ese campo y solo actualizamos `seconds` +
/// `duration_seconds` + `updated_at`.
///
/// `file_id` selecciona la entrada dentro del store multi-file
/// (§6 audit) — dos episodios del mismo pack conviven sin pisarse.
/// `episode` guarda metadata de S/E cuando aplica (habilita
/// "continuar viendo" y "siguiente episodio" sin re-parsear
/// nombres).
///
/// Si la posición reportada supera `COMPLETION_THRESHOLD` (95%) del
/// runtime, borra SOLO esa entrada del store — otras entradas del
/// mismo torrent (otros episodios) sobreviven.
///
/// Errores silenciosos (log a stderr): el flujo del player no debe
/// romperse porque no podamos persistir una posición.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn save_position(
    infohash: &str,
    file_id: usize,
    seconds: f64,
    duration_seconds: f64,
    episode: Option<ResumeEpisode>,
) {
    let Ok(base) = cache_dir() else {
        return;
    };
    save_position_in(&base, infohash, file_id, seconds, duration_seconds, episode);
}

/// Variante testeable: idem `save_position` sobre un base dir
/// explícito. Los tests pueden crear un tempdir y llamar aquí sin
/// tocar la caché real del sistema (portable a macOS/Windows, donde
/// `dirs::cache_dir` no respeta `XDG_CACHE_HOME`).
fn save_position_in(
    base: &Path,
    infohash: &str,
    file_id: usize,
    seconds: f64,
    duration_seconds: f64,
    episode: Option<ResumeEpisode>,
) {
    let entry = base.join(infohash);
    // Si la entrada no existe (magnet nunca reproducido en persistente,
    // o purgada por el prune), no la creamos aquí — el StreamHandle
    // vivo la habría creado al arrancar.
    if !entry.exists() {
        return;
    }
    let path = entry.join(RESUME_FILE);

    // Read-modify-write con resiliencia a corrupción: si el fichero
    // existe pero no parsea (write parcial de una sesión previa),
    // NO lo sobreescribimos — preservar la posibilidad de recuperación
    // manual es preferible a machacar con default limpio.
    let mut store = match read_store(&path) {
        ResumeParse::Store(s) => s,
        ResumeParse::Absent => ResumeStore::default(),
        ResumeParse::Corrupt => return,
    };
    let key = file_id.to_string();

    // Regla de completado: si `seconds/duration > 0.95`, borra ESTA
    // entrada del store. Preserva otras entradas (otros episodios).
    // El check requiere una duración conocida > 0 — si el player nos
    // manda `duration_seconds = 0` (ffprobe falló, live stream), no
    // aplicamos la regla.
    if duration_seconds > 0.0 && seconds / duration_seconds >= COMPLETION_THRESHOLD {
        if store.files.remove(&key).is_some() {
            if store.files.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else if let Err(e) = write_store_atomic(&path, &store) {
                tracing::warn!(target: "resume", error = %e, "failed to persist store after completion");
            }
        }
        return;
    }

    let mut entry_r = store.files.remove(&key).unwrap_or_default();
    entry_r.seconds = Some(seconds.max(0.0));
    if duration_seconds > 0.0 {
        entry_r.duration_seconds = Some(duration_seconds);
    }
    if episode.is_some() {
        entry_r.episode = episode;
    }
    entry_r.updated_at = now_unix();
    store.files.insert(key, entry_r);

    if let Err(e) = write_store_atomic(&path, &store) {
        tracing::warn!(target: "resume", error = %e, "failed to persist position");
    }
}

/// Escribe el store atómicamente. Rename es atómico en POSIX y en
/// NTFS (Windows). No cross-device (tmp y destino en el mismo dir),
/// así que no falla por EXDEV. Evita que un crash o Cmd+Q a mitad de
/// escritura deje el fichero truncado (que la próxima lectura
/// interpretaría como corrupto y descartaría).
fn write_store_atomic(path: &Path, store: &ResumeStore) -> std::io::Result<()> {
    let json = serde_json::to_string(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// Tamaño total en bytes de la caché.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn total_size() -> u64 {
    let Ok(root) = cache_dir() else {
        return 0;
    };
    dir_size_bytes(&root)
}

/// Borra TODAS las entradas de la caché (equivalente a `rm -rf` del
/// directorio raíz, recreándolo vacío).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn clear_all() -> Result<()> {
    let root = cache_dir()?;
    // No borramos el root en sí: solo su contenido, así siguientes
    // llamadas a `cache_dir()` no fallan por permisos si el directorio
    // padre no es escribible.
    if let Ok(iter) = std::fs::read_dir(&root) {
        for entry in iter.flatten() {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(())
}

/// Purga entradas cuyo `.last_used` sea más viejo que `ttl_days`.
/// Devuelve los bytes liberados. Un TTL de 0 se trata como 1 día (para
/// evitar borrar entradas recién tocadas por un race con el drop del
/// StreamHandle).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn prune(ttl_days: u32) -> Result<u64> {
    let root = cache_dir()?;
    let ttl_secs = (ttl_days.max(1) as u64) * 24 * 3600;
    let cutoff = now_unix().saturating_sub(ttl_secs);
    let mut freed = 0u64;
    let Ok(iter) = std::fs::read_dir(&root) else {
        return Ok(0);
    };
    for entry in iter.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let path = entry.path();
        let last_used = entry_last_used(&path);
        if last_used == 0 || last_used >= cutoff {
            continue;
        }
        let size = dir_size_bytes(&path);
        if std::fs::remove_dir_all(&path).is_ok() {
            freed = freed.saturating_add(size);
        }
    }
    Ok(freed)
}

/// Barre `std::env::temp_dir()` en busca de tempdirs huérfanos con
/// nuestros prefijos (`videodrome-hls-*`, `videodrome-stream-*`) y
/// los borra. Se llama al arranque de la app (main.rs y gui.rs::run).
///
/// Motivo (Fase F del audit Windows): en NTFS no se puede borrar un
/// fichero mientras otro handle lo tiene abierto sin
/// `FILE_SHARE_DELETE`. Cuando el `TempDir::drop` corre mientras
/// ffmpeg / axum tienen aún un `.ts` abierto, el unlink falla en
/// silencio y queda basura en `%TEMP%`. En macOS/Linux el unlink
/// procede aunque haya handles abiertos, así que el problema no
/// aparece — pero el barrido cubre también crashes / SIGKILLs en
/// cualquier SO. Barato y seguro: solo borramos directorios con
/// nuestro prefijo, así que no podemos tocar nada del user.
///
/// No propaga errores: silencioso y best-effort. Devuelve el número
/// de directorios borrados (informativo para logs).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn prune_orphan_tempdirs() -> usize {
    const PREFIXES: &[&str] = &["videodrome-hls-", "videodrome-stream-"];
    let temp = std::env::temp_dir();
    let Ok(iter) = std::fs::read_dir(&temp) else {
        return 0;
    };
    let mut count = 0;
    for entry in iter.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !PREFIXES.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        // best-effort: si otro proceso vivo tiene handles abiertos
        // en NTFS puede fallar; en la siguiente ejecución tocará.
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_start_and_end() {
        assert_eq!(parse_range("bytes=100-200"), Some((Some(100), Some(200))));
    }

    #[test]
    fn parse_range_start_open() {
        assert_eq!(parse_range("bytes=1000-"), Some((Some(1000), None)));
    }

    #[test]
    fn parse_range_suffix() {
        assert_eq!(parse_range("bytes=-500"), Some((None, Some(500))));
    }

    #[test]
    fn parse_range_rejects_both_empty() {
        // Necesario para que la rama `Some((None, None))` en
        // `serve_video` sea genuinamente inalcanzable — no relajar
        // sin actualizar el `unreachable!` de allí.
        assert_eq!(parse_range("bytes=-"), None);
    }

    #[test]
    fn parse_range_rejects_missing_prefix() {
        assert_eq!(parse_range("100-200"), None);
    }

    #[test]
    fn parse_range_rejects_non_numeric() {
        assert_eq!(parse_range("bytes=abc-xyz"), None);
    }

    // ── §4 audit series: select_file ─────────────────────────────

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

    #[cfg(feature = "gui")]
    #[test]
    fn is_valid_hls_filename_rejects_playlist() {
        // El playlist va por su propia ruta (`/hls/playlist.m3u8` →
        // `serve_hls_playlist`). Este handler solo debe ver segments.
        assert!(!is_valid_hls_filename("playlist.m3u8"));
        // El `live.m3u8` que escribe ffmpeg tampoco se sirve nunca.
        assert!(!is_valid_hls_filename("live.m3u8"));
    }

    #[cfg(feature = "gui")]
    #[test]
    fn is_valid_hls_filename_accepts_segments() {
        assert!(is_valid_hls_filename("seg-00000.ts"));
        assert!(is_valid_hls_filename("seg-00042.ts"));
        assert!(is_valid_hls_filename("seg-99999.ts"));
        // Longitudes distintas al padding %05d también valen (parseamos
        // el idx como u64 sin exigir 5 dígitos).
        assert!(is_valid_hls_filename("seg-0.ts"));
        assert!(is_valid_hls_filename("seg-1234567.ts"));
    }

    #[cfg(feature = "gui")]
    #[test]
    fn is_valid_hls_filename_rejects_traversal() {
        assert!(!is_valid_hls_filename("../etc/passwd"));
        assert!(!is_valid_hls_filename("..\\etc\\passwd"));
        assert!(!is_valid_hls_filename("seg-00000.ts/../foo"));
        assert!(!is_valid_hls_filename("seg-00000.ts\\foo"));
    }

    #[cfg(feature = "gui")]
    #[test]
    fn is_valid_hls_filename_rejects_wrong_shape() {
        assert!(!is_valid_hls_filename(""));
        assert!(!is_valid_hls_filename("playlist.m3u"));
        assert!(!is_valid_hls_filename("seg-.ts"));
        // El formato antiguo `seg-<sid>-<idx>.ts` YA NO es válido —
        // el modelo VOD estático usa nombres estables sin sid.
        assert!(!is_valid_hls_filename("seg-1-0000.ts"));
        assert!(!is_valid_hls_filename("seg-a.ts"));
        assert!(!is_valid_hls_filename("seg-00000.tsx"));
    }

    #[cfg(feature = "gui")]
    #[test]
    fn parse_seg_idx_extracts_number() {
        assert_eq!(parse_seg_idx("seg-00000.ts"), Some(0));
        assert_eq!(parse_seg_idx("seg-00042.ts"), Some(42));
        assert_eq!(parse_seg_idx("seg-99999.ts"), Some(99999));
        assert_eq!(parse_seg_idx("seg-1234567.ts"), Some(1234567));
        assert_eq!(parse_seg_idx("seg-a.ts"), None);
        assert_eq!(parse_seg_idx("seg-.ts"), None);
        assert_eq!(parse_seg_idx("playlist.m3u8"), None);
    }

    #[cfg(feature = "gui")]
    #[test]
    fn max_produced_idx_ignores_below_floor_and_defaults_below_floor() {
        // Sin ningún fichero producido, el helper devuelve `floor - 1`
        // — de forma que el chequeo `idx > produced + LOOKAHEAD` solo
        // dispara restart cuando el idx pedido está muy por delante.
        let td = tempfile::tempdir().unwrap();
        assert_eq!(max_produced_idx(td.path(), 100), 99);

        // Con segmentos por debajo del floor, se ignoran (son residuos
        // de un job anterior sobre el mismo tempdir compartido).
        std::fs::write(td.path().join("seg-00050.ts"), b"").unwrap();
        std::fs::write(td.path().join("seg-00099.ts"), b"").unwrap();
        assert_eq!(max_produced_idx(td.path(), 100), 99);

        // Con segmentos >= floor, devuelve el máximo.
        std::fs::write(td.path().join("seg-00100.ts"), b"").unwrap();
        std::fs::write(td.path().join("seg-00105.ts"), b"").unwrap();
        std::fs::write(td.path().join("seg-00103.ts"), b"").unwrap();
        assert_eq!(max_produced_idx(td.path(), 100), 105);

        // Ficheros con extensión distinta (.tmp de temp_file, .m3u8)
        // NO cuentan: solo `seg-NNNN.ts` completos.
        std::fs::write(td.path().join("seg-00200.ts.tmp"), b"").unwrap();
        std::fs::write(td.path().join("live.m3u8"), b"").unwrap();
        assert_eq!(max_produced_idx(td.path(), 100), 105);
    }

    #[test]
    fn parse_infohash_reexports_from_torrents() {
        // El helper de stream.rs debe delegar en torrents::parse_infohash
        // (misma normalización → lowercase, misma validación).
        let hash = parse_infohash("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567");
        assert_eq!(hash.unwrap().len(), 40);
    }

    // Tests de persistencia de resume. Operan sobre un tempdir por
    // test vía las variantes `_in` de `save_position`/`load_resume`,
    // así que son portables (macOS/Windows/Linux) y no tocan la
    // caché real del sistema.
    #[cfg(feature = "gui")]
    mod resume_persistence {
        use super::*;

        fn make_entry(base: &std::path::Path, hash: &str) {
            std::fs::create_dir_all(base.join(hash)).unwrap();
        }

        // Wrappers cortos: los tests históricos pasaban seconds+duration.
        // Con §6 audit añadimos file_id + episode. Estos helpers
        // encapsulan file_id=0, episode=None → los tests legacy se
        // leen igual y solo los nuevos usan la firma completa.
        fn save(base: &std::path::Path, hash: &str, seconds: f64, duration: f64) {
            save_position_in(base, hash, 0, seconds, duration, None);
        }
        fn load(base: &std::path::Path, hash: &str) -> Option<Resume> {
            load_resume_in(base, hash, 0)
        }

        #[test]
        fn save_position_writes_seconds_and_duration() {
            let td = tempfile::tempdir().unwrap();
            let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
            make_entry(td.path(), hash);
            save(td.path(), hash, 123.4, 4500.0);
            let r = load(td.path(), hash).unwrap();
            assert_eq!(r.seconds, Some(123.4));
            assert_eq!(r.duration_seconds, Some(4500.0));
        }

        #[test]
        fn save_position_preserves_prior_fraction() {
            let td = tempfile::tempdir().unwrap();
            let hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
            make_entry(td.path(), hash);
            // Simulamos un Drop previo legacy (v1) escribiendo el
            // shape plano. `load_store` lo migra bajo files["0"].
            let path = td.path().join(hash).join(RESUME_FILE);
            std::fs::write(
                &path,
                r#"{"fraction":0.42,"seconds":null,"duration_seconds":null,"updated_at":100}"#,
            )
            .unwrap();
            save(td.path(), hash, 60.0, 3600.0);
            let r = load(td.path(), hash).unwrap();
            assert!(
                (r.fraction - 0.42).abs() < 1e-6,
                "fraction sobrescrita: {r:?}"
            );
            assert_eq!(r.seconds, Some(60.0));
            assert_eq!(r.duration_seconds, Some(3600.0));
        }

        #[test]
        fn save_position_deletes_when_over_completion_threshold() {
            let td = tempfile::tempdir().unwrap();
            let hash = "cccccccccccccccccccccccccccccccccccccccc";
            make_entry(td.path(), hash);
            save(td.path(), hash, 100.0, 1000.0);
            assert!(load(td.path(), hash).is_some());
            save(td.path(), hash, 960.0, 1000.0);
            assert!(load(td.path(), hash).is_none());
        }

        #[test]
        fn save_position_noop_when_entry_dir_missing() {
            let td = tempfile::tempdir().unwrap();
            let hash = "dddddddddddddddddddddddddddddddddddddddd";
            save(td.path(), hash, 30.0, 60.0);
            assert!(load(td.path(), hash).is_none());
        }

        #[test]
        fn save_position_ignores_zero_duration() {
            let td = tempfile::tempdir().unwrap();
            let hash = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
            make_entry(td.path(), hash);
            save(td.path(), hash, 1_000_000.0, 0.0);
            let r = load(td.path(), hash).unwrap();
            assert_eq!(r.seconds, Some(1_000_000.0));
            assert!(r.duration_seconds.is_none());
        }

        #[test]
        fn save_position_deletes_at_exactly_95_percent() {
            let td = tempfile::tempdir().unwrap();
            let hash = "ffffffffffffffffffffffffffffffffffffffff";
            make_entry(td.path(), hash);
            save(td.path(), hash, 50.0, 100.0);
            assert!(load(td.path(), hash).is_some());
            save(td.path(), hash, 95.0, 100.0);
            assert!(load(td.path(), hash).is_none());
        }

        #[test]
        fn save_position_preserves_corrupt_existing_file() {
            let td = tempfile::tempdir().unwrap();
            let hash = "1111111111111111111111111111111111111111";
            make_entry(td.path(), hash);
            let path = td.path().join(hash).join(RESUME_FILE);
            let corrupt = r#"{"fraction":0.42,"seconds":123.4"#;
            std::fs::write(&path, corrupt).unwrap();
            save(td.path(), hash, 999.0, 3600.0);
            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(after, corrupt);
        }

        #[test]
        fn save_position_writes_are_atomic() {
            let td = tempfile::tempdir().unwrap();
            let hash = "2222222222222222222222222222222222222222";
            make_entry(td.path(), hash);
            save(td.path(), hash, 42.0, 3600.0);
            let entry_dir = td.path().join(hash);
            let leftovers: Vec<_> = std::fs::read_dir(&entry_dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s == "tmp")
                })
                .collect();
            assert!(leftovers.is_empty(), "quedaron `.tmp` sin renombrar");
        }

        // ── §6 audit: multi-file ─────────────────────────────────

        #[test]
        fn save_position_isolates_entries_per_file_id() {
            let td = tempfile::tempdir().unwrap();
            let hash = "3333333333333333333333333333333333333333";
            make_entry(td.path(), hash);
            save_position_in(td.path(), hash, 3, 100.0, 3600.0, None);
            save_position_in(td.path(), hash, 5, 200.0, 3600.0, None);
            let r3 = load_resume_in(td.path(), hash, 3).unwrap();
            let r5 = load_resume_in(td.path(), hash, 5).unwrap();
            assert_eq!(r3.seconds, Some(100.0));
            assert_eq!(r5.seconds, Some(200.0));
        }

        #[test]
        fn save_position_completion_removes_only_that_file() {
            // E03 se termina, E02 sigue vivo con su posición.
            let td = tempfile::tempdir().unwrap();
            let hash = "4444444444444444444444444444444444444444";
            make_entry(td.path(), hash);
            save_position_in(td.path(), hash, 2, 300.0, 3600.0, None);
            save_position_in(td.path(), hash, 3, 3500.0, 3600.0, None);
            // E03 (file 3) > 95% → borrado
            assert!(load_resume_in(td.path(), hash, 3).is_none());
            // E02 (file 2) intacto
            let r = load_resume_in(td.path(), hash, 2).unwrap();
            assert_eq!(r.seconds, Some(300.0));
        }

        #[test]
        fn save_position_stores_episode_metadata() {
            let td = tempfile::tempdir().unwrap();
            let hash = "5555555555555555555555555555555555555555";
            make_entry(td.path(), hash);
            let ep = ResumeEpisode {
                season: 2,
                episode: 3,
                tmdb_id: Some(31234),
            };
            save_position_in(td.path(), hash, 3, 100.0, 3600.0, Some(ep));
            let r = load_resume_in(td.path(), hash, 3).unwrap();
            let e = r.episode.expect("episode meta debía persistirse");
            assert_eq!(e.season, 2);
            assert_eq!(e.episode, 3);
            assert_eq!(e.tmdb_id, Some(31234));
        }

        #[test]
        fn load_resume_any_returns_most_recent_by_updated_at() {
            let td = tempfile::tempdir().unwrap();
            let hash = "6666666666666666666666666666666666666666";
            make_entry(td.path(), hash);
            save_position_in(td.path(), hash, 1, 10.0, 3600.0, None);
            std::thread::sleep(std::time::Duration::from_secs(1));
            save_position_in(td.path(), hash, 2, 20.0, 3600.0, None);
            let r = load_resume_any_in(td.path(), hash, None).unwrap();
            assert_eq!(r.seconds, Some(20.0));
        }

        #[test]
        fn load_resume_any_filters_by_episode() {
            let td = tempfile::tempdir().unwrap();
            let hash = "7777777777777777777777777777777777777777";
            make_entry(td.path(), hash);
            let ep_a = ResumeEpisode {
                season: 1,
                episode: 1,
                tmdb_id: None,
            };
            let ep_b = ResumeEpisode {
                season: 2,
                episode: 3,
                tmdb_id: None,
            };
            save_position_in(td.path(), hash, 0, 10.0, 3600.0, Some(ep_a));
            save_position_in(td.path(), hash, 5, 400.0, 3600.0, Some(ep_b));
            let r = load_resume_any_in(td.path(), hash, Some((2, 3))).unwrap();
            assert_eq!(r.seconds, Some(400.0));
            let none = load_resume_any_in(td.path(), hash, Some((9, 9)));
            assert!(none.is_none());
        }

        #[test]
        fn legacy_v1_file_migrates_to_files_zero_on_load() {
            // Un resume.json escrito por el binario antiguo (plano)
            // debe leerse como si estuviera bajo files["0"].
            let td = tempfile::tempdir().unwrap();
            let hash = "8888888888888888888888888888888888888888";
            make_entry(td.path(), hash);
            let path = td.path().join(hash).join(RESUME_FILE);
            std::fs::write(
                &path,
                r#"{"fraction":0.5,"seconds":600.0,"duration_seconds":3600.0,"updated_at":42}"#,
            )
            .unwrap();
            let r = load_resume_in(td.path(), hash, 0).unwrap();
            assert_eq!(r.seconds, Some(600.0));
            assert!((r.fraction - 0.5).abs() < 1e-6);
            // Otros file_ids no matchean.
            assert!(load_resume_in(td.path(), hash, 3).is_none());
        }
    }
}
