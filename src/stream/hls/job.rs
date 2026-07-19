//! Ciclo de vida de los `Child` de ffmpeg: `spawn_hls`,
//! `ensure_hls_job`, warmup paralelo de librqbit para el offset de
//! arranque, snapshot del stderr para diagnóstico de fallos. Extraído
//! de `stream.rs` en el refactor (commit paso 4b). Sin cambios de
//! comportamiento.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
use axum::http::StatusCode;
use tokio::io::{AsyncSeekExt, SeekFrom};
use tokio_util::sync::CancellationToken;

use super::super::state::{current_client_capabilities, AppState, HlsJob, HlsMode, HLS_SEG_SECS};
use super::argv::{
    audio_transcode_argv, probe_is_hdr_video, probe_selected_audio, probe_video_height,
};

#[allow(unused_imports)]
use crate::winutil::HideConsoleExt;

pub(super) async fn ensure_hls_job(state: &AppState, idx: u64) -> Result<(), (StatusCode, String)> {
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
        // Cancelar el warmup del job viejo ANTES del kill: el warmup
        // mantiene un FileStream abierto contra librqbit, y librqbit
        // reparte el ancho de banda entre TODOS los FileStreams
        // activos. Si sobrevive al respawn, el nuevo ffmpeg se lleva
        // la mitad de la velocidad efectiva.
        if let Some(token) = old.warmup_cancel.as_ref() {
            token.cancel();
        }
        // Cancelar la tarea de throttle ANTES del kill: si sobrevive,
        // podría enviar SIGCONT a un pid reciclado por el kernel para
        // otro proceso completamente ajeno tras el reap del child.
        // SIGKILL funciona sobre procesos parados, así que no hace
        // falta SIGCONT previo — matamos directamente.
        if let Some(token) = old.throttle_cancel.as_ref() {
            token.cancel();
        }
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

    // Warm-up EN PARALELO (audit §2): NO bloqueamos el spawn de
    // ffmpeg. Antes ejecutábamos el warmup síncronamente antes del
    // spawn — 24 s de serialización pura en el peor caso, con
    // ffmpeg parado sin razón (ffmpeg lee por HTTP, esperaría esos
    // mismos bytes en paralelo con la descarga en cuanto arranque).
    //
    // La tarea corre concurrentemente y su único efecto es la
    // priorización de piezas en librqbit; nadie la espera. Se
    // cancela al reemplazar el job (arriba) para no dejar
    // FileStreams huérfanos compitiendo con el nuevo ffmpeg.
    let warmup_cancel = if start_seconds > 5.0 {
        let token = CancellationToken::new();
        let token_task = token.clone();
        let state_task = state.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = token_task.cancelled() => {
                    tracing::info!(
                        target: "warmup",
                        start_seconds,
                        "cancelled (job replaced or stream dropped)"
                    );
                }
                _ = warmup_librqbit_for_offset(&state_task, start_seconds) => {}
            }
        });
        Some(token)
    } else {
        None
    };

    let (child, stderr_tail) = spawn_hls(state, &dir, idx, audio_idx, mode, start_seconds).await?;
    // Detección de fallo temprano: si el argv es inválido
    // (filter missing, codec sin soporte, PATH roto…) ffmpeg
    // muere en <100 ms con exit != 0. Sin este check el loop de
    // `serve_hls_segment` respawnearía indefinidamente cada
    // 150 ms. Damos 500 ms de gracia — un spawn "sano" tarda
    // decenas de ms en abrir el input pero no exita nunca; uno
    // "malo" muere casi al instante.
    //
    // Snapshotamos el estado del hw encoder ANTES del sleep: si
    // muere y usábamos hw, hay una retry con libx264 (audit
    // "sparse+throttle+hwaccel" §4c). El probe de arranque no es
    // suficiente por sí solo — los drivers hw fallan de formas
    // creativas por-título-que-toca (formato exótico, VRAM
    // agotada, resolución no soportada).
    let hw_used = matches!(mode, HlsMode::Transcode) && crate::ffmpeg::hw_encoder().is_some();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let mut child = child;
    let mut stderr_tail = stderr_tail;
    match child.try_wait() {
        Ok(Some(status)) if !status.success() => {
            let tail = snapshot_stderr_tail(&stderr_tail);
            tracing::warn!(
                target: "ffmpeg",
                code = %status,
                stderr_tail = %tail,
                hw_used,
                "ffmpeg (hls) exited during warmup window"
            );
            // Fallback runtime: si el fallo ocurrió con hw encoder
            // activo, marcamos como degradado y reintentamos UNA
            // vez con libx264. El resto de la sesión ya va software
            // (HW_DEGRADED persiste hasta reiniciar el proceso).
            if hw_used {
                crate::ffmpeg::mark_hw_degraded();
                tracing::info!(
                    target: "hls",
                    "reintentando spawn con libx264 (hw encoder degradado)"
                );
                let (new_child, new_tail) =
                    spawn_hls(state, &dir, idx, audio_idx, mode, start_seconds).await?;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let mut new_child = new_child;
                if let Ok(Some(status2)) = new_child.try_wait() {
                    if !status2.success() {
                        // Ni siquiera libx264 arranca — es un fallo
                        // real (filter missing, codec sin soporte,
                        // PATH roto). Marcamos fatal y salimos.
                        if let Some(token) = warmup_cancel.as_ref() {
                            token.cancel();
                        }
                        let tail2 = snapshot_stderr_tail(&new_tail);
                        let msg = format!(
                            "ffmpeg exited with {} in <500ms tras retry con libx264: {}",
                            status2, tail2
                        );
                        tracing::error!(target: "hls", error = %msg, "FATAL");
                        let mut guard = state.hls.lock().await;
                        if let Some(hls) = guard.as_mut() {
                            hls.fatal_error = Some(msg.clone());
                        }
                        return Err((StatusCode::INTERNAL_SERVER_ERROR, msg));
                    }
                }
                child = new_child;
                stderr_tail = new_tail;
            } else {
                // Fallo sin hw encoder: fatal, no hay retry útil.
                if let Some(token) = warmup_cancel.as_ref() {
                    token.cancel();
                }
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
    // Throttle del transcode (audit "sparse+throttle+hwaccel" §2):
    // solo se activa en mode = Transcode; en Copy es I/O-bound y
    // no tiene sentido pausarlo (bufferear por delante es DESEABLE).
    // La task se auto-termina al cancelarse el token en el próximo
    // reemplazo del job.
    let (pid, paused, throttle_cancel) = {
        let pid = child.id();
        let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = if matches!(mode, HlsMode::Transcode) {
            Some(super::throttle::spawn_throttle_task(state.clone()))
        } else {
            None
        };
        (pid, paused, cancel)
    };
    hls.job = Some(HlsJob {
        child,
        start_idx: idx,
        pid,
        paused,
        warmup_cancel,
        throttle_cancel,
        stderr_tail,
    });
    Ok(())
}

/// Snapshot de las últimas líneas de stderr capturadas por el
/// reader task de `spawn_hls`. Devuelve una `String` multilínea
/// (una por línea) lista para inyectar en un `tracing::warn!`.
/// Corto y lock-free frente a la task lectora: solo bloqueamos
/// mientras clonamos el `VecDeque`.
pub(super) fn snapshot_stderr_tail(tail: &Arc<StdMutex<VecDeque<String>>>) -> String {
    match tail.lock() {
        Ok(buf) => buf.iter().cloned().collect::<Vec<_>>().join("\n"),
        Err(_) => String::new(),
    }
}

// ── Cobertura de tests ────────────────────────────────────────────────────
//
// `spawn_hls` / `ensure_hls_job` arrancan ffmpeg contra un torrent vivo
// de librqbit → no son testeables en unitario sin lanzar binarios del
// sistema. Lo que sí está cubierto sin deps externas:
//
//  * `snapshot_stderr_tail` — ver `#[cfg(test)] mod tests` abajo.
//
// Smoke test de integración: el CI corre el ciclo completo de arranque
// (probe → HLS spawn → primer segmento) contra un archivo de test en
// `tests/smoke/` con ffmpeg preinstalado (job de CI `gui-check`). Si
// ese job sigue verde, el spawn path está sano.
//
// Lo que queda pendiente de cobertura unitaria futura:
//   - Inyectar un spawner mockeado en `spawn_hls` para testear las
//     transiciones Created → Running → Evicted sin ffmpeg real. Requiere
//     refactor de la firma de `spawn_hls` para aceptar un `ChildSpawner`
//     trait (o equivalente).
//   - Transición warmup_cancel: verificar que el token se cancela antes
//     del kill del job viejo, para no dejar FileStreams huérfanos.

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
async fn spawn_hls(
    state: &AppState,
    dir: &Path,
    idx: u64,
    audio_idx: Option<usize>,
    mode: HlsMode,
    start_seconds: f64,
) -> Result<(tokio::process::Child, Arc<StdMutex<VecDeque<String>>>), (StatusCode, String)> {
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
    // Hardware-accelerated DECODE (audit "sparse+throttle+hwaccel"
    // §4b). Cubre la mitad de la factura CPU en fuentes HEVC/H.264
    // (el otro 50% es el encode, ver más abajo). El flag va ANTES
    // del `-i` — asocia el decoder al input inmediatamente
    // siguiente. Si el códec del input no está soportado por el
    // backend hw (p.ej. AV1 sobre VideoToolbox antiguo), ffmpeg cae
    // a software decode automáticamente sin error.
    //
    // Solo aplica en Transcode; en Copy no hay decode+encode, solo
    // remux — `-hwaccel` sería no-op y ensuciaría la línea de log.
    //
    // NO usamos `-hwaccel_output_format` (mantener frames en GPU
    // hasta el encode): dispararía errores con los filtros de
    // tonemap HDR y complicaría el fallback runtime a libx264. El
    // punto dulce robustez/rendimiento es decode-hw → RAM → encode-hw.
    if matches!(mode, HlsMode::Transcode) && crate::ffmpeg::hw_encoder().is_some() {
        #[cfg(target_os = "macos")]
        {
            cmd.arg("-hwaccel").arg("videotoolbox");
        }
        #[cfg(windows)]
        {
            // `d3d11va` es genérico — funciona sobre cualquier GPU
            // (NVIDIA/Intel/AMD) sin acoplarse al encoder elegido.
            // dxva2 sería otra opción pero d3d11va es la ruta
            // moderna que ffmpeg prefiere.
            cmd.arg("-hwaccel").arg("d3d11va");
        }
    }
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
    tracing::info!(
        target: "hls",
        mode = ?mode,
        idx,
        start_seconds,
        "video: argv decision"
    );
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
            // Audit §5: CRF 18 High 5.2 + veryfast (libx264).
            // Audit "sparse+throttle+hwaccel" §4: si hay hw encoder
            // detectado, usar el argv específico del vendor
            // (VideoToolbox / NVENC / QSV / AMF) en lugar de
            // libx264. Mantiene `-force_key_frames`, `-pix_fmt`,
            // `-bf 0`, `-avoid_negative_ts` en AMBAS ramas para
            // que la rejilla de segmentos (cortes en múltiplos
            // exactos de HLS_SEG_SECS) siga siendo compatible.
            //
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
            //
            // El tonemap va SIEMPRE por CPU (aunque el encoder
            // sea hw). Usar hwupload/hwdownload alrededor del
            // filter chain sería más rápido pero introduce
            // fragilidad — el punto dulce robustez/rendimiento
            // es decode-hw → RAM → tonemap-CPU → encode-hw (ver
            // comentario del `-hwaccel` arriba).
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
            // Selección de encoder: hw si detectado, si no libx264.
            match crate::ffmpeg::hw_encoder() {
                Some(hw) => {
                    let height = probe_video_height(state).await;
                    let argv = super::argv::hw_encoder_argv(hw, height);
                    tracing::info!(
                        target: "hls",
                        encoder = hw.ffmpeg_name(),
                        height,
                        argv = ?argv,
                        "video: hw encode"
                    );
                    for a in &argv {
                        cmd.arg(a);
                    }
                }
                None => {
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
                        // x264-params: scenecut=0 evita keyframes
                        // por detección de cambio de escena (romperían
                        // la rejilla de 4s exactos); sliced-threads=0
                        // conserva la referencia entre threads.
                        .arg("-x264-params")
                        .arg("scenecut=0:slices=1:sliced-threads=0");
                }
            }
            // Args comunes a AMBAS ramas (libx264 y hw encoders).
            // Van tras la selección del encoder para no repetir
            // código y garantizar que la rejilla de segmentos sea
            // compatible entre modos.
            cmd.arg("-pix_fmt")
                .arg("yuv420p")
                .arg("-bf")
                .arg("0")
                // Keyframes forzados en múltiplos exactos de 4s (0,
                // 4, 8, …). Requisito NO NEGOCIABLE para que dos
                // jobs distintos (uno desde 0, otro desde `-ss
                // 1728`) corten segmentos en las mismas fronteras
                // temporales, y por tanto sean intercambiables.
                // VideoToolbox y NVENC lo respetan con ffmpeg ≥ 6.
                .arg("-force_key_frames")
                .arg("expr:gte(t,n_forced*4)")
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
    //
    // TRANSCODE (audit §3, revisado): la argv de la rama transcode
    // depende del SO porque el multicanal solo funciona donde el
    // decoder del WebView lo soporta. Matriz completa en el docstring
    // de `audio_transcode_argv`:
    //
    //   * macOS / WKWebView    → mantener layout del origen (AAC
    //                            5.1 sale del transmux, CoreAudio
    //                            hace el downmix al device).
    //   * Windows / WebView2   → `-ac 2 -b:a 256k` forzados.
    //                            Chromium rechaza AAC >2ch con
    //                            `kUnsupportedConfig`.
    //   * Linux / WebKitGTK    → mismo fix que Windows cuando
    //                            llegue el soporte (GStreamer
    //                            playbin no negocia >2ch sin
    //                            pulse-surround).
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
        tracing::info!(
            target: "hls",
            src = ?in_audio_codec,
            channels = ?audio_channels,
            mode = ?mode,
            "audio: -c:a copy"
        );
    } else {
        // Argv delegado a `audio_transcode_argv` (SO-condicional):
        // ver docstring para la matriz Chromium/WKWebView/GStreamer.
        // Tests unitarios en `mod tests` verifican que la rama
        // no-macOS SIEMPRE incluye `-ac 2` y `256k`.
        let argv = audio_transcode_argv(audio_channels);
        tracing::info!(
            target: "hls",
            src = ?in_audio_codec,
            channels = ?audio_channels,
            mode = ?mode,
            argv = ?argv,
            "audio: transcode aac"
        );
        for a in argv {
            cmd.arg(a);
        }
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
    // Anillo circular de las últimas ~60 líneas del stderr para
    // poder loguearlas al detectar exit != 0. Sin esto, el warn
    // acaba siendo "ffmpeg died, exit=N" sin pista de causa.
    let stderr_tail: Arc<StdMutex<VecDeque<String>>> =
        Arc::new(StdMutex::new(VecDeque::with_capacity(64)));
    if let Some(stderr) = child.stderr.take() {
        let tail_task = stderr_tail.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // ffmpeg stderr → nivel `debug` (con `-loglevel error`
                // solo emite lo importante). Al operar con
                // `VIDEODROME_LOG_LEVEL=debug` el usuario ve el argv
                // completo + errores por consola.
                tracing::debug!(target: "ffmpeg-hls", "{line}");
                if let Ok(mut buf) = tail_task.lock() {
                    if buf.len() >= 60 {
                        buf.pop_front();
                    }
                    buf.push_back(line);
                }
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
    Ok((child, stderr_tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_empty_tail_gives_empty_string() {
        let tail = Arc::new(StdMutex::new(VecDeque::<String>::new()));
        assert_eq!(snapshot_stderr_tail(&tail), "");
    }

    #[test]
    fn snapshot_tail_joins_lines_with_newline() {
        let tail = Arc::new(StdMutex::new(VecDeque::from([
            "Error: codec not found".to_string(),
            "Error: invalid option".to_string(),
        ])));
        let s = snapshot_stderr_tail(&tail);
        assert_eq!(s, "Error: codec not found\nError: invalid option");
    }

    #[test]
    fn snapshot_tail_single_line_no_trailing_newline() {
        let tail = Arc::new(StdMutex::new(VecDeque::from(["only line".to_string()])));
        assert_eq!(snapshot_stderr_tail(&tail), "only line");
    }

    #[test]
    fn snapshot_does_not_mutate_buffer() {
        let tail = Arc::new(StdMutex::new(VecDeque::from([
            "a".to_string(),
            "b".to_string(),
        ])));
        let _ = snapshot_stderr_tail(&tail);
        let guard = tail.lock().expect("lock");
        assert_eq!(
            guard.len(),
            2,
            "buffer debe permanecer intacto tras snapshot"
        );
    }
}
