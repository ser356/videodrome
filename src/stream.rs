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

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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

const LAST_USED_FILE: &str = ".last_used";
const RESUME_FILE: &str = "resume.json";

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
            eprintln!("[subs] compute_moviehash timeout at 10s (peers lentos)");
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
        if let Some(hash) = self.infohash.as_deref() {
            let max = self.max_seek.load(Ordering::Relaxed);
            if self.file_len > 0 {
                let fraction = (max as f32 / self.file_len as f32).clamp(0.0, 1.0);
                let path = self.data_dir.join(RESUME_FILE);
                let existing = match std::fs::read_to_string(&path) {
                    Err(_) => Some(Resume::default()),
                    Ok(s) => match serde_json::from_str::<Resume>(&s) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            eprintln!(
                                "[resume] Drop: existing file at {} unparseable ({e}); skipping",
                                path.display()
                            );
                            None
                        }
                    },
                };
                if let Some(mut resume) = existing {
                    resume.fraction = fraction;
                    resume.updated_at = now_unix();
                    if let Err(e) = write_resume_atomic(&path, &resume) {
                        eprintln!("[resume] Drop: atomic write failed: {e}");
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
    active_request: Arc<tokio::sync::Mutex<Option<(CancellationToken, tokio::time::Instant)>>>,
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

/// Arranca una sesión BitTorrent para el magnet dado, sirve el fichero
/// principal (el más grande) por HTTP en `127.0.0.1:PORT` y devuelve la
/// URL para el reproductor.
///
/// Si el magnet expone infohash, los datos se guardan en la caché
/// persistente (`<cache>/videodrome/streams/<infohash>/`) — la próxima
/// vez que se abra esta misma peli, librqbit reutiliza los ficheros y
/// arranca casi al instante. Sin infohash, se cae a un tempdir efímero.
pub async fn start(magnet: String) -> Result<StreamHandle> {
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

    // Selecciona el fichero más grande (heurística estándar para películas)
    let (file_id, file_name, file_len) = handle
        .with_metadata(|md| {
            md.file_infos
                .iter()
                .enumerate()
                .max_by_key(|(_, f)| f.len)
                .map(|(i, f)| (i, f.relative_filename.to_string_lossy().into_owned(), f.len))
        })
        .context("No se pudo leer metadata del torrent")?
        .context("Torrent sin ficheros")?;

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
    eprintln!(
        "[video] Range {start}-{end} ({} MB, {:.1}% of file)",
        content_length / 1_048_576,
        (start as f64 / state.file_len as f64) * 100.0
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
        let should_cancel_prev = guard
            .as_ref()
            .map(|(_, started)| started.elapsed().as_millis() >= BURST_WINDOW_MS)
            .unwrap_or(false);
        let now = tokio::time::Instant::now();
        if should_cancel_prev {
            if let Some((prev, _)) = guard.replace((request_token.clone(), now)) {
                prev.cancel();
            }
        } else {
            // No cancelamos (burst reciente o no había previa). Sobrescribimos
            // el slot con el nuestro para que la SIGUIENTE cancele a esta
            // si llega después del burst window.
            *guard = Some((request_token.clone(), now));
        }
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
    let stream = futures::stream::StreamExt::take_until(raw, Box::pin(cancel_fut));
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
    // El playlist es función pura de la duración: no espera a ffmpeg
    // ni respawnea nada. Necesitamos `duration_seconds` del probe; si
    // no está cacheado (raro: el frontend llama a `/probe.json` antes
    // de montar el `<video>`), lo calculamos aquí.
    let info = ensure_probe(&state)
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
    let n = (duration / HLS_SEG_SECS).ceil() as u64;
    if n == 0 {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "peli demasiado corta para HLS".to_string(),
        ));
    }
    // Playlist VOD: todos los segmentos declarados desde el arranque,
    // último EXTINF ajustado al resto real (`duration - (n-1)*4`).
    // `#EXT-X-ENDLIST` presente ⇒ Safari lo trata como VOD puro:
    // barra de progreso completa desde el primer ms y seek nativo a
    // cualquier punto.
    let mut playlist = String::with_capacity(96 + (n as usize) * 32);
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:3\n");
    playlist.push_str(&format!(
        "#EXT-X-TARGETDURATION:{}\n",
        HLS_SEG_SECS.ceil() as u64
    ));
    playlist.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    playlist.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    playlist.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
    for i in 0..n {
        let extinf = if i + 1 == n {
            // Último segmento: la diferencia real, mínimo 0.001s para
            // que no salga 0.00000 (que algunos parsers tratan como
            // segmento vacío).
            (duration - (n - 1) as f64 * HLS_SEG_SECS).max(0.001)
        } else {
            HLS_SEG_SECS
        };
        playlist.push_str(&format!("#EXTINF:{extinf:.5},\nseg-{i:05}.ts\n"));
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    if std::env::var("VIDEODROME_DEBUG").is_ok() {
        eprintln!(
            "[hls] playlist emitted: duration={duration:.3}s n={n} target={}s",
            HLS_SEG_SECS.ceil() as u64
        );
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        // Estable durante la vida del stream (misma duración ⇒ mismo
        // playlist). Dejamos que el WebView lo cachee.
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

/// Garantiza que existe el tempdir compartido del stream HLS. Se
/// crea perezosamente en la primera petición de segmento; sobrevive
/// a reinicios de ffmpeg (todos los jobs del stream escriben aquí,
/// los segmentos son cache para toda la vida del stream).
#[cfg(feature = "gui")]
async fn ensure_hls_dir(state: &AppState) -> Result<PathBuf, (StatusCode, String)> {
    let mut guard = state.hls.lock().await;
    if let Some(hls) = guard.as_ref() {
        return Ok(hls.dir.clone());
    }
    let tempdir = tempfile::Builder::new()
        .prefix("videodrome-hls-")
        .tempdir()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tempdir: {e}")))?;
    let dir = tempdir.path().to_path_buf();
    *guard = Some(HlsState {
        dir: dir.clone(),
        _tempdir: tempdir,
        job: None,
        audio_idx: None,
    });
    Ok(dir)
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
            eprintln!("[hls] TIMEOUT: segmento {file} (idx={idx}) no disponible tras 60s");
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                format!("segmento {file} no disponible tras 60s"),
            ));
        }
        // Log de progreso una única vez tras 5s de espera para
        // detectar spawns lentos sin ensuciar la consola en el caso
        // rápido.
        if !logged_wait && started_at.elapsed().as_secs() >= 5 {
            eprintln!(
                "[hls] waiting for seg-{idx:05}.ts... {}s elapsed",
                started_at.elapsed().as_secs()
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
    let (old_job, dir, audio_idx) = {
        let mut guard = state.hls.lock().await;
        let hls = guard
            .as_mut()
            .expect("dir must be ensured before ensure_hls_job");
        (hls.job.take(), hls.dir.clone(), hls.audio_idx)
    };
    if let Some(mut old) = old_job {
        // Cancelar la Range GET del ffmpeg viejo contra `/video`:
        // axum cierra el body → librqbit libera el FileStream → las
        // piezas priorizadas se liberan para el nuevo.
        {
            let mut req_guard = state.active_request.lock().await;
            if let Some((token, _)) = req_guard.take() {
                token.cancel();
            }
        }
        let kill_started = tokio::time::Instant::now();
        let _ = old.child.kill().await;
        let _ = old.child.wait().await;
        eprintln!(
            "[hls] killed old ffmpeg job (start_idx={}) in {}ms",
            old.start_idx,
            kill_started.elapsed().as_millis()
        );
    }

    // Warm-up de librqbit: si el idx pedido corresponde a un offset
    // alto y las piezas están frías, priorizamos su descarga ANTES
    // de que ffmpeg haga su primer read. Reduce el tiempo hasta el
    // primer segmento típico de 60s → 15-30s en pelis pesadas.
    let start_seconds = idx as f64 * HLS_SEG_SECS;
    if start_seconds > 5.0 {
        warmup_librqbit_for_offset(state, start_seconds).await;
    }

    let child = spawn_hls(state, &dir, idx, audio_idx).await?;
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
        eprintln!("[warmup] skip: no duration cached yet");
        return;
    };
    if duration <= 0.0 {
        return;
    }
    let byte_offset = ((start_seconds / duration) * state.file_len as f64) as u64;
    let byte_offset = byte_offset.min(state.file_len.saturating_sub(1));
    eprintln!(
        "[warmup] priming librqbit at offset {byte_offset} ({:.1}%) for start={start_seconds}s",
        (byte_offset as f64 / state.file_len as f64) * 100.0
    );
    let mut file_stream = match state.handle.clone().stream(state.file_id) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[warmup] librqbit stream failed: {e}");
            return;
        }
    };
    if let Err(e) = file_stream.seek(SeekFrom::Start(byte_offset)).await {
        eprintln!("[warmup] seek failed: {e}");
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
        Ok(Ok(n)) => eprintln!("[warmup] primed {n} byte at offset {byte_offset}"),
        Ok(Err(e)) => eprintln!("[warmup] read err: {e}"),
        Err(_) => eprintln!("[warmup] read timeout at 3s (piezas frías, seguimos)"),
    }
    // NB: NO tocamos `state.max_seek` aquí. Antes lo hacíamos
    // "para que la próxima Range GET real no resetee la prioridad",
    // pero `max_seek` NO influye en la priorización de piezas de
    // librqbit — solo se usa para persistir `resume.json` al drop.
    // Contaminarlo desde un warm-up estimado provocaba que un peek
    // al 90% dejara el resume ahí para siempre, o que el resume
    // avanzase sin que el usuario reprodujese realmente ese offset.
}

/// Spawnea un ffmpeg que producirá `seg-<idx>.ts`, `seg-<idx+1>.ts`,
/// … en `dir` (tempdir compartido). Argv clave:
///
///   * `-ss <idx*4>` antes de `-i`: fast seek por demuxer (keyframe
///     ≤ t). Combinado con `-force_key_frames expr:gte(t,n_forced*4)`
///     que fuerza keyframes en múltiplos exactos de 4s, el primer
///     segmento del job corta en frontera exacta (igual que
///     produciría el job del stream desde t=0). Requisito para que
///     dos jobs distintos produzcan segmentos intercambiables.
///
///   * `-start_number <idx>`: los ficheros se numeran desde el
///     índice global, coincidiendo con los URIs del playlist
///     estático (`seg-<idx>.ts`).
///
///   * `-output_ts_offset <idx*4>`: los PTS del MPEG-TS de salida
///     arrancan en el tiempo absoluto del segmento, no en 0. Sin
///     esto, `currentTime`, subtítulos y timeline se desplazarían
///     tras cada reinicio de ffmpeg.
///
///   * `-hls_flags independent_segments+temp_file+omit_endlist`:
///     `temp_file` es la clave — ffmpeg escribe `seg-NNNNN.ts.tmp`
///     y renombra atómicamente a `.ts` al cerrar. El handler solo
///     ve `.ts` completos, sin heurísticas de tamaño/mtime.
///     `omit_endlist` evita que ffmpeg escriba `#EXT-X-ENDLIST` en
///     `live.m3u8` (que ignoramos — nuestro playlist estático es
///     el único que sirve).
///
///   * `live.m3u8` como output: es el playlist que escribe ffmpeg,
///     se ignora (no se sirve nunca); solo nos interesan los .ts.
///     Es inevitable que el muxer hls lo escriba.
#[cfg(feature = "gui")]
async fn spawn_hls(
    state: &AppState,
    dir: &Path,
    idx: u64,
    audio_idx: Option<usize>,
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
    let start_seconds = idx as f64 * HLS_SEG_SECS;

    let mut cmd = tokio::process::Command::new(bin);
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
    // Video: siempre transcode a H.264 8-bit High single-slice. HLS
    // TS segments no soportan HEVC en Safari <14 y añade complejidad;
    // libx264 veryfast es suficiente para 1080p en cualquier M-series.
    cmd.arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg("23")
        .arg("-profile:v")
        .arg("high")
        .arg("-level:v")
        .arg("4.1")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-bf")
        .arg("0")
        // Keyframes forzados en múltiplos exactos de 4s (0, 4, 8, ...).
        // Requisito para que dos jobs distintos (uno desde 0, otro
        // desde `-ss 1728`) corten segmentos en las mismas fronteras
        // temporales, y por tanto sean intercambiables.
        .arg("-force_key_frames")
        .arg("expr:gte(t,n_forced*4)")
        .arg("-x264-params")
        .arg("scenecut=0:slices=1:sliced-threads=0")
        // Reset de timestamps al mínimo tras el input (combina con
        // `+genpts`). El `-output_ts_offset` de abajo reintroduce
        // el tiempo absoluto en el mux de salida.
        .arg("-avoid_negative_ts")
        .arg("make_zero");
    // Audio: siempre AAC 192k (TS puede llevar AAC ADTS, muy compatible).
    // Aunque el input ya sea AAC, remuxar TS con `-c:a copy` a veces
    // da bitstream errors por packet alignment.
    cmd.arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-ac")
        .arg("2");
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
                eprintln!("[ffmpeg-hls] {line}");
            }
        });
    }
    eprintln!(
        "[hls] spawned ffmpeg: idx={idx} start={start_seconds}s dir={}",
        dir.display()
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
    let info = ensure_probe(&state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
            if let Some((token, _)) = req_guard.take() {
                token.cancel();
            }
        }
        let _ = old.child.kill().await;
        let _ = old.child.wait().await;
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

    let output = tokio::process::Command::new(bin)
        .arg("-hide_banner")
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
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("spawn ffmpeg: {e}"),
            )
        })?;

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
        let _ = tokio::process::Command::new("taskkill")
            .args(["/IM", "vlc.exe", "/T"])
            .status()
            .await;
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

/// Lee el `resume.json` de una entrada. Devuelve `None` si no existe o
/// está corrupto.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn load_resume(infohash: &str) -> Option<Resume> {
    load_resume_in(&cache_dir().ok()?, infohash)
}

/// Variante testeable: opera sobre un directorio base explícito
/// (`<base>/<infohash>/resume.json`) en vez de resolver `cache_dir()`.
/// La versión pública lo llama con el resultado de `cache_dir()`.
fn load_resume_in(base: &Path, infohash: &str) -> Option<Resume> {
    let path = base.join(infohash).join(RESUME_FILE);
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Persiste una posición reportada por el player HTML. Merge-style:
/// si ya existe un `resume.json` (con `fraction` puesto por el Drop
/// anterior), preservamos ese campo y solo actualizamos `seconds` +
/// `duration_seconds` + `updated_at`.
///
/// Si la posición reportada supera `COMPLETION_THRESHOLD` (95%) del
/// runtime, borramos el resume: la peli está vista, la próxima
/// apertura empieza limpia sin preguntar por los créditos.
///
/// Errores silenciosos (log a stderr): el flujo del player no debe
/// romperse porque no podamos persistir una posición.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn save_position(infohash: &str, seconds: f64, duration_seconds: f64) {
    let Ok(base) = cache_dir() else {
        return;
    };
    save_position_in(&base, infohash, seconds, duration_seconds);
}

/// Variante testeable: idem `save_position` sobre un base dir
/// explícito. Los tests pueden crear un tempdir y llamar aquí sin
/// tocar la caché real del sistema (portable a macOS/Windows, donde
/// `dirs::cache_dir` no respeta `XDG_CACHE_HOME`).
fn save_position_in(base: &Path, infohash: &str, seconds: f64, duration_seconds: f64) {
    let entry = base.join(infohash);
    // Si la entrada no existe (magnet nunca reproducido en persistente,
    // o purgada por el prune), no la creamos aquí — el StreamHandle
    // vivo la habría creado al arrancar.
    if !entry.exists() {
        return;
    }
    let path = entry.join(RESUME_FILE);

    // Regla de completado: si `seconds/duration > 0.95`, borrar y
    // cortar. El check requiere una duración conocida > 0 — si el
    // player nos manda `duration_seconds = 0` (ffprobe falló, live
    // stream), no aplicamos la regla.
    if duration_seconds > 0.0 && seconds / duration_seconds >= COMPLETION_THRESHOLD {
        let _ = std::fs::remove_file(&path);
        return;
    }

    // Merge-style con resiliencia a corrupción: si el fichero existe
    // pero no parsea (write parcial de una sesión previa que murió a
    // mitad), NO lo sobreescribimos — perder datos malos es peor que
    // preservar la posibilidad de recuperación manual. Solo tratamos
    // "no existe" como "arranca de cero".
    let mut resume = match std::fs::read_to_string(&path) {
        Err(_) => Resume::default(),
        Ok(s) => match serde_json::from_str::<Resume>(&s) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "[resume] existing file at {} unparseable ({e}); skipping to preserve",
                    path.display()
                );
                return;
            }
        },
    };
    resume.seconds = Some(seconds.max(0.0));
    if duration_seconds > 0.0 {
        resume.duration_seconds = Some(duration_seconds);
    }
    resume.updated_at = now_unix();
    if let Err(e) = write_resume_atomic(&path, &resume) {
        eprintln!("[resume] failed to persist position: {e}");
    }
}

/// Escribe `resume.json` atómicamente: primero a `<file>.tmp`, luego
/// `rename` sobre el destino. Evita que un crash o Cmd+Q a mitad de
/// escritura deje el fichero truncado (que la próxima `load_resume`
/// interpretaría como corrupto y descartaría el resume entero).
///
/// El rename es atómico en POSIX y en NTFS (Windows). No cross-device
/// (tmp y destino en el mismo dir), así que no falla por EXDEV.
fn write_resume_atomic(path: &Path, resume: &Resume) -> std::io::Result<()> {
    let json = serde_json::to_string(resume)
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

        #[test]
        fn save_position_writes_seconds_and_duration() {
            let td = tempfile::tempdir().unwrap();
            let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
            make_entry(td.path(), hash);
            save_position_in(td.path(), hash, 123.4, 4500.0);
            let r = load_resume_in(td.path(), hash).unwrap();
            assert_eq!(r.seconds, Some(123.4));
            assert_eq!(r.duration_seconds, Some(4500.0));
        }

        #[test]
        fn save_position_preserves_prior_fraction() {
            let td = tempfile::tempdir().unwrap();
            let hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
            make_entry(td.path(), hash);
            // Simulamos un Drop previo escribiendo fraction directo.
            let path = td.path().join(hash).join(RESUME_FILE);
            std::fs::write(
                &path,
                r#"{"fraction":0.42,"seconds":null,"duration_seconds":null,"updated_at":100}"#,
            )
            .unwrap();
            save_position_in(td.path(), hash, 60.0, 3600.0);
            let r = load_resume_in(td.path(), hash).unwrap();
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
            save_position_in(td.path(), hash, 100.0, 1000.0);
            assert!(load_resume_in(td.path(), hash).is_some());
            save_position_in(td.path(), hash, 960.0, 1000.0);
            assert!(load_resume_in(td.path(), hash).is_none());
        }

        #[test]
        fn save_position_noop_when_entry_dir_missing() {
            let td = tempfile::tempdir().unwrap();
            let hash = "dddddddddddddddddddddddddddddddddddddddd";
            save_position_in(td.path(), hash, 30.0, 60.0);
            assert!(load_resume_in(td.path(), hash).is_none());
        }

        #[test]
        fn save_position_ignores_zero_duration() {
            // duration=0 → no aplicamos regla de completado
            // (evita división por cero) y tampoco escribimos el
            // campo duration_seconds.
            let td = tempfile::tempdir().unwrap();
            let hash = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
            make_entry(td.path(), hash);
            save_position_in(td.path(), hash, 1_000_000.0, 0.0);
            let r = load_resume_in(td.path(), hash).unwrap();
            assert_eq!(r.seconds, Some(1_000_000.0));
            assert!(r.duration_seconds.is_none());
        }

        #[test]
        fn save_position_deletes_at_exactly_95_percent() {
            // Boundary: >=0.95 borra (95.0/100.0 == 0.95 exacto).
            let td = tempfile::tempdir().unwrap();
            let hash = "ffffffffffffffffffffffffffffffffffffffff";
            make_entry(td.path(), hash);
            save_position_in(td.path(), hash, 50.0, 100.0);
            assert!(load_resume_in(td.path(), hash).is_some());
            save_position_in(td.path(), hash, 95.0, 100.0);
            assert!(load_resume_in(td.path(), hash).is_none());
        }

        #[test]
        fn save_position_preserves_corrupt_existing_file() {
            // Si el fichero está corrupto (write parcial anterior),
            // NO lo sobreescribimos: perder datos malos es peor que
            // preservar la posibilidad de recuperación manual. La
            // corrupción se produce en la práctica por Cmd+Q entre
            // el `write` y el `rename` de una versión antigua, o por
            // un crash de FS.
            let td = tempfile::tempdir().unwrap();
            let hash = "1111111111111111111111111111111111111111";
            make_entry(td.path(), hash);
            let path = td.path().join(hash).join(RESUME_FILE);
            let corrupt = r#"{"fraction":0.42,"seconds":123.4"#; // sin cerrar
            std::fs::write(&path, corrupt).unwrap();
            save_position_in(td.path(), hash, 999.0, 3600.0);
            // El fichero corrupto sigue tal cual (no lo hemos tocado).
            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(after, corrupt);
        }

        #[test]
        fn save_position_writes_are_atomic() {
            // Tras un write válido no debe quedar `.tmp` en el dir:
            // el rename atómico consume el fichero temporal.
            let td = tempfile::tempdir().unwrap();
            let hash = "2222222222222222222222222222222222222222";
            make_entry(td.path(), hash);
            save_position_in(td.path(), hash, 42.0, 3600.0);
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
    }
}
