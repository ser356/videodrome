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
//! El fichero físico se escribe a un directorio temporal que se limpia al
//! salir. No es sub-almacenamiento cero, pero sí efímero.

use std::net::SocketAddr;
use std::sync::Arc;

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
use tempfile::TempDir;
use tokio::io::{AsyncSeekExt, SeekFrom};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Handle de una sesión de streaming activa. `Drop` cancela el servidor
/// HTTP, detiene la sesión BitTorrent y borra el directorio temporal.
pub struct StreamHandle {
    pub url: String,
    pub file_name: String,
    handle: Arc<ManagedTorrent>,
    cancel: CancellationToken,
    _session: Arc<Session>,
    _tempdir: TempDir,
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
pub async fn start(magnet: String) -> Result<StreamHandle> {
    let tempdir = tempfile::Builder::new()
        .prefix("videodrome-stream-")
        .tempdir()
        .context("No se pudo crear directorio temporal")?;

    // Un solo cancellation token para toda la sesión: se propaga al motor
    // librqbit (DHT, listeners TCP/UDP, tareas de fondo) y al servidor axum.
    // Sin esto, al hacer Drop del StreamHandle el DHT persistía en un
    // puerto UDP fijo y el siguiente `Session::new` fallaba con "address
    // already in use" hasta que el proceso se reiniciaba.
    let cancel = CancellationToken::new();

    let session = Session::new_with_opts(
        tempdir.path().to_path_buf(),
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
    };
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
        handle,
        cancel,
        _session: session,
        _tempdir: tempdir,
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

/// Abre la URL con VLC y devuelve un flag que se pone a `false` cuando el
/// reproductor termina — así la TUI puede detectar que el usuario cerró VLC
/// y liberar el stream automáticamente.
///
/// Si `sub_path` está informado, se le pasa a VLC como `--sub-file=…` para
/// que arranque con los subtítulos ya cargados.
///
/// * macOS: `open -W -a VLC --args <url> [--sub-file=<path>]` — `-W` hace
///   que `open` bloquee hasta que VLC salga del todo (⌘Q). Cerrar solo la
///   ventana no cuenta (patrón estándar macOS). Si VLC no está instalado,
///   cae a `open <url>` (sin `-W`; el flag queda `false` inmediatamente).
/// * Linux: `vlc <url> [--sub-file=<path>]` directo. Fallback a
///   `xdg-open` (que no puede pasar subs).
/// * Windows: `start /wait vlc <url> [--sub-file=<path>]` — bloquea hasta
///   que VLC cierre.
///
/// Si no se puede lanzar ningún reproductor, el flag queda en `false` para
/// que la TUI limpie el stream inmediatamente en lugar de dejarlo colgando.
pub fn open_in_vlc(
    url: &str,
    sub_path: Option<&std::path::Path>,
) -> Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::{AtomicBool, Ordering};

    let alive = Arc::new(AtomicBool::new(true));

    // Preparamos el arg de sub UNA sola vez para no repetir la lógica en
    // cada rama de SO. VLC acepta `--sub-file=/ruta/absoluta.srt`.
    let sub_arg: Option<String> = sub_path.map(|p| format!("--sub-file={}", p.display()));

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
            cmd.spawn()
        }
        #[cfg(target_os = "windows")]
        {
            let mut cmd = tokio::process::Command::new("cmd");
            let mut args: Vec<&str> = vec!["/C", "start", "/wait", "vlc", url];
            if let Some(a) = sub_arg.as_deref() {
                args.push(a);
            }
            cmd.args(&args);
            cmd.spawn()
        }
    };

    match child_result {
        Ok(mut child) => {
            let alive_task = alive.clone();
            tokio::spawn(async move {
                let _ = child.wait().await;
                alive_task.store(false, Ordering::Relaxed);
            });
        }
        Err(_) => {
            // Nada se pudo lanzar → marca como no vivo para que el caller no
            // se quede pensando que está streamando eternamente.
            alive.store(false, Ordering::Relaxed);
        }
    }

    alive
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
