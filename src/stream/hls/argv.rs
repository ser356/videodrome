//! Helpers para elegir codec/bitrate de audio + probes HDR usados
//! por `spawn_hls`. Argv del transcode de audio con la matriz por
//! SO (macOS vs !macOS). Extraído de `stream.rs` en el refactor
//! (commit paso 4a). Sin cambios de comportamiento.

use super::super::state::AppState;
use super::grid::is_hdr_stream;

/// Devuelve `(channels, codec)` del stream de audio que ffmpeg va
/// a mapear en `spawn_hls` (`audio_idx` explícito o el primero por
/// defecto). Consulta `cached_probe`; si no hay probe cacheado
/// devuelve `(None, None)`.
///
/// Se usa para elegir bitrate AAC y — en el futuro — decidir
/// `-c:a copy` cuando la fuente ya es AAC/MP3 (audit §3).
///
/// `audio_idx` es el índice contando SÓLO streams de audio (igual
/// que el argv `-map 0:a:<n>`).
pub(in crate::stream) async fn probe_selected_audio(
    state: &AppState,
    audio_idx: Option<usize>,
) -> (Option<u32>, Option<String>) {
    let guard = state.cached_probe.lock().await;
    let Some(info) = guard.as_ref() else {
        return (None, None);
    };
    let mut audios = info
        .streams
        .iter()
        .filter(|s| s.kind == crate::ffmpeg::StreamKind::Audio);
    let target = match audio_idx {
        Some(n) => audios.nth(n),
        None => audios.next(),
    };
    match target {
        Some(a) => (a.channels, Some(a.codec.clone())),
        None => (None, None),
    }
}

/// Bitrate AAC transparente-perceptual escalado por canales.
/// `≤2ch` o desconocido → 256k. `3-6ch` (5.1) → 384k. `7+ch`
/// (7.1+) → 512k. AAC LC a ~64k/canal es transparente para
/// material típico. Sin canales conocidos, 256k es el suelo seguro
/// (nunca peor que el 192k anterior). Audit §5.
///
/// SOLO relevante en la rama macOS de `audio_transcode_argv`: el
/// resto de plataformas fuerza `-ac 2 -b:a 256k` sin preguntar al
/// número de canales (ver `audio_transcode_argv` para el porqué),
/// así que en Windows/Linux la fn quedaría dead_code — de ahí el
/// gate `#[cfg(target_os = "macos")]` que replica el del único
/// call site.
#[cfg(target_os = "macos")]
fn aac_bitrate_for_channels(channels: Option<u32>) -> &'static str {
    match channels {
        Some(n) if n >= 7 => "512k",
        Some(n) if n >= 3 => "384k",
        _ => "256k",
    }
}

/// Argv de la rama TRANSCODE de audio para `spawn_hls`.
///
/// **Matriz real de soporte AAC multicanal en los WebView que
/// usamos como target de reproducción in-app** — el player embebido
/// pinta el HLS transmux en un `<video>`, no en un decoder nativo:
///
/// | Plataforma | WebView         | AAC 5.1 vía `<video>`? | Fix |
/// |------------|-----------------|------------------------|-----|
/// | macOS      | WKWebView       | Sí — CoreAudio decodifica AAC-LC multicanal y hace downmix al output device | conservar layout del origen, bitrate escalado |
/// | Windows    | WebView2 (Chromium) | **No** — el media pipeline de Chromium rechaza AAC >2ch con `kUnsupportedConfig` y el `<video>` dispara `MediaError code 4` sin más pista | forzar `-ac 2 -b:a 256k` |
/// | Linux (día que llegue) | WebKitGTK / GStreamer | **No** — GStreamer `avdec_aac` decodifica el bitstream pero el pipeline `playbin` en la mayoría de distros no negocia canal > 2 sin `pulseaudio` con perfil surround activo. Falla igual de silenciosamente que Chromium. | igual: `-ac 2 -b:a 256k` |
///
/// Por eso el split es `macos` vs `not(macos)`: cubre Windows hoy y
/// Linux el día que se soporte, sin tener que reabrir la lógica.
///
/// El error literal ("kUnsupportedConfig") va en el comentario a
/// propósito — hemos revertido esto ~2 veces al "optimizar" el
/// downmix pensando que Chromium ya lo aceptaría; no lo hace.
///
/// Devuelve un `Vec<&'static str>` (no `String`) para que el test
/// unitario pueda hacer `assert_eq!` directo sin allocs. `channels`
/// solo se lee en la rama macOS; en el resto es `_ = channels`.
pub(in crate::stream) fn audio_transcode_argv(channels: Option<u32>) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = vec!["-c:a", "aac"];
    #[cfg(target_os = "macos")]
    {
        // WKWebView decodifica AAC 5.1/7.1 y el downmix lo hace
        // CoreAudio en el device de salida (estéreo o surround si
        // el user tiene HomePods/AVR conectado). Conservar layout
        // del origen es puro upside: cero pérdida de canales y
        // ahorro de CPU en el mixer.
        v.push("-b:a");
        v.push(aac_bitrate_for_channels(channels));
    }
    #[cfg(not(target_os = "macos"))]
    {
        // NO tocar sin verificar en Windows real. Chromium/WebView2
        // rechaza AAC >2ch con "kUnsupportedConfig" (visible en el
        // log de la MediaSource pipeline si se abre DevTools);
        // el player externo sólo emite `MediaError code 4`
        // ("MEDIA_ELEMENT_ERROR: Format error") sin causa. El fix
        // es forzar downmix a estéreo con `-ac 2`; el bitrate cae
        // a 256k porque ya no hay canales que sostener.
        //
        // WebKitGTK/GStreamer sufre lo mismo (`playbin` no negocia
        // >2ch sin pulse-surround). Aplica igual bajo `not(macos)`.
        let _ = channels;
        v.push("-ac");
        v.push("2");
        v.push("-b:a");
        v.push("256k");
    }
    v
}

/// `true` si el stream de vídeo principal del `cached_probe` es
/// HDR (SMPTE 2084 / arib-std-b67 / bt2020-10). Se consulta en
/// `spawn_hls` (rama Transcode) para meter la cadena
/// zscale+tonemap y evitar colores lavados en SDR. Audit §8.
pub(in crate::stream) async fn probe_is_hdr_video(state: &AppState) -> bool {
    let guard = state.cached_probe.lock().await;
    let Some(info) = guard.as_ref() else {
        return false;
    };
    info.streams
        .iter()
        .find(|s| s.kind == crate::ffmpeg::StreamKind::Video)
        .map(is_hdr_stream)
        .unwrap_or(false)
}

/// Altura en píxeles del stream de vídeo principal del
/// `cached_probe`. Se usa para elegir bitrate del hw encoder cuando
/// el modelo no acepta CRF (VideoToolbox) — bitrates por resolución
/// según audit §5 / §4b. `None` si no hay probe cacheado o el
/// stream no expone `height`.
pub(in crate::stream) async fn probe_video_height(state: &AppState) -> Option<u32> {
    let guard = state.cached_probe.lock().await;
    guard
        .as_ref()?
        .streams
        .iter()
        .find(|s| s.kind == crate::ffmpeg::StreamKind::Video)?
        .height
}

/// Bitrate objetivo para encoders sin CRF (VideoToolbox), escalado
/// por altura del vídeo. Los valores son "visualmente transparentes"
/// para VT según el audit — el margen de calidad se compra con
/// bitrate porque VT no soporta CRF.
///
///   - ≤ 480p  → 2M
///   - ≤ 720p  → 4M
///   - ≤ 1080p → 8M
///   - `>` 1080p → 10M (downscale a 1080p sería otra opción, pero
///     VT con 4K encode directo es más simple; el overhead extra
///     queda absorbido por la GPU).
///
/// `None` en `height` cae al bucket 1080p (asunción conservadora
/// para no infra-bitratear un 4K desconocido).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub(in crate::stream) fn vt_bitrate_for_height(height: Option<u32>) -> &'static str {
    match height {
        Some(h) if h <= 480 => "2M",
        Some(h) if h <= 720 => "4M",
        Some(h) if h <= 1080 => "8M",
        Some(_) => "10M",
        None => "8M",
    }
}

/// Argv específico del hw encoder elegido. NO incluye los args
/// comunes (`-pix_fmt yuv420p`, `-bf 0`, `-force_key_frames`,
/// `-avoid_negative_ts make_zero`) — esos los añade el caller
/// SIEMPRE, en ambas ramas hw y libx264, para que la rejilla de
/// segmentos siga siendo compatible.
///
/// Los presets están calibrados según audit §4b:
///   - `h264_videotoolbox`: bitrate objetivo + maxrate 1.5×, high
///     profile, `-realtime 0` (calidad sobre velocidad; a igual
///     bitrate mejora ~1-2 dB PSNR).
///   - `h264_nvenc`: preset p5 + rc vbr + cq 23 (equivalente
///     perceptual a CRF 23 de libx264 según pruebas NVIDIA).
///   - `h264_qsv`: global_quality 23 + veryfast (Intel Media SDK).
///   - `h264_amf`: quality balanced + cqp con qp 22/24 (AMD AMF
///     no tiene VBR de la misma calidad; CQP es lo más estable).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub(in crate::stream) fn hw_encoder_argv(
    encoder: crate::ffmpeg::HwEncoder,
    height: Option<u32>,
) -> Vec<String> {
    use crate::ffmpeg::HwEncoder;
    match encoder {
        HwEncoder::VideoToolbox => {
            let bitrate = vt_bitrate_for_height(height);
            let maxrate = match bitrate {
                "2M" => "3M",
                "4M" => "6M",
                "8M" => "12M",
                "10M" => "15M",
                _ => "12M",
            };
            vec![
                "-c:v".into(),
                "h264_videotoolbox".into(),
                "-b:v".into(),
                bitrate.into(),
                "-maxrate".into(),
                maxrate.into(),
                "-profile:v".into(),
                "high".into(),
                "-realtime".into(),
                "0".into(),
            ]
        }
        HwEncoder::Nvenc => vec![
            "-c:v".into(),
            "h264_nvenc".into(),
            "-preset".into(),
            "p5".into(),
            "-rc".into(),
            "vbr".into(),
            "-cq".into(),
            "23".into(),
            "-b:v".into(),
            "0".into(),
            "-profile:v".into(),
            "high".into(),
        ],
        HwEncoder::Qsv => vec![
            "-c:v".into(),
            "h264_qsv".into(),
            "-global_quality".into(),
            "23".into(),
            "-preset".into(),
            "veryfast".into(),
            "-profile:v".into(),
            "high".into(),
        ],
        HwEncoder::Amf => vec![
            "-c:v".into(),
            "h264_amf".into(),
            "-quality".into(),
            "balanced".into(),
            "-rc".into(),
            "cqp".into(),
            "-qp_i".into(),
            "22".into(),
            "-qp_p".into(),
            "24".into(),
            "-profile:v".into(),
            "high".into(),
        ],
    }
}

// ── audio_transcode_argv — matriz por SO ─────────────────────
//
// Garantía dura: la rama non-macOS SIEMPRE fuerza `-ac 2` y
// `256k` independientemente del layout del origen; la rama
// macOS conserva el layout y escala el bitrate. Ver docstring
// de `audio_transcode_argv` para el porqué (Chromium
// `kUnsupportedConfig`, WKWebView + CoreAudio).
//
// El test de integración con ffmpeg real (transcode E-AC-3 5.1
// → AAC 2ch y verificación con ffprobe) vive en
// `windows_aac_downmix_produces_stereo` — está `#[ignore]` por
// defecto y CI lo corre en Windows con ffmpeg preinstalado.

#[cfg(all(test, not(target_os = "macos")))]
#[test]
fn audio_transcode_argv_forces_stereo_downmix_off_macos() {
    // Con 6 canales, la rama non-macOS igualmente debe forzar
    // downmix a 2ch: Chromium/WebView2 rechaza AAC multicanal
    // con `kUnsupportedConfig`.
    let argv = audio_transcode_argv(Some(6));
    assert_eq!(argv[0], "-c:a");
    assert_eq!(argv[1], "aac");
    let ac = argv
        .windows(2)
        .find(|w| w[0] == "-ac")
        .expect("-ac 2 debe estar presente fuera de macOS");
    assert_eq!(ac[1], "2");
    let br = argv
        .windows(2)
        .find(|w| w[0] == "-b:a")
        .expect("-b:a debe estar presente");
    assert_eq!(br[1], "256k");
}

#[cfg(all(test, not(target_os = "macos")))]
#[test]
fn audio_transcode_argv_ignores_channel_hint_off_macos() {
    // 2ch, 6ch, desconocido: la argv NO cambia — el bitrate es
    // 256k fijo porque ya no hay canales que sostener.
    for ch in [Some(2), Some(6), Some(8), None] {
        let argv = audio_transcode_argv(ch);
        assert!(argv.contains(&"-ac"));
        assert!(argv.contains(&"256k"));
        assert!(
            !argv.contains(&"384k") && !argv.contains(&"512k"),
            "bitrate multicanal filtrado en non-macos: {argv:?}"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn audio_transcode_argv_keeps_multichannel_on_macos() {
    // WKWebView + CoreAudio decodifica AAC multicanal → mantenemos
    // el layout del origen y solo escalamos el bitrate.
    let argv = audio_transcode_argv(Some(6));
    assert_eq!(argv[0], "-c:a");
    assert_eq!(argv[1], "aac");
    assert!(
        !argv.contains(&"-ac"),
        "macOS NO debe forzar downmix: {argv:?}"
    );
    let br = argv
        .windows(2)
        .find(|w| w[0] == "-b:a")
        .expect("-b:a debe estar presente");
    assert_eq!(br[1], "384k", "5.1 → 384k en macOS");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn audio_transcode_argv_stereo_gives_256k_on_macos() {
    // 2 canales → suelo 256k; WKWebView lo acepta sin downmix.
    let argv = audio_transcode_argv(Some(2));
    assert!(!argv.contains(&"-ac"), "macOS no fuerza downmix con 2ch");
    let br = argv.windows(2).find(|w| w[0] == "-b:a").expect("-b:a");
    assert_eq!(br[1], "256k", "estéreo → 256k en macOS");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn audio_transcode_argv_unknown_channels_gives_256k_on_macos() {
    // Sin dato de canales (None) → bitrate suelo 256k.
    let argv = audio_transcode_argv(None);
    assert!(!argv.contains(&"-ac"), "macOS no fuerza downmix con None");
    let br = argv.windows(2).find(|w| w[0] == "-b:a").expect("-b:a");
    assert_eq!(br[1], "256k", "None canales → 256k en macOS");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn audio_transcode_argv_7_1_gives_512k_on_macos() {
    // 8 canales (7.1) → bitrate máximo 512k en macOS.
    let argv = audio_transcode_argv(Some(8));
    assert!(!argv.contains(&"-ac"), "macOS no fuerza downmix con 7.1");
    let br = argv.windows(2).find(|w| w[0] == "-b:a").expect("-b:a");
    assert_eq!(br[1], "512k", "7.1 → 512k en macOS");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn audio_transcode_argv_mono_gives_256k_on_macos() {
    // 1 canal (mono) → suelo 256k.
    let argv = audio_transcode_argv(Some(1));
    let br = argv.windows(2).find(|w| w[0] == "-b:a").expect("-b:a");
    assert_eq!(br[1], "256k", "mono → 256k en macOS");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn audio_transcode_argv_always_starts_with_c_a_aac_on_macos() {
    // El primer argumento siempre debe ser el par -c:a aac independientemente
    // del número de canales.
    for ch in [None, Some(1), Some(2), Some(6), Some(8)] {
        let argv = audio_transcode_argv(ch);
        assert_eq!(argv[0], "-c:a", "primer arg: {argv:?}");
        assert_eq!(argv[1], "aac", "segundo arg: {argv:?}");
    }
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn audio_transcode_argv_scales_bitrate_by_channels_on_macos() {
    assert_eq!(audio_transcode_argv(None).last().copied(), Some("256k"));
    assert_eq!(audio_transcode_argv(Some(2)).last().copied(), Some("256k"));
    assert_eq!(audio_transcode_argv(Some(6)).last().copied(), Some("384k"));
    assert_eq!(audio_transcode_argv(Some(8)).last().copied(), Some("512k"));
}

// Integración con ffmpeg real. Genera un MKV sintético
// H.264 + E-AC-3 5.1 (lavfi + audiotestsrc en 6ch), lo pasa
// por la MISMA argv que `spawn_hls` construye para non-macos,
// y verifica con ffprobe que el segmento resultante lleva
// `codec_name=aac` y `channels=2`. Criterio de aceptación:
// "reproduce" (2ch, aac, TS válido), no solo "transcodea".
//
// Ignorado por defecto — requiere ffmpeg + ffprobe en PATH.
// CI Windows lo activa con `-- --ignored windows_aac_downmix`
// tras `choco install ffmpeg -y`.
#[cfg(all(test, not(target_os = "macos")))]
#[test]
#[ignore]
fn windows_aac_downmix_produces_stereo() {
    use std::process::Command as StdCommand;
    let ffmpeg = which::which("ffmpeg").expect("ffmpeg en PATH");
    let ffprobe = which::which("ffprobe").expect("ffprobe en PATH");

    let td = tempfile::tempdir().unwrap();
    let fixture = td.path().join("fixture-5_1.mkv");

    // Fixture: 2 s de vídeo `testsrc` (H.264) + audio `sine`
    // multiplicado a 6 canales, encodeado E-AC-3 5.1 en MKV.
    // Suficiente para forzar el codec/layout que Chromium
    // rechazaría sin el `-ac 2` del backend.
    let out = StdCommand::new(&ffmpeg)
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=320x240:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=48000",
            "-filter_complex",
            "[1:a]pan=5.1|FL=c0|FR=c0|FC=c0|LFE=c0|BL=c0|BR=c0[a5_1]",
            "-map",
            "0:v",
            "-map",
            "[a5_1]",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "eac3",
            "-ac",
            "6",
            "-b:a",
            "384k",
        ])
        .arg(&fixture)
        .output()
        .expect("spawn ffmpeg (fixture)");
    assert!(
        out.status.success(),
        "fixture ffmpeg falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Transcode con la argv EXACTA de la rama non-macos.
    // Prependemos `-i <fixture>` y appendeamos un output TS
    // one-shot para verificar el segmento resultante.
    let segment = td.path().join("out.ts");
    let audio_args = audio_transcode_argv(Some(6));
    let mut cmd = StdCommand::new(&ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(&fixture)
        .args(["-map", "0:v:0", "-map", "0:a:0", "-c:v", "copy"])
        .args(&audio_args)
        .args(["-t", "1", "-f", "mpegts"])
        .arg(&segment);
    let trans = cmd.output().expect("spawn ffmpeg (transcode)");
    assert!(
        trans.status.success(),
        "transcode falló: {}",
        String::from_utf8_lossy(&trans.stderr)
    );

    // Verificación con ffprobe. Buscamos EL stream de audio y
    // exigimos codec_name=aac + channels=2. "Reproduce" real:
    // el TS carga en un `<video>` de Chromium sin
    // kUnsupportedConfig.
    let probe = StdCommand::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,channels",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(&segment)
        .output()
        .expect("spawn ffprobe");
    let stdout = String::from_utf8_lossy(&probe.stdout);
    assert!(
        stdout.contains("codec_name=aac"),
        "segmento debe llevar AAC, ffprobe: {stdout}"
    );
    assert!(
        stdout.contains("channels=2"),
        "segmento debe ser estéreo (Chromium rechaza >2ch), ffprobe: {stdout}"
    );
}

// ── hw_encoder_argv + vt_bitrate_for_height ──────────────────
//
// Tests unitarios sin ffmpeg. Verifican:
//   1. La rejilla NO-negociable de args comunes NO se cuela en
//      `hw_encoder_argv` (`-force_key_frames`, `-pix_fmt`, `-bf`,
//      `-avoid_negative_ts` los añade el caller SIEMPRE).
//   2. El encoder correcto se emite por vendor.
//   3. Los buckets de bitrate por altura cubren los 4 casos
//      documentados.

#[cfg(test)]
#[test]
fn vt_bitrate_buckets_by_height() {
    use super::argv::vt_bitrate_for_height;
    assert_eq!(vt_bitrate_for_height(Some(240)), "2M");
    assert_eq!(vt_bitrate_for_height(Some(480)), "2M");
    assert_eq!(vt_bitrate_for_height(Some(720)), "4M");
    assert_eq!(vt_bitrate_for_height(Some(1080)), "8M");
    assert_eq!(vt_bitrate_for_height(Some(2160)), "10M");
    // None cae al bucket 1080p (conservador: no infra-bitratear un
    // 4K desconocido).
    assert_eq!(vt_bitrate_for_height(None), "8M");
}

#[cfg(test)]
#[test]
fn hw_encoder_argv_starts_with_c_v_and_correct_encoder() {
    use crate::ffmpeg::HwEncoder;
    for (encoder, expected_name) in [
        (HwEncoder::VideoToolbox, "h264_videotoolbox"),
        (HwEncoder::Nvenc, "h264_nvenc"),
        (HwEncoder::Qsv, "h264_qsv"),
        (HwEncoder::Amf, "h264_amf"),
    ] {
        let argv = super::argv::hw_encoder_argv(encoder, Some(1080));
        assert_eq!(argv[0], "-c:v");
        assert_eq!(argv[1], expected_name, "encoder mismatch: {argv:?}");
    }
}

#[cfg(test)]
#[test]
fn hw_encoder_argv_omits_common_grid_args() {
    // Los args de la rejilla (`-force_key_frames`, `-pix_fmt`,
    // `-bf`, `-avoid_negative_ts`) los añade `spawn_hls` fuera
    // de esta función para AMBAS ramas (libx264 y hw). Que
    // aparezcan aquí duplicaría y ffmpeg tomaría el último —
    // más contamos con no colar el bug.
    use crate::ffmpeg::HwEncoder;
    for encoder in [
        HwEncoder::VideoToolbox,
        HwEncoder::Nvenc,
        HwEncoder::Qsv,
        HwEncoder::Amf,
    ] {
        let argv = super::argv::hw_encoder_argv(encoder, Some(1080));
        for banned in ["-force_key_frames", "-pix_fmt", "-bf", "-avoid_negative_ts"] {
            assert!(
                !argv.iter().any(|a| a == banned),
                "{banned} debe añadirlo el caller, no hw_encoder_argv({encoder:?}): {argv:?}"
            );
        }
    }
}

#[cfg(test)]
#[test]
fn hw_encoder_argv_videotoolbox_scales_bitrate_with_height() {
    use crate::ffmpeg::HwEncoder;
    let argv_1080 = super::argv::hw_encoder_argv(HwEncoder::VideoToolbox, Some(1080));
    let argv_2160 = super::argv::hw_encoder_argv(HwEncoder::VideoToolbox, Some(2160));
    // El -b:v de 4K debe ser mayor que el de 1080p.
    let bitrate_of = |argv: &[String]| -> String {
        argv.windows(2)
            .find(|w| w[0] == "-b:v")
            .map(|w| w[1].clone())
            .expect("-b:v")
    };
    assert_eq!(bitrate_of(&argv_1080), "8M");
    assert_eq!(bitrate_of(&argv_2160), "10M");
}
