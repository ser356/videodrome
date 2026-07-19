//! Decisión de modo HLS (Copy vs Transcode) + construcción de la
//! rejilla de segmentos. Extraído de `stream.rs` en el refactor
//! (commit paso 4a). Sin cambios de comportamiento.

use super::super::state::{HlsMode, HLS_SEG_SECS};

/// Decide `HlsMode` + construye la rejilla de segmentos para el
/// playlist. Audit §2/§7:
///
///   * Preferencia `Transcode` → siempre transcode con rejilla fija.
///   * Preferencia `Copy` → intentar copy; si falla, ERROR (el user
///     lo pidió expresamente, no cambiamos de modo bajo sus pies).
///   * Preferencia `Auto` (default): copy si (1) el códec de vídeo
///     es compatible con el cliente vía DIRECT-eligible codec set,
///     (2) el `KeyframeIndex` se puede leer, (3) el max GOP ≤ 10s.
///     Si algo falla → transcode.
pub(in crate::stream) async fn decide_mode_and_segments(
    info: &crate::ffmpeg::MediaInfo,
    caps: &crate::ffmpeg::ClientCapabilities,
    pref: crate::preferences::QualityMode,
    url: &str,
) -> (HlsMode, Vec<(f64, f64)>) {
    use crate::preferences::QualityMode;
    let duration = info.duration_seconds.unwrap_or(0.0);
    let transcode_grid = build_transcode_grid(duration);

    match pref {
        QualityMode::Transcode => (HlsMode::Transcode, transcode_grid),
        QualityMode::Copy => match try_build_copy_grid(info, caps, url).await {
            Ok(grid) if !grid.is_empty() => (HlsMode::Copy, grid),
            Ok(_) => {
                tracing::info!(target: "hls", "pref=Copy pero grid vacía → fallback transcode");
                (HlsMode::Transcode, transcode_grid)
            }
            Err(e) => {
                tracing::info!(target: "hls", error = %e, "pref=Copy falló → fallback transcode");
                (HlsMode::Transcode, transcode_grid)
            }
        },
        QualityMode::Auto => match try_build_copy_grid(info, caps, url).await {
            Ok(grid) if !grid.is_empty() => {
                tracing::info!(target: "hls", segments = grid.len(), "auto → COPY viable");
                (HlsMode::Copy, grid)
            }
            Ok(_) => {
                tracing::info!(target: "hls", "auto → grid vacía, transcode");
                (HlsMode::Transcode, transcode_grid)
            }
            Err(e) => {
                tracing::info!(target: "hls", error = %e, "auto → copy no viable, transcode");
                (HlsMode::Transcode, transcode_grid)
            }
        },
    }
}

/// Construye la rejilla fija de segmentos de `HLS_SEG_SECS`. El
/// último puede ser más corto para no exceder la duración total.
pub(in crate::stream) fn build_transcode_grid(duration: f64) -> Vec<(f64, f64)> {
    if duration <= 0.0 {
        return Vec::new();
    }
    let n = (duration / HLS_SEG_SECS).ceil() as usize;
    (0..n)
        .map(|i| {
            let start = i as f64 * HLS_SEG_SECS;
            let len = if i + 1 == n {
                (duration - start).max(0.001)
            } else {
                HLS_SEG_SECS
            };
            (start, len)
        })
        .collect()
}

/// `true` si el stream de vídeo declara transfer characteristics
/// HDR: SMPTE 2084 (PQ, típico en BluRay UHD) o ARIB STD-B67 (HLG,
/// broadcast). Audit §8.
pub(in crate::stream) fn is_hdr_stream(video: &crate::ffmpeg::StreamInfo) -> bool {
    video
        .color_transfer
        .as_deref()
        .map(|t| {
            let t = t.to_ascii_lowercase();
            t.contains("smpte2084") || t.contains("arib-std-b67") || t.contains("bt2020-10")
        })
        .unwrap_or(false)
}

/// Intenta construir la rejilla de segmentos para modo COPY:
/// fetchea el keyframe index del contenedor y agrupa keyframes en
/// segmentos ≥ `HLS_SEG_SECS`. Devuelve error si el códec no es
/// compatible con el cliente, si el índice no se puede leer, o si
/// el max GOP > 10s (audit §2d — con GOPs enormes el seek en copy
/// sería inaceptable).
pub(in crate::stream) async fn try_build_copy_grid(
    info: &crate::ffmpeg::MediaInfo,
    caps: &crate::ffmpeg::ClientCapabilities,
    url: &str,
) -> anyhow::Result<Vec<(f64, f64)>> {
    use anyhow::bail;
    let video = info
        .streams
        .iter()
        .find(|s| s.kind == crate::ffmpeg::StreamKind::Video)
        .ok_or_else(|| anyhow::anyhow!("sin stream de vídeo"))?;
    // Códec debe ser algo que el cliente pueda reproducir vía TS
    // sin transcode. H.264 universal; HEVC 8-bit solo si el cliente
    // declara `hevc`; HEVC 10-bit necesita `hevc10` Y salir de HDR
    // (dejado a §6/§8 futuros).
    let codec_ok = match video.codec.as_str() {
        "h264" => caps.supports("h264"),
        "hevc" | "h265" => {
            let is_10bit = video
                .pix_fmt
                .as_deref()
                .map(|p| {
                    let p = p.to_ascii_lowercase();
                    p.contains("10le") || p.contains("10be")
                })
                .unwrap_or(false);
            if is_10bit {
                caps.supports("hevc10")
            } else {
                caps.supports("hevc")
            }
        }
        _ => false,
    };
    if !codec_ok {
        bail!(
            "cliente no soporta '{}' vía TS copy (pix_fmt={:?})",
            video.codec,
            video.pix_fmt
        );
    }
    // Audit §8: HDR (SMPTE 2084 / arib-std-b67) es incompatible
    // con TS-copy incluso si el cliente soporta HEVC 10-bit. La
    // ausencia de tone-map + metadata deja los colores lavados en
    // pantallas SDR. Bailamos → el caller cae a transcode con la
    // cadena zscale+tonemap.
    if is_hdr_stream(video) {
        bail!(
            "HDR (color_transfer={:?}) → transcode+tonemap",
            video.color_transfer
        );
    }
    // Fetch keyframe index. Cliente HTTP reutilizable: creamos uno
    // simple aquí (localhost, sin cookies ni auth).
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_default();
    let idx =
        crate::keyframes::fetch_keyframe_index(&client, url, info.container.as_deref()).await?;
    let max_gap = idx.max_gap_seconds();
    const MAX_GOP_SECONDS: f64 = 10.0;
    if max_gap > MAX_GOP_SECONDS {
        bail!("GOP máximo {max_gap:.1}s > {MAX_GOP_SECONDS}s (seek en copy sería inaceptable)");
    }
    Ok(idx.variable_segments(HLS_SEG_SECS))
}
