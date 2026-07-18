//! Estado compartido entre los handlers HTTP del stream + capacidades
//! del cliente HLS/direct. Extraído de `stream.rs` en el refactor
//! (commit paso 3). Sin cambios de comportamiento.

#[cfg(feature = "gui")]
use std::collections::VecDeque;
use std::net::SocketAddr;
#[cfg(feature = "gui")]
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
#[cfg(feature = "gui")]
use std::sync::Mutex as StdMutex;
#[cfg(feature = "gui")]
use std::sync::{OnceLock, RwLock};

use librqbit::ManagedTorrent;
use tokio_util::sync::CancellationToken;

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

#[derive(Clone)]
pub(super) struct AppState {
    pub(super) handle: Arc<ManagedTorrent>,
    pub(super) file_id: usize,
    pub(super) file_len: u64,
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
    pub(super) active_request:
        Arc<tokio::sync::Mutex<Option<(u64, CancellationToken, tokio::time::Instant)>>>,
    /// Contador atómico de peticiones a `/video` en la vida del
    /// stream. Se incrementa una vez por request y el valor se usa
    /// como `req_id` en el log (`req#N`) y en el slot
    /// `active_request` para poder loguear `cancelled_prev=<id>` sin
    /// pasar el id de forma explícita entre handlers. Overflow real
    /// después de 2^64 peticiones — ~584 años a 1e9 req/s.
    pub(super) request_counter: Arc<AtomicU64>,
    /// Compartido con `StreamHandle`. Se actualiza en cada Range con
    /// start explícito (fetch_max) para trackear la posición de
    /// reproducción alcanzada — usada para persistir `resume.json`.
    pub(super) max_seek: Arc<AtomicU64>,
    /// Addr del listener local — necesario para que los handlers del
    /// player HTML (`/probe.json`, `/play.mp4`) construyan la URL que
    /// pasan a ffprobe/ffmpeg como input (`http://127.0.0.1:PORT/video`).
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub(super) local_addr: SocketAddr,
    /// Caché de `ffprobe` sobre el input. Se popula la primera vez que
    /// se pide `/probe.json` o `/play.mp4` y se reutiliza — ffprobe
    /// tarda 1-3s con Range requests sobre el stream de librqbit, no
    /// queremos pagarlo en cada seek.
    #[cfg(feature = "gui")]
    pub(super) cached_probe: Arc<tokio::sync::Mutex<Option<crate::ffmpeg::MediaInfo>>>,
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
    pub(super) hls: Arc<tokio::sync::Mutex<Option<HlsState>>>,
}

/// Estado HLS compartido durante la vida del stream. El `dir` /
/// `_tempdir` viven aquí (NO en `HlsJob`) porque queremos que los
/// segmentos producidos por un job sigan siendo cache válido para
/// el resto del stream — un seek hacia atrás a zona ya transcodeada
/// se sirve del disco sin respawn de ffmpeg.
#[cfg(feature = "gui")]
pub(super) struct HlsState {
    /// Tempdir compartido. Todos los segmentos `seg-NNNNN.ts` viven
    /// aquí, producidos por cualquier job durante la vida del
    /// stream.
    pub(super) dir: PathBuf,
    pub(super) _tempdir: tempfile::TempDir,
    /// Job ffmpeg activo, si lo hay. `None` cuando no hay ninguna
    /// transcodificación en curso (todos los segmentos pedidos
    /// están ya en disco).
    pub(super) job: Option<HlsJob>,
    /// Índice de stream de audio del INPUT que ffmpeg mapea a la
    /// salida. `None` = ffmpeg auto-selecciona (0:a:0 por defecto).
    /// Cuando el user cambia de pista vía `POST /hls/audio`, matamos
    /// el job activo, purgamos segmentos y guardamos aquí la nueva
    /// selección; el próximo respawn usa `-map 0:v:0 -map 0:a:<idx>`.
    pub(super) audio_idx: Option<usize>,
    /// Estrategia decidida al init: Copy (remux -c:v copy, cero
    /// pérdida) o Transcode (libx264 CRF 18). Audit §2/§7. La
    /// decisión mira el probe + client caps + preferences y se
    /// congela para toda la vida del stream — un cambio de
    /// preferencia NO afecta a un stream ya arrancado.
    pub(super) mode: HlsMode,
    /// Rejilla de segmentos: para cada idx, `(start_seconds,
    /// duration_seconds)`. En modo Transcode todos duran
    /// `HLS_SEG_SECS`; en modo Copy la rejilla es variable y
    /// viene del `KeyframeIndex.variable_segments()` — los cortes
    /// caen en keyframes reales del archivo (audit §2b).
    pub(super) segments: Vec<(f64, f64)>,
    /// Último idx pedido por `serve_hls_segment`. La tarea de
    /// eviction LRU lo usa como playhead para decidir qué
    /// segmentos son "lejanos" y candidatos a borrar (audit §6).
    /// Inicializa a 0 (arranque) y avanza monótono con seek
    /// forward + oscila con scrubbing. Cero coste de sincronía
    /// (atomic).
    pub(super) last_requested_idx: Arc<AtomicU64>,
    /// Handle a la tarea de eviction para poder abortarla al drop
    /// del stream. `None` si el budget es 0 (evicción desactivada).
    pub(super) _evictor: Option<tokio::task::JoinHandle<()>>,
    /// Sticky failure: si algún spawn de ffmpeg murió en <500ms
    /// con exit code != 0, guardamos aquí el mensaje del último
    /// error. Todos los `serve_hls_segment` siguientes devuelven
    /// 500 con ese mensaje SIN respawnear ffmpeg, hasta que el
    /// user cierre el player. Necesario para no entrar en loop
    /// infinito cuando el argv es inválido (filter missing,
    /// codec sin soporte, etc.).
    pub(super) fatal_error: Option<String>,
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
pub(super) enum HlsMode {
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
pub(super) struct HlsJob {
    pub(super) child: tokio::process::Child,
    /// Primer índice de segmento que produce este job. Los ficheros
    /// que emite son `seg-<start_idx>.ts`, `seg-<start_idx+1>.ts`,
    /// etc. Se compara con el idx pedido en cada request para
    /// decidir si el job actual puede servirlo (dentro de la
    /// ventana) o hay que reiniciar en el idx pedido.
    pub(super) start_idx: u64,
    /// Cancela la tarea de warm-up asociada al job (audit §2). El
    /// warm-up corre en paralelo con ffmpeg (NUNCA bloquea el spawn)
    /// y su único efecto secundario es la priorización de piezas en
    /// librqbit. Cuando reemplazamos el job (seek fuera de ventana o
    /// audio switch), cancelamos también su warm-up para no dejar un
    /// FileStream vivo compitiendo con el del nuevo ffmpeg.
    pub(super) warmup_cancel: Option<CancellationToken>,
    /// Últimas ~60 líneas de `child.stderr` capturadas por la task
    /// lectora spawneada en `spawn_hls`. Se consulta cuando el
    /// proceso sale con código ≠ 0 para poder loguear el motivo real
    /// (`ffmpeg` con `-loglevel error` emite solo lo importante).
    /// Antes de esto la salida del proceso se descartaba al reventar,
    /// dejando el log con "ffmpeg exited" sin diagnóstico.
    pub(super) stderr_tail: Arc<StdMutex<VecDeque<String>>>,
}

/// Duración fija de cada segmento HLS, en segundos. Debe coincidir
/// con `-hls_time` y con `-force_key_frames expr:gte(t,n_forced*4)`
/// del spawn de ffmpeg — el conjunto es lo que garantiza que dos
/// jobs distintos produzcan segmentos intercambiables en las
/// mismas fronteras temporales.
#[cfg(feature = "gui")]
pub(super) const HLS_SEG_SECS: f64 = 4.0;

/// Cuántos segmentos por delante del último producido tolera el job
/// activo antes de considerar la petición un seek hacia adelante y
/// reiniciar en el idx pedido. `6 × 4s = 24s` de headroom: un
/// scrubbing rápido dentro de esa ventana espera al job actual (ya
/// está transcodeando cerca), un salto mayor respawnea.
#[cfg(feature = "gui")]
pub(super) const HLS_LOOKAHEAD: u64 = 6;
