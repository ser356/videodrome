//! Índice de keyframes por HTTP Range reads — audit §2.
//!
//! El pipeline HLS en modo `-c:v copy` necesita saber en qué
//! timestamps EXISTEN keyframes en el fichero original: los cortes
//! de segmento HLS no se pueden forzar (copy = demux+mux, no
//! re-encode). Sin este índice, ffmpeg cortaría en cualquier frame
//! y los segmentos resultantes no serían auto-suficientes → hls.js
//! y WKWebView los rechazan.
//!
//! La trampa que este módulo EVITA: `ffprobe -show_frames` sobre
//! el fichero descargaría el archivo entero para enumerar keyframes.
//! Inaceptable sobre torrent. La vía correcta es leer el ÍNDICE
//! del contenedor, que vive en zonas pequeñas y direccionables por
//! HTTP Range:
//!
//!   * MKV: el elemento `Cues` (bloque tabular de CuePoint) vive
//!     normalmente al final del Segment; el `SeekHead` inicial
//!     (primer bloque después del EBML header) apunta a su offset.
//!     Total leído: header + SeekHead + Info + Tracks + Cues =
//!     típicamente <50 KB incluso para pelis de 4 horas.
//!
//!   * MP4/MOV: tablas `stss` (sync sample table) + `stts` (sample
//!     timing) dentro de `moov`. La mayoría de releases MP4 fuerzan
//!     `moov` al principio (releases "web-optimized"); en el resto
//!     está al final y hay que hacer un tail-fetch. TODO: soporte
//!     MP4 en un follow-up (v1: solo MKV, que cubre >80% del
//!     catálogo de releases 1080p/2160p según el audit).
//!
//! Si el índice no se puede leer (contenedor no soportado, SeekHead
//! ausente, GOPs patológicos > 10s, etc.), retornamos error y el
//! caller cae a transcode — nunca fail-open sobre copy.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::header;

/// Índice de keyframes del stream de vídeo principal.
#[derive(Debug, Clone)]
pub struct KeyframeIndex {
    /// Timestamps de keyframes en segundos, monótonos crecientes
    /// empezando en 0.0 (o el primer keyframe si el stream no
    /// arranca en I-frame — raro pero posible). Siempre al menos
    /// una entrada.
    pub timestamps: Vec<f64>,
    /// Duración total del stream de vídeo en segundos, si el
    /// contenedor la declara.
    pub duration: Option<f64>,
    /// Formato de origen detectado. Solo informativo (para logs).
    #[allow(dead_code)]
    pub source: KeyframeSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeSource {
    /// Leído del elemento MKV `Cues`.
    MkvCues,
}

impl KeyframeIndex {
    /// Máximo hueco entre keyframes consecutivos, en segundos. Se
    /// usa para detectar GOPs patológicos (>10s) que harían el
    /// seek en copy inaceptable — audit §2d.
    pub fn max_gap_seconds(&self) -> f64 {
        self.timestamps
            .windows(2)
            .map(|w| w[1] - w[0])
            .fold(0.0_f64, f64::max)
    }

    /// Devuelve el timestamp del keyframe ≤ `t`. Se usa como argv
    /// `-ss` de ffmpeg para arrancar un segmento HLS en frontera
    /// EXACTA de keyframe (requisito para que los segmentos sean
    /// intercambiables entre jobs). `t=0` devuelve el primer
    /// keyframe (típicamente 0.0).
    #[allow(dead_code)]
    pub fn keyframe_at_or_before(&self, t: f64) -> f64 {
        // El vector está ordenado; binary_search_by con partial_cmp
        // devuelve el índice del primer elemento >= t, así que
        // retrocedemos uno.
        match self
            .timestamps
            .binary_search_by(|k| k.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(idx) => self.timestamps[idx],
            Err(0) => self.timestamps[0],
            Err(idx) => self.timestamps[idx - 1],
        }
    }

    /// Agrupa keyframes en "segmentos virtuales" cuya duración es
    /// ≥ `target_seconds`. Devuelve la lista de (start_time,
    /// duration) — el playlist HLS de duración variable se
    /// construye desde aquí. Audit §2b.
    ///
    /// El último segmento se ajusta a `duration` real si se
    /// conoce, para no declarar EXTINF que supere el fin del
    /// fichero (Safari/hls.js son estrictos con eso).
    pub fn variable_segments(&self, target_seconds: f64) -> Vec<(f64, f64)> {
        if self.timestamps.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut seg_start = self.timestamps[0];
        for &next in &self.timestamps[1..] {
            let seg_len = next - seg_start;
            if seg_len >= target_seconds {
                out.push((seg_start, seg_len));
                seg_start = next;
            }
        }
        // Último segmento: hasta la duración total del stream si
        // la conocemos; si no, hasta el último keyframe (que es lo
        // que sabemos con certeza).
        let end = self.duration.unwrap_or(
            *self
                .timestamps
                .last()
                .expect("timestamps non-empty: checked at fn start"),
        );
        let last_len = (end - seg_start).max(0.001);
        out.push((seg_start, last_len));
        out
    }
}

// ── HTTP client con Range ─────────────────────────────────────

/// Cliente HTTP para el fetch de índices. Se pasa el `reqwest::Client`
/// desde fuera para reutilizar la conexión (nuestro servidor local
/// está en `127.0.0.1`, un solo host).
async fn ranged_get(client: &reqwest::Client, url: &str, start: u64, end: u64) -> Result<Vec<u8>> {
    let resp = client
        .get(url)
        .header(header::RANGE, format!("bytes={start}-{end}"))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .with_context(|| format!("GET {url} Range={start}-{end}"))?;
    // Toleramos tanto 206 (Partial Content, lo esperable) como 200
    // (algunos servidores no honran Range para ficheros pequeños).
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status} en Range={start}-{end}");
    }
    Ok(resp.bytes().await?.to_vec())
}

/// Averigua la longitud total del recurso. HEAD es lo canónico pero
/// nuestro servidor `serve_video` no responde a HEAD explícito, así
/// que hacemos un GET Range de 1 byte y leemos `Content-Range`
/// (`bytes 0-0/12345`) para extraer el total.
async fn resource_length(client: &reqwest::Client, url: &str) -> Result<u64> {
    let resp = client
        .get(url)
        .header(header::RANGE, "bytes=0-0")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .with_context(|| format!("GET {url} Range=0-0"))?;
    if let Some(cr) = resp.headers().get(header::CONTENT_RANGE) {
        let s = cr.to_str().unwrap_or_default();
        // Formato: "bytes 0-0/TOTAL"
        if let Some(total) = s.rsplit('/').next() {
            if let Ok(n) = total.parse::<u64>() {
                return Ok(n);
            }
        }
    }
    // Fallback: Content-Length (será 1 si tuvo Range, o el total
    // si el server ignoró el header).
    resp.headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .context("No se pudo determinar tamaño del recurso")
}

// ── MKV / EBML parser (mínimo, hand-rolled) ───────────────────
//
// Solo implementamos los elementos que necesitamos:
//   EBML Header (para saltarlo)
//   Segment                     18 53 80 67
//     SeekHead                  11 4D 9B 74
//       Seek                    4D BB
//         SeekID                53 AB
//         SeekPosition          53 AC
//     Info                      15 49 A9 66
//       TimecodeScale           2A D7 B1  (u64, ns por tick)
//       Duration                44 89     (f64, en ticks)
//     Tracks                    16 54 AE 6B
//       TrackEntry              AE
//         TrackNumber           D7
//         TrackType             83        (1 = video)
//     Cues                      1C 53 BB 6B
//       CuePoint                BB
//         CueTime               B3        (en ticks)
//         CueTrackPositions     B7
//           CueTrack            F7
//
// EBML VINT: primer byte tiene 0..=8 bits de "marca de longitud" en
// el prefijo (leading zeros = longitud extra en bytes). El bit
// líder de la marca es el separador; el resto de bits (con los
// siguientes bytes) es el valor.

const ID_EBML_HEADER: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3];
const ID_SEGMENT: &[u8] = &[0x18, 0x53, 0x80, 0x67];
const ID_SEEK_HEAD: &[u8] = &[0x11, 0x4D, 0x9B, 0x74];
const ID_SEEK: &[u8] = &[0x4D, 0xBB];
const ID_SEEK_ID: &[u8] = &[0x53, 0xAB];
const ID_SEEK_POSITION: &[u8] = &[0x53, 0xAC];
const ID_INFO: &[u8] = &[0x15, 0x49, 0xA9, 0x66];
const ID_TIMECODE_SCALE: &[u8] = &[0x2A, 0xD7, 0xB1];
const ID_DURATION: &[u8] = &[0x44, 0x89];
const ID_TRACKS: &[u8] = &[0x16, 0x54, 0xAE, 0x6B];
const ID_TRACK_ENTRY: &[u8] = &[0xAE];
const ID_TRACK_NUMBER: &[u8] = &[0xD7];
const ID_TRACK_TYPE: &[u8] = &[0x83];
const ID_CUES: &[u8] = &[0x1C, 0x53, 0xBB, 0x6B];
const ID_CUE_POINT: &[u8] = &[0xBB];
const ID_CUE_TIME: &[u8] = &[0xB3];
const ID_CUE_TRACK_POSITIONS: &[u8] = &[0xB7];
const ID_CUE_TRACK: &[u8] = &[0xF7];

const TRACK_TYPE_VIDEO: u64 = 1;
const DEFAULT_TIMECODE_SCALE_NS: u64 = 1_000_000;

/// Lee un VINT desde `buf[pos]`. Devuelve `(valor, longitud_en_bytes)`.
/// El valor NO tiene la máscara de longitud aplicada — es el
/// "unsigned integer" con los bits reales. Se usa tanto para IDs
/// (leaving the marker in) como para sizes (removing it) según el
/// llamador.
fn read_vint_raw(buf: &[u8], pos: usize) -> Result<(u64, usize)> {
    if pos >= buf.len() {
        bail!("VINT read fuera de buffer @ {pos}");
    }
    let first = buf[pos];
    if first == 0 {
        bail!("VINT ilegal (byte 0)");
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        bail!("VINT longitud > 8 bytes");
    }
    if pos + len > buf.len() {
        bail!("VINT trunco @ {pos} (necesito {len} bytes)");
    }
    let mut val: u64 = 0;
    for i in 0..len {
        val = (val << 8) | buf[pos + i] as u64;
    }
    Ok((val, len))
}

/// Lee un VINT de SIZE (con la máscara de longitud eliminada).
fn read_vint_size(buf: &[u8], pos: usize) -> Result<(u64, usize)> {
    let (raw, len) = read_vint_raw(buf, pos)?;
    // Máscara de longitud: bit alto del primer byte. Ejemplos:
    //   len=1: mask = 0x7F <<  0 =                   0x7F
    //   len=2: mask = 0x3F << 8  =                 0x3FFF
    //   len=3: mask = 0x1F << 16 =               0x1FFFFF
    //   ...
    let mask = (1u64 << (7 * len)) - 1;
    Ok((raw & mask, len))
}

/// Lee un element ID leaving the marker in (para comparar con las
/// constantes ID_*).
fn read_element_id(buf: &[u8], pos: usize) -> Result<(&[u8], usize)> {
    let (_raw, len) = read_vint_raw(buf, pos)?;
    Ok((&buf[pos..pos + len], len))
}

/// Cabecera de elemento EBML: (id_bytes, size, header_len).
struct EbmlHeader<'a> {
    id: &'a [u8],
    size: u64,
    header_len: usize,
}

fn read_element_header(buf: &[u8], pos: usize) -> Result<EbmlHeader<'_>> {
    let (id, id_len) = read_element_id(buf, pos)?;
    let (size, size_len) = read_vint_size(buf, pos + id_len)?;
    Ok(EbmlHeader {
        id,
        size,
        header_len: id_len + size_len,
    })
}

fn read_uint(buf: &[u8], len: usize) -> u64 {
    let mut v = 0u64;
    for &byte in &buf[..len] {
        v = (v << 8) | byte as u64;
    }
    v
}

fn read_float(buf: &[u8], len: usize) -> Result<f64> {
    match len {
        4 => {
            let arr: [u8; 4] = buf[..4].try_into().map_err(|_| {
                anyhow::anyhow!(
                    "read_float: buffer too short for f32 (got {} bytes)",
                    buf.len()
                )
            })?;
            Ok(f32::from_be_bytes(arr) as f64)
        }
        8 => {
            let arr: [u8; 8] = buf[..8].try_into().map_err(|_| {
                anyhow::anyhow!(
                    "read_float: buffer too short for f64 (got {} bytes)",
                    buf.len()
                )
            })?;
            Ok(f64::from_be_bytes(arr))
        }
        n => bail!("float EBML con longitud rara: {n}"),
    }
}

/// Busca la posición absoluta donde arranca el DATA del `Segment`
/// (después de su cabecera). Devuelve `(segment_data_start,
/// segment_data_size_or_none)`. `None` en el size indica
/// "unknown/streaming" (0xFF... en el VINT) — común en MKV.
fn find_segment_start(buf: &[u8], base_offset: u64) -> Result<(u64, Option<u64>)> {
    let mut pos = 0;
    // Puede haber cero o un EBML Header al inicio. Skip si aparece.
    if buf.len() >= 4 && &buf[..4] == ID_EBML_HEADER {
        let hdr = read_element_header(buf, pos).context("EBML header")?;
        pos += hdr.header_len + hdr.size as usize;
    }
    // El siguiente elemento DEBE ser Segment.
    let hdr = read_element_header(buf, pos).context("Segment header")?;
    if hdr.id != ID_SEGMENT {
        bail!(
            "Esperaba Segment (0x18538067), encontré 0x{}",
            hex_id(hdr.id)
        );
    }
    let data_start = base_offset + (pos + hdr.header_len) as u64;
    // Un size con todos los bits del "valor" a 1 significa "unknown"
    // (VINT streaming). Detectar por comparación con la máscara.
    let size = if is_unknown_size_vint(&buf[pos + hdr.header_len - vint_size_len(hdr.header_len)..])
    {
        None
    } else {
        Some(hdr.size)
    };
    Ok((data_start, size))
}

/// Longitud en bytes del VINT de SIZE embebido en un header EBML de
/// longitud total `header_len`, sabiendo que la ID también es un
/// VINT. Truco: header_len = id_len + size_len; contamos id_len
/// releyendo. Como shortcut, lo redescubrimos usando VINT rules.
fn vint_size_len(header_len: usize) -> usize {
    // No podemos deducirlo sin volver a leer el ID. Este helper solo
    // se usa para el chequeo "unknown size" de arriba, que asume
    // que el size VINT arranca justo antes del final del header.
    // En la práctica el caller nos pasa un slice apuntando al SIZE
    // (no al ID), así que leemos el VINT rules del primer byte.
    // Devolvemos 1 si no podemos deducirlo — el "unknown" solo
    // ocurre en 8 bytes normalmente (0xFF FF FF FF FF FF FF FF).
    header_len.min(8)
}

/// Un size VINT "unknown" (streaming) tiene TODOS los bits del
/// valor a 1. Con `len=1`: 0x7F. Con `len=8`: 0xFFFFFFFFFFFFFFFF.
fn is_unknown_size_vint(buf: &[u8]) -> bool {
    if buf.is_empty() {
        return false;
    }
    let Ok((raw, len)) = read_vint_raw(buf, 0) else {
        return false;
    };
    let mask = (1u64 << (7 * len)) - 1;
    (raw & mask) == mask
}

fn hex_id(id: &[u8]) -> String {
    id.iter().map(|b| format!("{b:02X}")).collect()
}

/// Escanea un master element (`Info`, `Tracks`, `Cues`, ...)
/// llamando a `on_child` por cada elemento hijo con su id, offset
/// dentro del payload, y size. Se detiene cuando ha consumido
/// `master_size` bytes.
fn scan_master<F: FnMut(&[u8], usize, u64) -> Result<()>>(
    payload: &[u8],
    master_size: u64,
    mut on_child: F,
) -> Result<()> {
    let end = (master_size as usize).min(payload.len());
    let mut pos = 0;
    while pos < end {
        let hdr = read_element_header(payload, pos)?;
        let data_pos = pos + hdr.header_len;
        if data_pos + hdr.size as usize > payload.len() {
            // Elemento se sale del buffer que nos pasaron — cortar
            // silenciosamente (el caller ha fetched menos de lo
            // necesario). No es un error fatal si el elemento no
            // era el que buscábamos.
            break;
        }
        on_child(hdr.id, data_pos, hdr.size)?;
        pos = data_pos + hdr.size as usize;
    }
    Ok(())
}

/// Punto de entrada MKV: parsea header + SeekHead + Info + Tracks
/// + Cues vía Range reads sobre `url`.
///
/// Los offsets del SeekHead son RELATIVOS al inicio del data del
/// Segment.
pub async fn fetch_mkv_keyframes(client: &reqwest::Client, url: &str) -> Result<KeyframeIndex> {
    // 1) Header + inicio del Segment + primer SeekHead.
    let total_len = resource_length(client, url).await?;
    let head_end = (65_535).min(total_len.saturating_sub(1));
    let head = ranged_get(client, url, 0, head_end).await?;

    let (segment_data_start, _segment_data_size) =
        find_segment_start(&head, 0).context("MKV: no encontré Segment en los primeros 64 KB")?;
    let seg_rel_start = segment_data_start as usize;
    let seek_head_payload = &head[seg_rel_start..];

    // El primer elemento del Segment normalmente ES el SeekHead.
    let sh_hdr = read_element_header(seek_head_payload, 0)
        .context("MKV: header del primer elemento del Segment")?;
    if sh_hdr.id != ID_SEEK_HEAD {
        // Algunos archivos ponen Info o SSA primero — buscamos
        // el SeekHead escaneando los primeros elementos del Segment.
        // Simplificamos: intentamos leer el siguiente elemento hasta
        // 3 veces buscando SeekHead.
        return fetch_from_scanning(client, url, &head, segment_data_start).await;
    }

    // Parseamos SeekHead → mapa {ID → offset}.
    let mut targets = std::collections::HashMap::<Vec<u8>, u64>::new();
    let sh_start = sh_hdr.header_len;
    let sh_end = sh_start + sh_hdr.size as usize;
    if sh_end > seek_head_payload.len() {
        // SeekHead no cabe entero en el buffer inicial — hacer un
        // fetch adicional del rango que falta.
        let need_start = segment_data_start + sh_start as u64;
        let need_end = segment_data_start + sh_end as u64 - 1;
        let more = ranged_get(client, url, need_start, need_end).await?;
        parse_seek_head(&more, sh_hdr.size, &mut targets)?;
    } else {
        parse_seek_head(
            &seek_head_payload[sh_start..sh_end],
            sh_hdr.size,
            &mut targets,
        )?;
    }

    // 2) Buscar los offsets que necesitamos: Info, Tracks, Cues.
    let info_off = targets.get(ID_INFO).copied();
    let tracks_off = targets.get(ID_TRACKS).copied();
    let cues_off = targets
        .get(ID_CUES)
        .copied()
        .context("MKV sin Cues (fichero sin índice — copy no viable)")?;

    // 3) Fetch Info (TimecodeScale + Duration).
    let (timecode_scale_ns, duration_ticks) = match info_off {
        Some(off) => fetch_info(client, url, segment_data_start + off).await?,
        None => (DEFAULT_TIMECODE_SCALE_NS, None),
    };
    let tick_seconds = timecode_scale_ns as f64 / 1_000_000_000.0;
    let duration_seconds = duration_ticks.map(|t| t * tick_seconds);

    // 4) Fetch Tracks → video track number.
    let video_track = match tracks_off {
        Some(off) => fetch_video_track_number(client, url, segment_data_start + off).await?,
        None => {
            // Sin Tracks explícito no sabemos qué track es vídeo.
            // Asumimos 1 (el track number por defecto en la mayoría
            // de MKVs con un solo vídeo). Si el fichero tiene layout
            // raro nos devolverá 0 CuePoints y saldremos por el
            // check final.
            1
        }
    };

    // 5) Fetch Cues → timestamps del video track.
    let timestamps = fetch_cues(
        client,
        url,
        segment_data_start + cues_off,
        video_track,
        tick_seconds,
        total_len,
    )
    .await?;

    if timestamps.is_empty() {
        bail!("MKV Cues no contenía keyframes para el track {video_track}");
    }

    Ok(KeyframeIndex {
        timestamps,
        duration: duration_seconds,
        source: KeyframeSource::MkvCues,
    })
}

/// Escanea Segment secuencialmente buscando SeekHead o (si no lo
/// hay) Info/Tracks/Cues directos. Fallback para archivos raros.
async fn fetch_from_scanning(
    _client: &reqwest::Client,
    _url: &str,
    _head: &[u8],
    _segment_data_start: u64,
) -> Result<KeyframeIndex> {
    // v1: no soportamos MKV sin SeekHead al inicio (raro pero
    // existe). El caller cae a transcode.
    bail!("MKV sin SeekHead al inicio del Segment (v1 no soportado)")
}

/// Parsea el payload de un SeekHead escribiendo entries
/// {SeekID → SeekPosition} en `out`.
fn parse_seek_head(
    payload: &[u8],
    size: u64,
    out: &mut std::collections::HashMap<Vec<u8>, u64>,
) -> Result<()> {
    scan_master(payload, size, |id, data_pos, sub_size| {
        if id != ID_SEEK {
            return Ok(());
        }
        // Payload del Seek: SeekID + SeekPosition.
        let mut cur = data_pos;
        let end = data_pos + sub_size as usize;
        let mut seek_id: Option<Vec<u8>> = None;
        let mut seek_pos: Option<u64> = None;
        while cur < end {
            let hdr = read_element_header(payload, cur)?;
            let data = cur + hdr.header_len;
            let dend = data + hdr.size as usize;
            if dend > payload.len() {
                break;
            }
            if hdr.id == ID_SEEK_ID {
                // SeekID es a su vez un VINT (el ID del elemento
                // buscado, con la marca de longitud incluida).
                seek_id = Some(payload[data..dend].to_vec());
            } else if hdr.id == ID_SEEK_POSITION {
                seek_pos = Some(read_uint(&payload[data..dend], hdr.size as usize));
            }
            cur = dend;
        }
        if let (Some(id), Some(pos)) = (seek_id, seek_pos) {
            out.insert(id, pos);
        }
        Ok(())
    })
}

/// Fetches Info element y devuelve (TimecodeScale ns, Duration en ticks).
async fn fetch_info(
    client: &reqwest::Client,
    url: &str,
    abs_offset: u64,
) -> Result<(u64, Option<f64>)> {
    // 4 KB debería ser más que suficiente para Info.
    let end = abs_offset + 4096;
    let buf = ranged_get(client, url, abs_offset, end).await?;
    let hdr = read_element_header(&buf, 0).context("Info header")?;
    if hdr.id != ID_INFO {
        bail!("Esperaba Info, encontré 0x{}", hex_id(hdr.id));
    }
    let payload = &buf[hdr.header_len..];
    let mut scale = DEFAULT_TIMECODE_SCALE_NS;
    let mut duration: Option<f64> = None;
    scan_master(payload, hdr.size, |id, data_pos, sub_size| {
        let dend = data_pos + sub_size as usize;
        if dend > payload.len() {
            return Ok(());
        }
        if id == ID_TIMECODE_SCALE {
            scale = read_uint(&payload[data_pos..dend], sub_size as usize);
        } else if id == ID_DURATION {
            duration = Some(read_float(&payload[data_pos..dend], sub_size as usize)?);
        }
        Ok(())
    })?;
    Ok((scale, duration))
}

/// Busca el TrackNumber del primer TrackEntry con TrackType=video.
async fn fetch_video_track_number(
    client: &reqwest::Client,
    url: &str,
    abs_offset: u64,
) -> Result<u64> {
    // Tracks suele ser <32 KB.
    let end = abs_offset + 32_768;
    let buf = ranged_get(client, url, abs_offset, end).await?;
    let hdr = read_element_header(&buf, 0).context("Tracks header")?;
    if hdr.id != ID_TRACKS {
        bail!("Esperaba Tracks, encontré 0x{}", hex_id(hdr.id));
    }
    let payload = &buf[hdr.header_len..];
    let mut result: Option<u64> = None;
    scan_master(payload, hdr.size, |id, data_pos, sub_size| {
        if id != ID_TRACK_ENTRY || result.is_some() {
            return Ok(());
        }
        let mut cur = data_pos;
        let end = data_pos + sub_size as usize;
        let mut track_num: Option<u64> = None;
        let mut track_type: Option<u64> = None;
        while cur < end && cur < payload.len() {
            let sub = read_element_header(payload, cur)?;
            let data = cur + sub.header_len;
            let dend = data + sub.size as usize;
            if dend > payload.len() {
                break;
            }
            if sub.id == ID_TRACK_NUMBER {
                track_num = Some(read_uint(&payload[data..dend], sub.size as usize));
            } else if sub.id == ID_TRACK_TYPE {
                track_type = Some(read_uint(&payload[data..dend], sub.size as usize));
            }
            cur = dend;
        }
        if track_type == Some(TRACK_TYPE_VIDEO) {
            if let Some(n) = track_num {
                result = Some(n);
            }
        }
        Ok(())
    })?;
    result.ok_or_else(|| anyhow!("Tracks sin pista de vídeo (TrackType=1)"))
}

/// Descarga y parsea Cues → timestamps ordenados del `video_track`.
async fn fetch_cues(
    client: &reqwest::Client,
    url: &str,
    abs_offset: u64,
    video_track: u64,
    tick_seconds: f64,
    total_len: u64,
) -> Result<Vec<f64>> {
    // Primero leemos el header de Cues (unos pocos bytes) para saber
    // su size — luego decidimos si hace falta un segundo fetch.
    let probe_end = (abs_offset + 15).min(total_len.saturating_sub(1));
    let probe = ranged_get(client, url, abs_offset, probe_end).await?;
    let hdr = read_element_header(&probe, 0).context("Cues header")?;
    if hdr.id != ID_CUES {
        bail!("Esperaba Cues, encontré 0x{}", hex_id(hdr.id));
    }
    let total_needed = hdr.header_len as u64 + hdr.size;
    let end = (abs_offset + total_needed).min(total_len).saturating_sub(1);
    // Safety cap: Cues gigante (>16 MB) huele a fichero raro. Cortamos.
    if end - abs_offset > 16 * 1024 * 1024 {
        bail!("Cues > 16 MB — sospechoso, abortamos");
    }
    let buf = ranged_get(client, url, abs_offset, end).await?;
    let hdr = read_element_header(&buf, 0)?;
    let payload = &buf[hdr.header_len..];
    let mut out = Vec::<f64>::new();
    scan_master(payload, hdr.size, |id, data_pos, sub_size| {
        if id != ID_CUE_POINT {
            return Ok(());
        }
        let mut cur = data_pos;
        let end = data_pos + sub_size as usize;
        let mut cue_time: Option<u64> = None;
        let mut cue_track: Option<u64> = None;
        while cur < end && cur < payload.len() {
            let sub = read_element_header(payload, cur)?;
            let data = cur + sub.header_len;
            let dend = data + sub.size as usize;
            if dend > payload.len() {
                break;
            }
            if sub.id == ID_CUE_TIME {
                cue_time = Some(read_uint(&payload[data..dend], sub.size as usize));
            } else if sub.id == ID_CUE_TRACK_POSITIONS {
                // Payload del CTP: CueTrack + CueClusterPosition + ...
                let mut ctp_pos = data;
                let ctp_end = data + sub.size as usize;
                while ctp_pos < ctp_end && ctp_pos < payload.len() {
                    let inner = read_element_header(payload, ctp_pos)?;
                    let idata = ctp_pos + inner.header_len;
                    let iend = idata + inner.size as usize;
                    if iend > payload.len() {
                        break;
                    }
                    if inner.id == ID_CUE_TRACK {
                        cue_track = Some(read_uint(&payload[idata..iend], inner.size as usize));
                    }
                    ctp_pos = iend;
                }
            }
            cur = dend;
        }
        if let (Some(t), Some(tr)) = (cue_time, cue_track) {
            if tr == video_track {
                out.push(t as f64 * tick_seconds);
            }
        }
        Ok(())
    })?;
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
    Ok(out)
}

// ── Entry point ───────────────────────────────────────────────

/// Devuelve el índice de keyframes de `url` intentando cada
/// parser conocido según el contenedor. Se llama con
/// `container` = el `MediaInfo.container` que reporta ffprobe
/// (`"matroska,webm"` para MKV, `"mov,mp4,..."` para MP4).
pub async fn fetch_keyframe_index(
    client: &reqwest::Client,
    url: &str,
    container: Option<&str>,
) -> Result<KeyframeIndex> {
    let c = container.unwrap_or("").to_ascii_lowercase();
    if c.contains("matroska") || c.contains("webm") {
        fetch_mkv_keyframes(client, url).await
    } else if c.contains("mp4") || c.contains("mov") {
        // TODO: implementar stss+stts parser. Por ahora, sin
        // keyframe index para MP4 → el caller cae a transcode
        // (que es lo que hacíamos antes) o a DIRECT si la matriz
        // del audit §1 lo permite.
        bail!("Container MP4/MOV: keyframe index no implementado en v1")
    } else {
        bail!("Container '{c}' no soportado por fetch_keyframe_index")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vint_size_1_byte() {
        let buf = [0x81]; // 1000_0001 → len=1, val=1 (con máscara)
        let (val, len) = read_vint_size(&buf, 0).unwrap();
        assert_eq!(len, 1);
        assert_eq!(val, 1);
    }

    #[test]
    fn vint_size_2_bytes() {
        let buf = [0x40, 0x02]; // 0100_0000 0000_0010 → len=2, val=2
        let (val, len) = read_vint_size(&buf, 0).unwrap();
        assert_eq!(len, 2);
        assert_eq!(val, 2);
    }

    #[test]
    fn keyframe_at_or_before_exact() {
        let idx = KeyframeIndex {
            timestamps: vec![0.0, 4.0, 8.0, 12.0],
            duration: Some(16.0),
            source: KeyframeSource::MkvCues,
        };
        assert_eq!(idx.keyframe_at_or_before(0.0), 0.0);
        assert_eq!(idx.keyframe_at_or_before(4.0), 4.0);
        assert_eq!(idx.keyframe_at_or_before(5.5), 4.0);
        assert_eq!(idx.keyframe_at_or_before(11.99), 8.0);
        assert_eq!(idx.keyframe_at_or_before(12.0), 12.0);
        assert_eq!(idx.keyframe_at_or_before(999.0), 12.0);
    }

    #[test]
    fn variable_segments_regular_gops() {
        let idx = KeyframeIndex {
            timestamps: vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0],
            duration: Some(12.0),
            source: KeyframeSource::MkvCues,
        };
        let segs = idx.variable_segments(4.0);
        // Con target 4s: agrupa (0,4), (4,4), y cierra el último con
        // duration = 12 → (8, 4).
        assert_eq!(segs.len(), 3);
        assert!((segs[0].0 - 0.0).abs() < 1e-6);
        assert!((segs[0].1 - 4.0).abs() < 1e-6);
        assert!((segs[1].0 - 4.0).abs() < 1e-6);
        assert!((segs[2].0 - 8.0).abs() < 1e-6);
        assert!((segs[2].1 - 4.0).abs() < 1e-6);
    }

    #[test]
    fn max_gap_seconds_computes() {
        let idx = KeyframeIndex {
            timestamps: vec![0.0, 4.0, 15.0, 19.0],
            duration: None,
            source: KeyframeSource::MkvCues,
        };
        assert!((idx.max_gap_seconds() - 11.0).abs() < 1e-6);
    }
}
