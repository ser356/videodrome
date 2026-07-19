//! Handlers HTTP del servidor local de streaming: `/video` (byte
//! ranges), `/probe.json` (ffprobe cacheado), middlewares (CORS +
//! request log), y — en builds gui — `POST /hls/audio` para el
//! cambio de pista + `GET /subs/embedded/<idx>` para extraer subs
//! integrados. También los helpers de parsing (`parse_range`) y los
//! wrappers de I/O (`LimitedRead`, `TracedResponseStream`).
//!
//! Extraído de `stream.rs` en el refactor (commit paso 5). Sin
//! cambios de comportamiento.

use std::sync::atomic::Ordering;

use anyhow::Result;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use tokio::io::{AsyncSeekExt, SeekFrom};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "gui")]
use super::hls::ensure_hls_dir;
#[cfg(feature = "gui")]
use super::state::current_client_capabilities;
use super::state::AppState;

#[allow(unused_imports)]
use crate::winutil::HideConsoleExt;

/// Handler HTTP. Soporta `Range: bytes=X-Y` (200/206). Sin Range devuelve
/// el fichero entero como 200 OK.
pub(super) async fn serve_video(
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
pub(super) async fn add_cors_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
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

/// Middleware que emite un `info!` por cada petición a `/hls/*` con
/// método, ruta y status de la respuesta. Complementa a
/// `add_cors_headers`: se aplica ANTES (queda arriba en la pila de
/// layers) para que el `status` reflejado sea el emitido por el
/// handler (los handlers HLS pueden devolver 200 / 503 / 504 / 500
/// según deadline / stalled / fatal, y sin este log era imposible
/// correlacionar la request del WebView con el `warn!` interno).
#[cfg(feature = "gui")]
pub(super) async fn log_hls_requests(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_string();
    let is_hls = path.starts_with("/hls/");
    if !is_hls {
        return next.run(req).await;
    }
    let method = req.method().clone();
    let resp = next.run(req).await;
    tracing::info!(
        target: "hls-http",
        method = %method,
        path = %path,
        status = resp.status().as_u16(),
        "hls request"
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
pub(super) async fn serve_probe(
    State(state): State<AppState>,
) -> Result<axum::Json<crate::ffmpeg::MediaInfo>, Response> {
    let mut info = match ensure_probe(&state).await {
        Ok(info) => info,
        Err(e) => {
            // Rama estructurada: timeout de ffprobe → 504 +
            // `{reason:"probe_stalled", bytes:0, elapsed_s:N}`.
            // El frontend distingue así "swarm sin seeders" (mensaje
            // "prueba otro release", botón Volver → lista de
            // torrents) de "ffmpeg roto" (mensaje "comprueba
            // ffmpeg"). Antes el timeout se hundía en un 500 con
            // mensaje libre y el frontend no podía diferenciar.
            if let Some(stalled) = e.downcast_ref::<crate::ffmpeg::ProbeStalled>() {
                tracing::warn!(
                    target: "probe",
                    reason = "probe_stalled",
                    elapsed_s = stalled.elapsed_s,
                    "returning 504"
                );
                let body = format!(
                    r#"{{"reason":"probe_stalled","bytes":0,"elapsed_s":{}}}"#,
                    stalled.elapsed_s
                );
                let resp = Response::builder()
                    .status(StatusCode::GATEWAY_TIMEOUT)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap_or_else(|_| Response::new(Body::empty()));
                return Err(resp);
            }
            // Fallo real de ffprobe/ffmpeg (binario ausente, JSON
            // corrupto, permission denied, exit != 0…): log con
            // causa a nivel `error!` y 500 genérico. El frontend
            // mantiene su mensaje "comprueba ffmpeg" en este caso.
            // `?e` usa el Debug de `anyhow::Error` que imprime la
            // cadena completa (`Caused by: …`), a diferencia de
            // `%e` que se queda con el mensaje más externo.
            tracing::error!(
                target: "probe",
                error = ?e,
                "probe failed"
            );
            let msg = e.to_string();
            let resp = Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from(msg))
                .unwrap_or_else(|_| Response::new(Body::empty()));
            return Err(resp);
        }
    };
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
pub(in crate::stream) async fn ensure_probe(state: &AppState) -> Result<crate::ffmpeg::MediaInfo> {
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
pub(super) struct AudioSwitchQuery {
    idx: usize,
}

#[cfg(feature = "gui")]
pub(super) async fn set_hls_audio(
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
pub(super) async fn serve_embedded_subtitle(
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
        // Antes tirábamos el stderr al vacío en la rama 415:
        // devolvíamos "no bitmap sub" sin evidencia real del motivo,
        // así que un fallo distinto (input inaccesible, filter roto)
        // se camuflaba de "unsupported" y no se diagnosticaba nunca.
        // Logueamos la cola completa a `warn!(target: "ffmpeg", ...)`.
        tracing::warn!(
            target: "ffmpeg",
            code = %output.status,
            idx,
            classified = if unsupported { "unsupported" } else { "internal" },
            stderr_tail = %stderr,
            "ffmpeg (subs embedded) exited"
        );
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
}
