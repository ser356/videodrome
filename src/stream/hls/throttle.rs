//! Throttle del transcode ffmpeg (audit "sparse+throttle+hwaccel" §2).
//!
//! Sin throttle, ffmpeg transcodifica un título a la velocidad máxima
//! de la CPU (~25× tiempo real en HEVC 1080p → H.264 medido en el log
//! de evidencia: seg-00000..seg-00021 servidos en 3.2 s, un segmento
//! de 4 s cada ~160 ms). El consumo del player es 1 segmento / 4 s
//! reales, así que la CPU (~997 %, ventiladores, batería) es puro
//! desperdicio: se transcodifica la película entera aunque el user
//! esté en el minuto 2 o abandone a los 10.
//!
//! Política (histéresis para evitar ping-pong):
//!   - PAUSAR cuando ventaja > `PAUSE_ADVANTAGE_S` (120 s = 30
//!     segmentos por delante del playhead).
//!   - REANUDAR cuando ventaja < `RESUME_ADVANTAGE_S` (45 s).
//!
//! La histéresis 120/45 absorbe con margen los 4 s/segmento del
//! consumo real y evita que la task oscile en cada tick.
//!
//! Solo aplica a mode = Transcode. Copy es I/O-bound, marginal en
//! CPU, y llenar el caché con antelación es DESEABLE (buffer contra
//! enjambres irregulares).
//!
//! Mecanismo por plataforma (abstracción `pause_process` /
//! `resume_process`):
//!
//!   - **Unix**: `SIGSTOP` / `SIGCONT` al pid de ffmpeg. Congelado
//!     instantáneo, estado y sockets intactos, CPU a cero. La
//!     conexión HTTP contra `/video` queda viva pero sin leer — axum
//!     no cierra por inactividad de escritura por defecto, así que
//!     el body stream simplemente deja de avanzar hasta el SIGCONT.
//!     `SIGKILL` funciona sobre procesos parados, así que el kill
//!     de `ensure_hls_job` procede sin necesidad de SIGCONT previo.
//!
//!   - **Windows**: sin señales de este estilo. Estrategia
//!     kill-and-resurrect: al superar la ventaja se mata el job
//!     entero y el modelo bajo demanda de `serve_hls_segment` lo
//!     re-spawnea cuando el player pide un idx no producido. Coste:
//!     un re-spawn con `-ss` (~1 s, ya optimizado por el audit de
//!     cold-start). No usamos `NtSuspendProcess` / `DebugActiveProcess`
//!     (APIs no documentadas o con efectos colaterales — antivirus,
//!     debuggers — que no compensan).

use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::super::state::{AppState, HlsMode, HLS_SEG_SECS};
use super::max_produced_idx;

/// Umbral a partir del cual pausamos ffmpeg. 120 s ≈ 30 segmentos
/// por delante del playhead.
const PAUSE_ADVANTAGE_S: f64 = 120.0;
/// Umbral por debajo del cual reanudamos. 45 s ≈ 11 segmentos de
/// buffer restante, suficiente para no re-buffear.
const RESUME_ADVANTAGE_S: f64 = 45.0;
/// Intervalo de evaluación de la política. Cada 2 s: barato, y
/// reactivo suficiente (con 45 s de headroom sobra).
const TICK_INTERVAL_SECS: u64 = 2;

/// Spawnea la tarea de control. Devuelve un token que aborta la
/// tarea al cancelarlo — `ensure_hls_job` lo llama antes de matar
/// el ffmpeg viejo para evitar que la tarea envíe SIGCONT a un pid
/// reciclado por el kernel para otro proceso ajeno.
///
/// La tarea se auto-termina cuando: el job es reemplazado (token
/// cancelado), el proceso murió (child.try_wait no lo detecta
/// desde aquí, pero el spawn_hls task lector de stderr sí, y el
/// evictor / próximo request veremos el estado), o el modo ya no
/// es Transcode.
pub(in crate::stream) fn spawn_throttle_task(state: AppState) -> CancellationToken {
    let token = CancellationToken::new();
    let token_task = token.clone();
    tokio::spawn(async move {
        run_throttle_loop(state, token_task).await;
    });
    token
}

async fn run_throttle_loop(state: AppState, cancel: CancellationToken) {
    let interval = Duration::from_secs(TICK_INTERVAL_SECS);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                return;
            }
            _ = tokio::time::sleep(interval) => {}
        }
        if !tick(&state).await {
            return;
        }
    }
}

/// Un ciclo de evaluación. Devuelve `false` si la task debe
/// terminarse (job desaparecido, cambió de mode a Copy, etc.).
async fn tick(state: &AppState) -> bool {
    // Snapshot del estado bajo lock. Todo el trabajo pesado
    // (SIGSTOP/SIGCONT, kill en Windows) va fuera del lock — pero
    // el kill Windows requiere volver a coger el lock para
    // manipular `hls.job`.
    let snapshot = {
        let guard = state.hls.lock().await;
        let Some(hls) = guard.as_ref() else {
            return false; // stream cerrado
        };
        let Some(job) = hls.job.as_ref() else {
            return true; // job aún no arrancado o entre spawns
        };
        if hls.mode != HlsMode::Transcode {
            return false; // solo Transcode se throttlea
        }
        Snapshot {
            mode: hls.mode,
            dir: hls.dir.clone(),
            start_idx: job.start_idx,
            playhead: hls.last_requested_idx.load(Ordering::Relaxed),
            pid: job.pid,
            paused: job.paused.clone(),
        }
    };
    let produced = max_produced_idx(&snapshot.dir, snapshot.start_idx);
    // `produced` puede ser `< playhead` cuando el player ha hecho
    // seek hacia adelante y el nuevo job apenas ha arrancado — la
    // "ventaja" negativa se satura a 0 para no reanudar por error
    // (playhead lejos delante no significa que ffmpeg vaya sobrado).
    let advantage_segments = produced.saturating_sub(snapshot.playhead);
    let advantage_s = (advantage_segments as f64) * HLS_SEG_SECS;
    let currently_paused = snapshot.paused.load(Ordering::Relaxed);

    if !currently_paused && advantage_s > PAUSE_ADVANTAGE_S {
        apply_pause(state, &snapshot, advantage_s).await;
    } else if currently_paused && advantage_s < RESUME_ADVANTAGE_S {
        apply_resume(&snapshot, advantage_s).await;
    }
    true
}

struct Snapshot {
    #[allow(dead_code)] // usado solo para logs/depuración futura
    mode: HlsMode,
    dir: std::path::PathBuf,
    start_idx: u64,
    playhead: u64,
    pid: Option<u32>,
    paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

// ── Unix: SIGSTOP / SIGCONT ─────────────────────────────────────

#[cfg(unix)]
async fn apply_pause(_state: &AppState, snap: &Snapshot, advantage_s: f64) {
    let Some(pid) = snap.pid else {
        return;
    };
    // SAFETY: `libc::kill` es una syscall; segura de llamar. El pid
    // proviene de `Child::id()` del proceso que spawneamos y aún
    // no hemos reap-eado (la task de throttle se aborta antes del
    // kill en `ensure_hls_job`). Peor caso: el pid ya fue reap-eado
    // por otro thread y el kernel lo recicló para otro proceso —
    // enviaríamos SIGSTOP a un ajeno. Mitigación: el CancellationToken
    // se aborta ANTES de `child.kill().await`, así que este código
    // no corre concurrentemente con la reap.
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) };
    if rc == 0 {
        snap.paused.store(true, Ordering::Relaxed);
        tracing::info!(
            target: "hls-throttle",
            pid,
            start_idx = snap.start_idx,
            playhead = snap.playhead,
            advantage_s = format!("{advantage_s:.1}"),
            "PAUSE (SIGSTOP)"
        );
    } else {
        tracing::warn!(
            target: "hls-throttle",
            pid,
            errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            "SIGSTOP failed"
        );
    }
}

#[cfg(unix)]
async fn apply_resume(snap: &Snapshot, advantage_s: f64) {
    let Some(pid) = snap.pid else {
        return;
    };
    // SAFETY: idem `apply_pause`.
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGCONT) };
    if rc == 0 {
        snap.paused.store(false, Ordering::Relaxed);
        tracing::info!(
            target: "hls-throttle",
            pid,
            start_idx = snap.start_idx,
            playhead = snap.playhead,
            advantage_s = format!("{advantage_s:.1}"),
            "RESUME (SIGCONT)"
        );
    } else {
        tracing::warn!(
            target: "hls-throttle",
            pid,
            errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            "SIGCONT failed"
        );
    }
}

// ── Windows: kill-and-resurrect ────────────────────────────────

#[cfg(windows)]
async fn apply_pause(state: &AppState, snap: &Snapshot, advantage_s: f64) {
    // Marca "paused" ANTES del kill: evita re-matar en el próximo
    // tick antes de que el modelo bajo demanda spawnee el reemplazo.
    // El flag se resetea implícitamente cuando `serve_hls_segment`
    // instala un `HlsJob` nuevo (con su propio `Arc<AtomicBool>`).
    snap.paused.store(true, Ordering::Relaxed);
    let mut guard = state.hls.lock().await;
    let Some(hls) = guard.as_mut() else {
        return;
    };
    // El job debe seguir siendo el mismo que snapshotamos — si
    // cambió (respawn concurrente), no tocamos.
    let same_job = hls
        .job
        .as_ref()
        .map(|j| j.start_idx == snap.start_idx)
        .unwrap_or(false);
    if !same_job {
        return;
    }
    if let Some(mut old) = hls.job.take() {
        drop(guard);
        // Cancelamos también el warm-up para no dejar FileStream vivo.
        if let Some(tok) = old.warmup_cancel.as_ref() {
            tok.cancel();
        }
        let _ = old.child.kill().await;
        let _ = old.child.wait().await;
        tracing::info!(
            target: "hls-throttle",
            start_idx = snap.start_idx,
            playhead = snap.playhead,
            advantage_s = format!("{advantage_s:.1}"),
            "PAUSE (Windows: killed job; resurrect on-demand)"
        );
    }
}

#[cfg(windows)]
async fn apply_resume(_snap: &Snapshot, _advantage_s: f64) {
    // No-op en Windows: el "resume" lo hace `serve_hls_segment`
    // cuando el player pide un idx no producido (spawn on-demand).
    // El flag `paused` del snapshot es de la instancia vieja del
    // HlsJob (ya muerta); no lo tocamos.
}
