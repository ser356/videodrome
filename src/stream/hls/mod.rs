//! Sub-módulo HLS: playlist + segmentos + pipeline ffmpeg. Extraído
//! de `stream.rs` en el refactor. Contiene los handlers HTTP
//! (`serve_hls_playlist`, `serve_hls_segment`, `ensure_hls_dir`) más
//! los helpers de nombres (`parse_seg_idx`, `is_valid_hls_filename`,
//! `max_produced_idx`). Los sub-submódulos cubren `argv` (transcode
//! de audio + probes HDR), `evict` (LRU cleanup), `grid` (decisión
//! copy/transcode + rejilla de segmentos) y `job` (ciclo de vida
//! del `Child` de ffmpeg).

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::Response;

use super::state::{AppState, HlsState};

pub(super) mod argv;
pub(super) mod evict;
pub(super) mod grid;
mod job;
mod throttle;

/// Parsea `seg-NNNNN.ts` → `NNNNN` como u64. `None` si el nombre no
/// respeta el formato exacto (validación fuerte, path traversal-safe).
pub(in crate::stream) fn parse_seg_idx(name: &str) -> Option<u64> {
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
pub(in crate::stream) fn is_valid_hls_filename(name: &str) -> bool {
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
pub(in crate::stream) fn max_produced_idx(dir: &Path, floor: u64) -> u64 {
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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use self::evict::spawn_lru_evictor;
use self::grid::decide_mode_and_segments;
use self::job::{ensure_hls_job, snapshot_stderr_tail};
use super::server::ensure_probe;
use super::state::{current_client_capabilities, HLS_LOOKAHEAD, HLS_SEG_SECS};

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

pub(super) async fn serve_hls_playlist(
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
        // `no-store`: al hacer `POST /hls/audio` el backend purga
        // los `.ts` y respawnea ffmpeg con la nueva pista, pero los
        // clientes (WKWebView/AVFoundation y hls.js vía fetch)
        // seguían sirviendo los segmentos viejos del HTTP cache si
        // aquí poníamos `max-age=3600`. Como el playlist es una
        // función pura barata (~ms) y hls.js/AVFoundation tienen su
        // propio buffer en memoria para el rewind cercano, el hit
        // de perf de saltarse la cache HTTP es despreciable.
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(playlist.into_bytes()))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
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
pub(super) async fn ensure_hls_dir(state: &AppState) -> Result<PathBuf, (StatusCode, String)> {
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

pub(super) async fn serve_hls_segment(
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

    // Deadline sensible al progreso (audit §3.a). Dos condiciones
    // de salida:
    //
    //   * HARD_DEADLINE (120 s): tope duro. Si tras 2 min no ha
    //     aparecido el .ts, algo está muy mal y devolvemos 504.
    //
    //   * STALL_TIMEOUT (15 s sin progreso de descarga): si la
    //     telemetría de librqbit reporta CERO bytes nuevos durante
    //     15 s seguidos, respondemos 503 con JSON detallado —
    //     `{reason:"swarm_stalled", downloaded_pct, speed_bps,
    //     peers}`. El frontend distingue esto de un error genérico
    //     y pinta un mensaje honesto ("descarga a X kB/s, prueba
    //     otro release o VLC").
    //
    // Mientras haya progreso (aunque sea lento), esperamos:
    // reproduce lo mismo que VLC hace con enjambres modestos. El
    // usuario ve el overlay de arranque con la velocidad real.
    const HARD_DEADLINE_SECS: u64 = 120;
    const STALL_TIMEOUT_SECS: u64 = 15;
    let started_at = tokio::time::Instant::now();
    let hard_deadline = started_at + std::time::Duration::from_secs(HARD_DEADLINE_SECS);
    let initial_stats = state.handle.stats();
    let mut last_progress_bytes = initial_stats.progress_bytes;
    let mut last_progress_at = started_at;
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
                    // ffmpeg vivo? `try_wait` reap-ea el status si el
                    // proceso ya salió; capturamos ese status ANTES de
                    // marcar el job para respawn, porque el segundo
                    // `try_wait` sobre un child ya reap-eado devolvería
                    // `Ok(None)` y perderíamos el código + el motivo.
                    let wait_result = job.child.try_wait();
                    let alive = matches!(wait_result, Ok(None));
                    if !alive {
                        if let Ok(Some(status)) = wait_result {
                            if !status.success() {
                                let tail = snapshot_stderr_tail(&job.stderr_tail);
                                tracing::warn!(
                                    target: "ffmpeg",
                                    code = %status,
                                    stderr_tail = %tail,
                                    start_idx = job.start_idx,
                                    requested_idx = idx,
                                    "ffmpeg (hls) exited unexpectedly"
                                );
                            }
                        }
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

        // Snapshot de progreso: si librqbit sigue bajando bytes,
        // reseteamos el reloj de stall. `progress_bytes` cuenta
        // TODO el fichero, no solo las piezas del segmento — es OK:
        // basta con que el swarm dé cualquier bit para saber que
        // está vivo.
        let stats = state.handle.stats();
        if stats.progress_bytes > last_progress_bytes {
            last_progress_bytes = stats.progress_bytes;
            last_progress_at = tokio::time::Instant::now();
        }

        let now = tokio::time::Instant::now();
        if now >= hard_deadline {
            tracing::warn!(
                target: "hls",
                file = %file,
                idx,
                elapsed_s = started_at.elapsed().as_secs(),
                "TIMEOUT: hard deadline reached"
            );
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                format!("segmento {file} no disponible tras {HARD_DEADLINE_SECS}s"),
            ));
        }
        if last_progress_at.elapsed().as_secs() >= STALL_TIMEOUT_SECS {
            // Swarm stalled: cero bytes en 15 s. Reportar con
            // datos reales (velocidad, peers, %) para que el
            // frontend pinte un error honesto.
            let down_mbps = state
                .handle
                .live()
                .map(|l| l.down_speed_estimator().mbps())
                .unwrap_or(0.0);
            let live_peers = stats
                .live
                .as_ref()
                .map(|l| l.snapshot.peer_stats.live as u32)
                .unwrap_or(0);
            let downloaded_pct = if stats.total_bytes > 0 {
                (stats.progress_bytes as f64 / stats.total_bytes as f64) * 100.0
            } else {
                0.0
            };
            let speed_bps = (down_mbps * 1024.0 * 1024.0) as u64;
            tracing::warn!(
                target: "hls",
                file = %file,
                idx,
                elapsed_s = started_at.elapsed().as_secs(),
                stalled_s = last_progress_at.elapsed().as_secs(),
                down_mbps = format!("{down_mbps:.2}"),
                peers = live_peers,
                downloaded_pct = format!("{downloaded_pct:.1}"),
                "swarm_stalled"
            );
            let body = format!(
                r#"{{"reason":"swarm_stalled","downloaded_pct":{:.2},"speed_bps":{},"peers":{},"stalled_s":{}}}"#,
                downloaded_pct,
                speed_bps,
                live_peers,
                last_progress_at.elapsed().as_secs()
            );
            let resp = Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            return Ok(resp);
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
        // `no-store`: al cambiar de pista de audio, el backend
        // purga los `.ts` y respawnea ffmpeg con `-map 0:a:<idx>`
        // nuevo, pero los clientes seguían sirviendo desde HTTP
        // cache los segmentos viejos (mismo URL) — el cambio de
        // audio no se oía. Sin cache HTTP, cada request va al
        // backend, que sirve el `.ts` recién generado. El rewind
        // corto sigue barato porque hls.js/AVFoundation tienen su
        // propio buffer en memoria, y el rewind largo pega al
        // backend que ya tiene el `.ts` en disco.
        .header(header::CACHE_CONTROL, "no-store")
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_hls_filename_rejects_playlist() {
        // El playlist va por su propia ruta (`/hls/playlist.m3u8` →
        // `serve_hls_playlist`). Este handler solo debe ver segments.
        assert!(!is_valid_hls_filename("playlist.m3u8"));
        // El `live.m3u8` que escribe ffmpeg tampoco se sirve nunca.
        assert!(!is_valid_hls_filename("live.m3u8"));
    }

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

    #[test]
    fn is_valid_hls_filename_rejects_traversal() {
        assert!(!is_valid_hls_filename("../etc/passwd"));
        assert!(!is_valid_hls_filename("..\\etc\\passwd"));
        assert!(!is_valid_hls_filename("seg-00000.ts/../foo"));
        assert!(!is_valid_hls_filename("seg-00000.ts\\foo"));
    }

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
}
