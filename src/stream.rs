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
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Persistir resume ANTES de cancelar la sesión — la escritura es
        // síncrona y solo toca `<data_dir>/resume.json`, que no depende
        // del motor de librqbit.
        if let Some(hash) = self.infohash.as_deref() {
            let max = self.max_seek.load(Ordering::Relaxed);
            if self.file_len > 0 {
                let fraction =
                    (max as f32 / self.file_len as f32).clamp(0.0, 1.0);
                let resume = Resume {
                    fraction,
                    updated_at: now_unix(),
                };
                if let Ok(json) = serde_json::to_string(&resume) {
                    let _ = std::fs::write(self.data_dir.join(RESUME_FILE), json);
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
    /// Token de la petición HTTP en curso para este stream. Cuando llega
    /// una nueva Range request (típicamente porque VLC ha hecho seek),
    /// cancelamos la anterior aquí antes de crear la nueva. Sin esto el
    /// FileStream antiguo sigue vivo dentro del `body` de axum — y
    /// librqbit intercala pieces de todos los FileStreams activos, con lo
    /// que el nuevo (el que VLC está esperando) solo se lleva la mitad
    /// del ancho de banda. Resultado: buffering infinito tras cada seek.
    active_request: Arc<tokio::sync::Mutex<Option<CancellationToken>>>,
    /// Compartido con `StreamHandle`. Se actualiza en cada Range con
    /// start explícito (fetch_max) para trackear la posición de
    /// reproducción alcanzada — usada para persistir `resume.json`.
    max_seek: Arc<AtomicU64>,
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
    };
    let max_seek = state.max_seek.clone();
    let app = Router::new()
        .route("/video", get(serve_video))
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
        Some((None, None)) => (0, state.file_len - 1),
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

    // Cancela la petición HTTP anterior antes de arrancar la nueva. Así
    // el FileStream viejo se dropea y librqbit deja de repartir ancho de
    // banda con él — véase el comentario de `active_request` en `AppState`.
    let request_token = CancellationToken::new();
    {
        let mut guard = state.active_request.lock().await;
        if let Some(prev) = guard.replace(request_token.clone()) {
            prev.cancel();
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
            let mut cmd = tokio::process::Command::new("cmd");
            let mut args: Vec<&str> = vec!["/C", "start", "/wait", "vlc", url];
            if let Some(a) = sub_arg.as_deref() {
                args.push(a);
            }
            if let Some(a) = start_arg.as_deref() {
                args.push(a);
            }
            cmd.args(&args);
            cmd.spawn()
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

// ============================================================================
// Caché persistente de streams
// ============================================================================

/// Estado de resume persistido en `<data_dir>/resume.json`. `fraction`
/// es la relación `max_seek_bytes / file_len`, en [0.0, 1.0]. La GUI
/// convierte a segundos multiplicando por el runtime de TMDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resume {
    pub fraction: f32,
    pub updated_at: u64,
}

/// Directorio raíz de la caché de streams:
/// `<dirs::cache_dir>/videodrome/streams/`. Se crea si no existe.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn cache_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .context("No se puede obtener el directorio de caché del sistema")?
        .join("videodrome")
        .join("streams");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("No se pudo crear {}", dir.display()))?;
    Ok(dir)
}

/// Extrae el infohash (`xt=urn:btih:XXXX`) de un magnet, normalizado a
/// minúsculas. Acepta hex de 40 chars y base32 de 32 chars. Devuelve
/// `None` si el magnet no tiene `xt=urn:btih:` reconocible — típico en
/// links raros; el caller cae a tempdir efímero en ese caso.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn parse_infohash(magnet: &str) -> Option<String> {
    let rest = magnet.strip_prefix("magnet:?")?;
    for pair in rest.split('&') {
        let Some(v) = pair.strip_prefix("xt=urn:btih:") else {
            continue;
        };
        let hash = v.to_ascii_lowercase();
        let is_hex40 = hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit());
        let is_b32 =
            hash.len() == 32 && hash.chars().all(|c| c.is_ascii_alphanumeric());
        if is_hex40 || is_b32 {
            return Some(hash);
        }
    }
    None
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
    let path = cache_dir().ok()?.join(infohash).join(RESUME_FILE);
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
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
