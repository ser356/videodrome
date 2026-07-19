//! LRU eviction de segmentos .ts (audit §6).
//!
//! Modelo COPY = disco crece con bitrate ORIGINAL: un remux UHD
//! visto entero deja ~60 GB en el tempdir. La evicción por
//! presupuesto es NECESARIA (no opcional) para no llenar disco.
//!
//! Estrategia: cada `EVICT_INTERVAL_SECS` sumamos tamaños de
//! `seg-*.ts`; si el total supera `budget_bytes`, borramos los más
//! alejados del `last_requested_idx` (playhead) hasta bajar a 90%
//! del budget (10% de headroom para no evictar en cada ciclo).
//!
//! Safety window: nunca borramos idx en
//! `[playhead-2, playhead+HLS_LOOKAHEAD+2]`. Ese margen cubre el
//! segmento que se está reproduciendo, los ya buffered por el
//! player (típ. 2-3 hacia adelante), y el que ffmpeg está
//! produciendo justo ahora.
//!
//! Priorización: entre segmentos igual de lejanos, borramos primero
//! los que están POR DETRÁS del playhead — "rewind" es menos
//! común que "keep watching forward", y evictar-luego-rehacer
//! atrás es más barato (el ffmpeg respawn desde un keyframe atrás
//! solo cuesta lo que tarde librqbit en re-servir esas piezas, ya
//! cacheadas por libraría).
//!
//! Extraído de `stream.rs` en el refactor (commit paso 4a). Sin
//! cambios de comportamiento.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};

use super::super::state::HLS_LOOKAHEAD;
use super::parse_seg_idx;

const EVICT_INTERVAL_SECS: u64 = 10;
const EVICT_SAFETY_WINDOW: u64 = HLS_LOOKAHEAD + 2;
const EVICT_TARGET_RATIO: f64 = 0.9;

/// Spawnea la tarea de eviction. El JoinHandle se guarda en
/// `HlsState._evictor` para que `Drop` la aborte al cerrar el
/// stream (si no, seguiría escaneando un dir borrado).
pub(in crate::stream) fn spawn_lru_evictor(
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
async fn evict_once(dir: &Path, budget_bytes: u64, playhead_idx: u64) -> Result<()> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || evict_once_sync(&dir, budget_bytes, playhead_idx))
        .await
        .context("evict spawn_blocking join")?
}

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
        // `len()` == uso real aquí: los `.ts` los escribe ffmpeg
        // linealmente y renombra desde `.tmp` al cerrar (flag
        // `temp_file`), sin preasignación sparse. Por eso NO usamos
        // `st_blocks` como en `cache::dir_size_bytes` — no hay
        // discrepancia lógica-vs-físico que corregir.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_seg(dir: &std::path::Path, idx: u64, size: usize) {
        let path = dir.join(format!("seg-{idx:05}.ts"));
        fs::write(path, vec![0u8; size]).expect("write test seg");
    }

    #[test]
    fn no_eviction_when_under_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_seg(dir.path(), 0, 100);
        write_seg(dir.path(), 1, 100);
        evict_once_sync(dir.path(), 10_000, 5).expect("evict_once_sync");
        assert!(dir.path().join("seg-00000.ts").exists());
        assert!(dir.path().join("seg-00001.ts").exists());
    }

    #[test]
    fn safety_window_protects_segments_near_playhead() {
        let dir = tempfile::tempdir().expect("tempdir");
        let playhead = 20u64;
        // Rellena la safety window: abs_diff(idx, 20) <= EVICT_SAFETY_WINDOW (8).
        // Todos en [12, 28]. Total 17 × 1 000 = 17 000, budget = 5 000 → sobre presupuesto.
        for idx in 12u64..=28 {
            write_seg(dir.path(), idx, 1_000);
        }
        evict_once_sync(dir.path(), 5_000, playhead).expect("evict_once_sync");
        for idx in 12u64..=28 {
            let path = dir.path().join(format!("seg-{idx:05}.ts"));
            assert!(
                path.exists(),
                "seg-{idx:05}.ts debe estar protegido por safety window"
            );
        }
    }

    #[test]
    fn evicts_segments_beyond_safety_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        let playhead = 50u64;
        // Safety window: abs_diff(idx, 50) <= 8 → [42, 58].
        // Far behind (dist=50 > 8), near protected (dist=0 ≤ 8).
        write_seg(dir.path(), 0, 500_000); // dist=50, fuera de ventana
        write_seg(dir.path(), 30, 500_000); // dist=20, fuera de ventana
        write_seg(dir.path(), 50, 500_000); // dist=0, dentro de ventana (playhead)
                                            // Budget 800_000, total 1_500_000 → se debe evictar al menos un segmento lejano.
        evict_once_sync(dir.path(), 800_000, playhead).expect("evict_once_sync");
        assert!(
            dir.path().join("seg-00050.ts").exists(),
            "playhead protegido por safety window"
        );
        let far0_exists = dir.path().join("seg-00000.ts").exists();
        let far30_exists = dir.path().join("seg-00030.ts").exists();
        assert!(
            !far0_exists || !far30_exists,
            "al menos un segmento lejano debe haber sido evictado"
        );
    }

    #[test]
    fn backward_evicted_before_forward_at_same_distance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let playhead = 50u64;
        // dist=20 atrás vs dist=20 adelante — ambos fuera de la ventana (8).
        // El evictor prioriza los de ATRÁS según la política de score.
        write_seg(dir.path(), 30, 500_000); // detrás, dist=20
        write_seg(dir.path(), 70, 500_000); // delante, dist=20
                                            // Budget 600 000, total 1 000 000 → target 540 000 → basta evictar uno.
        evict_once_sync(dir.path(), 600_000, playhead).expect("evict_once_sync");
        assert!(
            dir.path().join("seg-00070.ts").exists(),
            "segmento de ADELANTE debe conservarse frente al de detrás"
        );
        assert!(
            !dir.path().join("seg-00030.ts").exists(),
            "segmento de DETRÁS debe evictarse primero"
        );
    }

    #[test]
    fn tmp_files_not_counted_nor_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        // .ts.tmp = ffmpeg escribiendo ahora mismo — NUNCA tocar.
        let tmp = dir.path().join("seg-00000.ts.tmp");
        fs::write(&tmp, vec![0u8; 2_000_000]).expect("write tmp");
        // El .ts completo sí cuenta; está bajo presupuesto (100 < 10 000).
        write_seg(dir.path(), 0, 100);
        evict_once_sync(dir.path(), 10_000, 100).expect("evict_once_sync");
        assert!(tmp.exists(), ".ts.tmp no debe ser eliminado por el evictor");
        assert!(
            dir.path().join("seg-00000.ts").exists(),
            ".ts bajo presupuesto conservado"
        );
    }
}
