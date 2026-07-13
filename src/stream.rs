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
        .prefix("letterboxd-stream-")
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
                .map(|(i, f)| {
                    (
                        i,
                        f.relative_filename.to_string_lossy().into_owned(),
                        f.len,
                    )
                })
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
    };
    let app = Router::new().route("/video", get(serve_video)).with_state(state);

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

    let (start, end) = match range {
        Some((s, Some(e))) => (s, e.min(state.file_len - 1)),
        Some((s, None)) => (s, state.file_len - 1),
        None => (0, state.file_len - 1),
    };

    if start >= state.file_len {
        return Err((
            StatusCode::RANGE_NOT_SATISFIABLE,
            format!("Range {start} >= {}", state.file_len),
        ));
    }

    let content_length = end - start + 1;

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

    // Convierte AsyncRead en un Stream<Item=Bytes> con límite.
    let limited = LimitedRead {
        inner: file_stream,
        remaining: content_length,
    };
    let stream = tokio_util::io::ReaderStream::with_capacity(limited, 64 * 1024);
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

/// Parsea `bytes=START-END` o `bytes=START-`. Solo soportamos un rango.
fn parse_range(v: &str) -> Option<(u64, Option<u64>)> {
    let rest = v.strip_prefix("bytes=")?;
    let (start, end) = rest.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = end.trim();
    let end = if end.is_empty() {
        None
    } else {
        Some(end.parse().ok()?)
    };
    Some((start, end))
}

/// Abre la URL con VLC y devuelve un flag que se pone a `false` cuando el
/// reproductor termina — así la TUI puede detectar que el usuario cerró VLC
/// y liberar el stream automáticamente.
///
/// * macOS: `open -W -a VLC <url>` — `-W` hace que `open` bloquee hasta que
///   VLC salga del todo (⌘Q). Cerrar solo la ventana no cuenta (patrón
///   estándar macOS: la app sigue viva). Si VLC no está instalado, cae a
///   `open <url>` (sin `-W`, el flag queda `false` inmediatamente).
/// * Linux: `vlc <url>` directo — el `Child` ES VLC, así que `wait()` sobre
///   él detecta el cierre. Fallback a `xdg-open`.
/// * Windows: `start /wait vlc <url>` — bloquea hasta que VLC cierre.
///
/// Si no se puede lanzar ningún reproductor, el flag queda en `false` para
/// que la TUI limpie el stream inmediatamente en lugar de dejarlo colgando.
pub fn open_in_vlc(url: &str) -> Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::{AtomicBool, Ordering};

    let alive = Arc::new(AtomicBool::new(true));

    let child_result: std::io::Result<tokio::process::Child> = {
        #[cfg(target_os = "macos")]
        {
            let vlc = tokio::process::Command::new("open")
                .args(["-W", "-a", "VLC", url])
                .spawn();
            match vlc {
                Ok(c) => Ok(c),
                Err(_) => tokio::process::Command::new("open").arg(url).spawn(),
            }
        }
        #[cfg(target_os = "linux")]
        {
            let vlc = tokio::process::Command::new("vlc").arg(url).spawn();
            match vlc {
                Ok(c) => Ok(c),
                Err(_) => tokio::process::Command::new("xdg-open").arg(url).spawn(),
            }
        }
        #[cfg(target_os = "windows")]
        {
            tokio::process::Command::new("cmd")
                .args(["/C", "start", "/wait", "vlc", url])
                .spawn()
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
