//! Wrappers finos sobre `ffmpeg` y `ffprobe` para el player HTML nativo.
//!
//! El player embebido (view `Player.tsx` en el frontend) consume un
//! `<video>` que apunta a un endpoint HTTP local. Detrás de ese endpoint
//! spawneamos ffmpeg en modo transmux (`-c copy`) para repackagear el
//! contenedor del torrent (típicamente MKV) a fMP4 fragmentado — que sí
//! reproducen WKWebView / WebView2 / WebKitGTK sin plugins.
//!
//! El binario se busca primero en PATH y, si falla (típico en macOS al
//! abrir la app desde Launchpad/Finder — el PATH heredado no incluye
//! `/opt/homebrew/bin`), en las rutas fijas [`FALLBACK_DIRS`]. Al
//! arranque comprobamos con [`is_available`]; si falla, el frontend
//! cae al fallback VLC. La distribución (Homebrew cask, Scoop, Nix)
//! declara `ffmpeg` como dependencia para que el user no tenga que
//! instalarlo a mano.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::winutil::HideConsoleExt;

// ── Windows: resolver de shims de Scoop ───────────────────────────────────
//
// `which::which("ffprobe")` en máquinas con ffmpeg instalado por Scoop
// devuelve `~\scoop\shims\ffprobe.exe`, que es el `shim.exe` genérico
// de Scoop. Ese proxy spawnea al binario real y sale, pero NO propaga
// la terminación: al llamar a `child.kill()` (probe cancelado, kill de
// sesión HLS, `kill_on_drop`) matamos al SHIM y el ffmpeg/ffprobe real
// queda huérfano indefinidamente — combinado con la ausencia de
// `CREATE_NO_WINDOW` (ver `winutil::HideConsoleExt`), ese huérfano
// arrastra su propia ventana de consola visible "para siempre".
//
// La cura es resolver el shim a su destino real leyendo el fichero de
// metadata `<nombre>.shim` adyacente ANTES de spawnear. Con el
// binario real spawneado directamente, `kill()` vuelve a comportarse
// igual que en macOS/Linux.

/// Resuelve un shim de Scoop (`~\scoop\shims\<name>.exe`) a la ruta
/// real del binario destino. Cada shim tiene adyacente un fichero de
/// metadata `<name>.shim` con formato:
///
/// ```text
/// path = "C:\Users\foo\scoop\apps\ffmpeg\current\bin\ffprobe.exe"
/// args = ...
/// ```
///
/// Si no hay `.shim` (binario normal, winget, install manual) o el
/// path no apunta a un fichero existente, devolvemos la ruta original.
///
/// NOTA sobre Chocolatey: choco usa `shimgen` que embebe la ruta
/// DENTRO del binario shim (no hay fichero legible al lado). Aquí no
/// lo resolvemos; si aparece el mismo síntoma con instalaciones de
/// choco, la vía correcta es priorizar la ruta real
/// (`chocolatey\lib\ffmpeg\tools\ffmpeg\bin\...`) en
/// `windows_fallback_dirs` por delante de `chocolatey\bin`.
#[cfg(windows)]
fn resolve_scoop_shim(p: &Path) -> PathBuf {
    let meta = p.with_extension("shim");
    let Ok(text) = std::fs::read_to_string(&meta) else {
        return p.to_path_buf();
    };
    for line in text.lines() {
        let t = line.trim_start();
        if !t.starts_with("path") {
            continue;
        }
        let Some(raw) = t.splitn(2, '=').nth(1) else {
            break;
        };
        let real = PathBuf::from(raw.trim().trim_matches('"'));
        if real.is_file() {
            return real;
        }
        break;
    }
    p.to_path_buf()
}

#[cfg(not(windows))]
#[inline]
fn resolve_scoop_shim(p: &Path) -> PathBuf {
    p.to_path_buf()
}

/// Nombre del binario según el SO. Windows añade `.exe`.
#[cfg(target_os = "windows")]
const FFMPEG_BIN: &str = "ffmpeg.exe";
#[cfg(target_os = "windows")]
const FFPROBE_BIN: &str = "ffprobe.exe";
#[cfg(not(target_os = "windows"))]
const FFMPEG_BIN: &str = "ffmpeg";
#[cfg(not(target_os = "windows"))]
const FFPROBE_BIN: &str = "ffprobe";

/// Rutas fijas donde buscar los binarios cuando `which::which` falla.
///
/// Motivo: en macOS las apps GUI (Launchpad / Finder / `open`) heredan
/// un PATH stub (`/usr/bin:/bin:/usr/sbin:/sbin`) que NO incluye
/// `/opt/homebrew/bin` ni `/usr/local/bin`, así que aunque el user
/// tenga `brew install ffmpeg`, `which` desde dentro del bundle
/// devuelve `None` y caemos a VLC sin motivo. Miramos las rutas
/// canónicas de Homebrew (arm64 + Intel) y MacPorts como fallback.
/// En Linux/BSD el problema es raro pero cubrimos `/usr/local/bin` por
/// si acaso.
#[cfg(not(target_os = "windows"))]
const FALLBACK_DIRS: &[&str] = &[
    "/opt/homebrew/bin", // Homebrew arm64
    "/usr/local/bin",    // Homebrew Intel + fallback Linux
    "/opt/local/bin",    // MacPorts
    "/usr/bin",          // system (Linux distros)
];

/// Rutas fijas para Windows. Cubre las tres formas típicas de
/// instalar ffmpeg cuando el user NO lo tiene en PATH:
///   * winget (`winget install Gyan.FFmpeg`) — crea shims en
///     `%LOCALAPPDATA%\Microsoft\WinGet\Links`.
///   * scoop (`scoop install ffmpeg`) — shims en `~\scoop\shims`.
///   * Instalación manual desde gyan.dev / BtbN — el usuario
///     descomprime el zip en `C:\ffmpeg\bin` (convención más
///     común aunque no oficial).
///
/// La lista se computa en runtime porque las rutas dependen de
/// variables de entorno (`LOCALAPPDATA`, `USERPROFILE`) que no se
/// pueden expresar como `&'static str`.
#[cfg(target_os = "windows")]
fn windows_fallback_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut v: Vec<PathBuf> = Vec::new();
    // Convención zip manual (más común).
    v.push(PathBuf::from(r"C:\ffmpeg\bin"));
    // winget shims (Gyan.FFmpeg / BtbN.FFmpeg registran binarios
    // aquí — un solo directorio con los .exe symlinkeados).
    if let Ok(lad) = std::env::var("LOCALAPPDATA") {
        v.push(PathBuf::from(lad).join(r"Microsoft\WinGet\Links"));
    }
    // scoop shims (`~\scoop\shims\ffmpeg.exe`).
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(r"scoop\shims"));
    }
    // Chocolatey.
    if let Ok(cd) = std::env::var("ChocolateyInstall") {
        v.push(PathBuf::from(cd).join("bin"));
    } else {
        v.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin"));
    }
    v
}

/// Busca `name` primero por PATH y, si falla, en los `FALLBACK_DIRS`
/// de la plataforma. Solo devuelve `Some` si la ruta existe como
/// fichero. En Windows, el resultado se pasa por `resolve_scoop_shim`
/// para saltarnos los proxies de Scoop y spawnear el binario real
/// (ver comentario del helper — sin esto los `kill()` matan al shim
/// y dejan ffmpeg/ffprobe huérfanos).
fn locate_bin(name: &str) -> Option<PathBuf> {
    if let Ok(p) = which::which(name) {
        return Some(resolve_scoop_shim(&p));
    }
    #[cfg(not(target_os = "windows"))]
    {
        for dir in FALLBACK_DIRS {
            let candidate = PathBuf::from(dir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        for dir in windows_fallback_dirs() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(resolve_scoop_shim(&candidate));
            }
        }
    }
    None
}

/// Ruta al binario `ffmpeg`. `None` si no está instalado. Se usa
/// `which::which` en vez de intentar spawnear para poder distinguir
/// "no está" (mostrar diálogo con instrucciones) de "está pero peta".
pub fn ffmpeg_binary() -> Option<PathBuf> {
    locate_bin(FFMPEG_BIN)
}

/// Ruta al binario `ffprobe`. Ambos se instalan juntos en todas
/// las distros que conocemos, pero comprobamos por separado por si
/// alguien tiene un ffmpeg mínimo sin ffprobe.
pub fn ffprobe_binary() -> Option<PathBuf> {
    locate_bin(FFPROBE_BIN)
}

/// `true` sii ambos binarios están disponibles. Es el gate para
/// activar el player HTML — si falla, el frontend usa VLC.
pub fn is_available() -> bool {
    ffmpeg_binary().is_some() && ffprobe_binary().is_some()
}

/// Cache de filtros disponibles en la build de ffmpeg del user.
/// Se popula on-demand la primera vez que se pregunta y no cambia
/// mientras la app corre.
static FFMPEG_FILTERS: std::sync::OnceLock<std::collections::HashSet<String>> =
    std::sync::OnceLock::new();

/// `true` si la build de ffmpeg del user tiene el filtro `name`.
/// Se usa en `spawn_hls` para decidir la cadena de tonemap: la
/// receta canónica usa `zscale` (requiere `--enable-libzimg` al
/// compilar ffmpeg), que NO viene en muchos builds de Homebrew /
/// scoop / winget. Si `zscale` no está, caemos a `colorspace`
/// (nativo, siempre disponible) — menos preciso pero al menos
/// no explota.
pub fn ffmpeg_has_filter(name: &str) -> bool {
    let filters = FFMPEG_FILTERS.get_or_init(load_ffmpeg_filters);
    filters.contains(name)
}

fn load_ffmpeg_filters() -> std::collections::HashSet<String> {
    // Sync porque solo se llama una vez desde una tarea async.
    // Timeout defensivo: si ffmpeg cuelga (raro), devolvemos set
    // vacío y todos los `has_filter` responden `false`.
    let Some(bin) = ffmpeg_binary() else {
        return std::collections::HashSet::new();
    };
    let out = {
        let mut cmd = std::process::Command::new(bin);
        cmd.args(["-hide_banner", "-filters"]);
        cmd.hide_console();
        cmd.output()
    };
    let Ok(out) = out else {
        return std::collections::HashSet::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    // Formato de línea: " T. name  desc"
    // (T = timeline support, . = commands, etc.).
    // Nos quedamos con la segunda columna.
    let mut set = std::collections::HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        // Ignora la cabecera y las líneas de leyenda.
        if trimmed.is_empty() || trimmed.starts_with("Filters:") || trimmed.starts_with("---") {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_ascii_whitespace().collect();
        // La primera columna suele ser flags de 3 chars (`T..`);
        // la segunda es el nombre.
        if parts.len() >= 2 && parts[0].len() <= 4 {
            set.insert(parts[1].to_string());
        }
    }
    if set.is_empty() {
        tracing::warn!(target: "ffmpeg", "no pude parsear -filters, tonemap HDR caerá a fallback");
    }
    set
}

// ── ffprobe ────────────────────────────────────────────────────────────────

/// Info que ffprobe devuelve sobre un stream. Solo mapeamos los campos
/// que necesita el player para decidir transmux vs transcode y para
/// mostrar la lista de audio/subs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    pub duration_seconds: Option<f64>,
    pub streams: Vec<StreamInfo>,
    /// Formato de contenedor del input tal cual lo reporta ffprobe
    /// (`matroska,webm` para .mkv, `mov,mp4,m4a,3gp,3g2,mj2` para .mp4).
    /// Se usa solo para logging — la decisión transmux/transcode se
    /// toma por códec de cada stream, no por contenedor.
    pub container: Option<String>,
    /// `true` si el frontend puede alimentar `<video src>` con `/video`
    /// directo, sin pasar por ffmpeg. Es cierto sólo cuando el source
    /// es MP4/MOV con H.264/HEVC + AAC/MP3 — el WebView los reproduce
    /// nativamente con seek por HTTP Range. Se calcula al final de
    /// `probe()` para tenerlo listo cuando el frontend decide el src.
    #[serde(default)]
    pub direct_playable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInfo {
    pub index: u32,
    pub kind: StreamKind,
    /// Nombre del códec según ffmpeg (`h264`, `hevc`, `aac`, `subrip`…).
    pub codec: String,
    /// Idioma ISO 639-2 si el track lo declara (`eng`, `spa`, `jpn`).
    /// Los MKV suelen tenerlo; los MP4 casi nunca.
    pub language: Option<String>,
    /// `title` tag del stream (raro pero útil, ej. "Director's commentary").
    pub title: Option<String>,
    /// Solo para video: `width`.
    pub width: Option<u32>,
    /// Solo para video: `height`.
    pub height: Option<u32>,
    /// Solo para video: pixel format (`yuv420p`, `yuv420p10le`, ...).
    /// Se usa para detectar 10-bit — WKWebView y WebView2 solo
    /// decodifican yuv420p 8-bit vía `<video>`, así que 10-bit
    /// requiere transcode.
    #[serde(default)]
    pub pix_fmt: Option<String>,
    /// Solo para video: profile (`Main`, `Main 10`, `High`, ...).
    /// Redundante con `pix_fmt` para HEVC pero ffprobe a veces
    /// solo lo reporta por aquí.
    #[serde(default)]
    pub profile: Option<String>,
    /// Solo para video: color transfer characteristics (BT.709,
    /// SMPTE 2084 / arib-std-b67 = HDR). Se usa en `try_build_copy_grid`
    /// para bailar HDR de la ruta copy (HLS-TS + copy + HDR es un
    /// campo de minas; se transcodea con tonemap) y en el spawn de
    /// transcode para meter la cadena de filtros zscale+tonemap.
    #[serde(default)]
    pub color_transfer: Option<String>,
    /// Solo para audio: número de canales (1=mono, 2=stereo, 6=5.1,
    /// 8=7.1). Se usa en `spawn_hls` para elegir bitrate AAC sin
    /// forzar downmix a estéreo (preservación de multicanal).
    #[serde(default)]
    pub channels: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
    Other,
}

/// Corre `ffprobe -v error -print_format json -show_streams -show_format
/// <url>` y parsea el JSON. `url` es normalmente el endpoint HTTP local
/// del stream de librqbit — ffprobe lo consume vía Range requests y solo
/// lee los primeros MB (cabecera + índice), así que no bloquea la
/// descarga completa.
pub async fn probe(url: &str) -> Result<MediaInfo> {
    let bin = ffprobe_binary().context("ffprobe no est\u{e1} en PATH")?;
    let started = std::time::Instant::now();
    tracing::info!(target: "probe", url = %url, "start");
    let mut cmd = Command::new(bin);
    cmd.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_streams",
        "-show_format",
        url,
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
    cmd.hide_console();

    // Timeout defensivo: si librqbit está descargando un swarm muerto
    // (o el moov está al final del fichero sin priorizar), ffprobe se
    // queda leyendo `/video` para siempre. 20 s es holgado para
    // cabeceras de MP4/MKV con Range OK, corto para no hacer esperar
    // al user cuando de verdad no va a llegar.
    //
    // El `spawn` + `wait_with_output` (en vez de `.output()` a secas)
    // permite que, al hacer timeout, el `Child` se drope dentro del
    // future de `tokio::time::timeout` y `kill_on_drop(true)` mate al
    // proceso real. Con el shim de Scoop resuelto en `locate_bin`,
    // ese kill ahora sí alcanza al ffprobe.exe destino.
    let child = cmd.spawn().context("Error al spawnear ffprobe")?;
    let out = match tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await {
        Ok(res) => res.context("Error al esperar ffprobe")?,
        Err(_) => {
            tracing::warn!(
                target: "probe",
                url = %url,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "timeout 20s — descarga del inicio del fichero no avanza"
            );
            bail!(
                "no se pudo analizar el v\u{ed}deo: la descarga del inicio del fichero no avanza (timeout ffprobe 20s)"
            );
        }
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        tracing::warn!(
            target: "probe",
            url = %url,
            status = %out.status,
            elapsed_ms = started.elapsed().as_millis() as u64,
            stderr = %stderr.trim(),
            "failed"
        );
        bail!(
            "ffprobe devolvi\u{f3} {} \u{2014} {}",
            out.status,
            stderr.trim()
        );
    }
    tracing::info!(
        target: "probe",
        url = %url,
        elapsed_ms = started.elapsed().as_millis() as u64,
        bytes = out.stdout.len(),
        "done"
    );

    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        streams: Vec<RawStream>,
        #[serde(default)]
        format: Option<RawFormat>,
    }
    #[derive(Deserialize)]
    struct RawFormat {
        #[serde(default)]
        duration: Option<String>,
        #[serde(default)]
        format_name: Option<String>,
    }
    #[derive(Deserialize)]
    struct RawStream {
        index: u32,
        codec_type: String,
        #[serde(default)]
        codec_name: Option<String>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        pix_fmt: Option<String>,
        #[serde(default)]
        profile: Option<String>,
        #[serde(default)]
        color_transfer: Option<String>,
        #[serde(default)]
        channels: Option<u32>,
        #[serde(default)]
        tags: Option<Tags>,
    }
    #[derive(Deserialize)]
    struct Tags {
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        title: Option<String>,
    }

    let raw: Raw = serde_json::from_slice(&out.stdout).context("ffprobe JSON inv\u{e1}lido")?;
    let duration_seconds = raw
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|s| s.parse::<f64>().ok());
    let container = raw.format.and_then(|f| f.format_name);
    let streams = raw
        .streams
        .into_iter()
        .map(|s| {
            let kind = match s.codec_type.as_str() {
                "video" => StreamKind::Video,
                "audio" => StreamKind::Audio,
                "subtitle" => StreamKind::Subtitle,
                _ => StreamKind::Other,
            };
            let (language, title) = match s.tags {
                Some(t) => (t.language, t.title),
                None => (None, None),
            };
            StreamInfo {
                index: s.index,
                kind,
                codec: s.codec_name.unwrap_or_default(),
                language,
                title,
                width: s.width,
                height: s.height,
                pix_fmt: s.pix_fmt,
                profile: s.profile,
                color_transfer: s.color_transfer,
                channels: s.channels,
            }
        })
        .collect();

    let info = MediaInfo {
        duration_seconds,
        streams,
        container,
        direct_playable: false,
    };
    // `direct_playable` NO se rellena aquí: depende de las
    // capacidades del cliente (audit §4). El caller (serve_probe en
    // stream.rs) llama a `compute_direct_playable(&info, &caps)`
    // con los caps reportados por el frontend vía canPlayType.
    Ok(info)
}

// ── Capacidades del cliente + compatibilidad con el player HTML ─────────

/// Códecs que el WebView del cliente sabe decodificar, reportados
/// desde el frontend vía `canPlayType()` en el arranque (audit §4).
/// El backend consume estos tags para decidir DIRECT vs COPY vs
/// TRANSCODE en `spawn_hls` — la matriz deja de ser una whitelist
/// estática y pasa a ser función real del cliente.
///
/// Tags cortos y estables (más fáciles de comparar que MIMEs largos
/// con `codecs="hvc1.2.4.L120.90"`):
///
///   * "h264"      → H.264 8-bit (baseline universal)
///   * "hevc"      → HEVC Main 8-bit
///   * "hevc10"    → HEVC Main 10 (10-bit / HDR SDR-tonemap)
///   * "av1"       → AV1 Main
///   * "vp9"       → VP9 profile 0/2
///   * "aac"       → AAC LC (universal en MP4/TS)
///   * "mp3"       → MPEG audio layer 3
///   * "ac3"       → Dolby Digital (raro en browsers)
///   * "eac3"      → Dolby Digital Plus
///   * "opus"      → Opus (Chromium sí, Safari 17+)
///   * "flac"      → FLAC
///
/// `codecs` vacío = frontend aún no ha reportado. En ese caso el
/// backend usa [`ClientCapabilities::safe_default`] (h264 + aac +
/// mp3), que replica el comportamiento pre-§4.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub codecs: Vec<String>,
}

impl ClientCapabilities {
    /// Fallback conservador: asume solo lo que TODO WebView soporta
    /// nativamente en `<video>`: H.264 8-bit + AAC/MP3. Se usa
    /// cuando el frontend aún no ha registrado sus caps. Es lo más
    /// restrictivo (max ffmpeg work) pero jamás falla al
    /// reproducir.
    pub fn safe_default() -> Self {
        Self {
            codecs: vec!["h264".into(), "aac".into(), "mp3".into()],
        }
    }

    /// `true` si el tag está declarado por el cliente. Comparación
    /// case-insensitive porque canPlayType a veces normaliza a
    /// lowercase, a veces no.
    pub fn supports(&self, tag: &str) -> bool {
        self.codecs.iter().any(|c| c.eq_ignore_ascii_case(tag))
    }
}

/// Códecs de video que aceptamos por el path DIRECT (`<video src>`
/// apuntando a `/video` raw). Requiere que el cliente los declare
/// vía `caps.supports(...)` — la lista pre-§4 (`["h264", "hevc"]`)
/// se conserva como techo semántico: aunque el cliente diga soportar
/// AV1, DIRECT solo va por H.264/HEVC porque `serve_video`
/// devuelve el fichero tal cual (MP4/MOV con esos códecs).
const DIRECT_VIDEO_CODECS: &[&str] = &["h264", "hevc"];

/// Códecs de audio compatibles con el path DIRECT. El resto
/// (opus, flac, vorbis, ac3, eac3, dts, truehd…) obliga a pasar
/// por transmux.
const DIRECT_AUDIO_CODECS: &[&str] = &["aac", "mp3"];

/// `true` si el source es MP4/MOV con códecs ya WebView-compatibles
/// y sin banderas raras (10-bit, 4:2:2/4:4:4, perfiles High 10…),
/// Y además el cliente declara soporte para esos códecs en `caps`.
/// En ese caso el player HTML apunta `<video src>` a `/video`
/// directo — sin subprocess, sin remux, con Range HTTP para seek
/// nativo.
///
/// Todo lo que no cumpla la matriz entra por el path HLS
/// (`spawn_hls` transcodifica o hace copy según §2).
pub fn compute_direct_playable(info: &MediaInfo, caps: &ClientCapabilities) -> bool {
    let Some(video) = info.streams.iter().find(|s| s.kind == StreamKind::Video) else {
        return false;
    };
    if !DIRECT_VIDEO_CODECS.contains(&video.codec.as_str()) {
        return false;
    }
    // El cliente debe declarar soporte para este códec de vídeo.
    // Si aún no ha reportado (safe_default), el cliente tiene h264
    // como mínimo — HEVC quedará excluido correctamente.
    if !caps.supports(&video.codec) {
        return false;
    }
    let audio = info.streams.iter().find(|s| s.kind == StreamKind::Audio);
    let audio_ok = audio
        .map(|s| DIRECT_AUDIO_CODECS.contains(&s.codec.as_str()) && caps.supports(&s.codec))
        .unwrap_or(false);
    if !audio_ok {
        return false;
    }
    // Escape hatch: aunque el códec figure en la whitelist,
    // WKWebView/WebView2 solo decodifican vía `<video>` con
    // chroma yuv420p 8-bit. HEVC "Main 10" (10-bit HDR/BluRay UHD),
    // H.264 High 10 y 4:2:2 leen OK del fichero pero fallan al
    // decodificar → los tratamos como no-direct y los mandamos a
    // HLS con transcode.
    //
    // Excepción: si el cliente declara `hevc10`, permitimos HEVC
    // Main 10 en DIRECT (WKWebView macOS con HW decoder lo hace
    // bien en SDR; HDR con DV metadata sigue siendo transcode+
    // tonemap en HLS — la decisión final la toma spawn_hls).
    let is_10bit_video = video
        .pix_fmt
        .as_deref()
        .map(|p| {
            let p = p.to_ascii_lowercase();
            p.contains("10le") || p.contains("10be") || p.contains("12le") || p.contains("12be")
        })
        .unwrap_or(false)
        || video
            .profile
            .as_deref()
            .map(|p| {
                let p = p.to_ascii_lowercase();
                p.contains("main 10") || p.contains("high 10")
            })
            .unwrap_or(false);
    if is_10bit_video && !caps.supports("hevc10") {
        return false;
    }
    let chroma_bad = video
        .pix_fmt
        .as_deref()
        .map(|p| {
            let p = p.to_ascii_lowercase();
            p.contains("422p") || p.contains("444p")
        })
        .unwrap_or(false);
    let profile_bad = video
        .profile
        .as_deref()
        .map(|p| {
            let p = p.to_ascii_lowercase();
            p.contains("high 4:")
        })
        .unwrap_or(false);
    if chroma_bad || profile_bad {
        return false;
    }
    // El contenedor de origen tiene que ser MP4/MOV (o similares).
    // MKV/AVI aunque lleven H.264 no van por `<video src>` directo
    // — WKWebView solo remuxa MP4 nativamente.
    info.container
        .as_deref()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            c.split(',')
                .any(|part| matches!(part.trim(), "mp4" | "mov" | "m4a" | "3gp" | "3g2" | "mj2"))
        })
        .unwrap_or(false)
}
