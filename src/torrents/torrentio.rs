//! Provider Torrentio (Stremio addon público). API JSON sin auth,
//! CORS habilitado, cero rate-limit apreciable. Es un META-agregador:
//! internamente consulta RARBG-legacy, 1337x, TPB, YTS, EZTV, KAT,
//! TorrentGalaxy, MagnetDL, Rutor, MejorTorrent, Wolfmax4k,
//! Cinecalidad, etc. y devuelve MAGNETS DEDUPLICADOS con release
//! name, seeders, size y tracker origen ya extraídos.
//!
//! Para SERIES es la fuente más rica del ecosistema — devuelve
//! episodios sueltos, season packs y series packs mezclados,
//! ordenados por calidad/tamaño. Además da `fileIdx` (el índice del
//! fichero dentro del torrent) resuelto, así podemos saltarnos la
//! heurística de `select_file` para packs con numeración rara.
//!
//! Endpoint:
//!
//!   GET https://<host>/<config>/stream/<type>/<id>.json
//!
//! - `<type>` = `movie` | `series`
//! - `<id>` = `tt<imdb>` para pelis, `tt<imdb>:<S>:<E>` para episodios
//! - `<config>` (opcional): providers=…|sort=qualitysize|qualityfilter=…
//!
//! Response:
//!
//! ```json
//! {"streams": [{
//!   "name": "Torrentio\n1080p WEB-DL",
//!   "title": "Fargo.S02E03.1080p.WEB-DL...\n👤 42 💾 1.2 GB ⚙️ 1337x",
//!   "infoHash": "abc...",
//!   "fileIdx": 0,
//!   "sources": ["dht:...", "tracker:udp://..."]
//! }]}
//! ```

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::{quality_from_title, MovieQuery, Torrent, TorrentProvider};

/// Lista de hosts Torrentio a probar en orden. El primero que
/// responda 200 con JSON gana.
///
///   1. `torrentio.strem.fun` — oficial. Cloudflare + rate-limit
///      generoso, uptime muy alto.
///   2. `torrentio-akram.baby` — mirror comunitario, mismo backend.
///
/// A diferencia de YTS/EZTV, `strem.fun` NO suele estar en las
/// listas DNS de bloqueo por ISP (el dominio no aparece en las
/// blocklists comerciales de streaming pirata — se distribuye como
/// "addon Stremio" que muchos ISP no reconocen como target).
const TORRENTIO_HOSTS: &[&str] = &[
    "https://torrentio.strem.fun",
    "https://torrentio-akram.baby",
];

/// Timeout por host — 4s, mayor que EZTV/YTS porque Torrentio
/// tiene que consultar internamente a 10+ trackers y agregar la
/// respuesta. En cold-cache la latencia legítima ronda los 2-3s.
const TORRENTIO_HOST_TIMEOUT: Duration = Duration::from_millis(4000);

/// Config path: activa el conjunto MÁS AMPLIO de sources posible
/// (para maximizar recall), ordena por qualitysize (los 1080p WEB
/// arriba, los CAM abajo), y filtra CAM/SCR/unknown quality (spam).
///
/// Los `providers=` disponibles se descubren desde el manifest:
/// yts, eztv, rarbg, 1337x, thepiratebay, kickasstorrents,
/// torrentgalaxy, magnetdl, horriblesubs, nyaasi, tokyotosho,
/// anidex, rutor, rutracker, comando, bludv, torrent9,
/// ilcorsaronero, mejortorrent, wolfmax4k, cinecalidad.
///
/// Los `qualityfilter=` posibles: scr, cam, unknown, 3d.
///
/// Los `sort=` posibles: quality (agrupa por 2160/1080/720…),
/// qualitysize (calidad + tamaño), size, seeders.
///
/// Formato del path: `key1=v1,v2|key2=v3` (pipe entre keys, coma
/// entre values). Va URL-safe: NO usar espacios ni `%`.
const CONFIG: &str = concat!(
    "providers=yts,eztv,rarbg,1337x,thepiratebay,kickasstorrents,",
    "torrentgalaxy,magnetdl,rutor,rutracker,mejortorrent,wolfmax4k,",
    "cinecalidad,nyaasi",
    "|sort=qualitysize",
    "|qualityfilter=scr,cam,unknown"
);

pub struct Torrentio;

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    streams: Vec<Stream>,
}

#[derive(Debug, Deserialize)]
struct Stream {
    /// Header con nombre del provider + quality tag ("Torrentio\n1080p WEB").
    /// Ignorado — sacamos el quality del `title` real.
    #[serde(default)]
    #[allow(dead_code)]
    name: String,
    /// Body multi-línea:
    ///   Fargo.S02E03.1080p.WEB-DL.x264-GRP
    ///   👤 42 💾 1.2 GB ⚙️ 1337x
    #[serde(default)]
    title: String,
    #[serde(default, rename = "infoHash")]
    info_hash: String,
    /// Índice del fichero de vídeo dentro del torrent (0-based). Se
    /// mapea directo a `Torrent.file_hint` — mata la ambigüedad de
    /// packs con numeración rara (anime absoluto, etc.).
    #[serde(default, rename = "fileIdx")]
    file_idx: Option<usize>,
    /// Trackers extra ("tracker:udp://…", "dht:hash"). Los
    /// pasamos al magnet como `&tr=` para maximizar peers.
    #[serde(default)]
    sources: Vec<String>,
}

#[async_trait]
impl TorrentProvider for Torrentio {
    fn name(&self) -> &'static str {
        "torrentio"
    }

    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>> {
        // Torrentio direcciona SOLO por IMDb id. Sin id no hay forma
        // de pegarle — mejor devolver vacío que hacer una petición
        // sin foco. En el flujo GUI el imdb_id viene resuelto vía
        // TMDB antes de llamar aquí; sin él la búsqueda directa por
        // texto ya está cubierta por knaben/apibay.
        let Some(imdb) = q.imdb_id.as_deref() else {
            return Ok(Vec::new());
        };
        let imdb_id = imdb.trim();
        if !imdb_id.starts_with("tt") || imdb_id.len() < 3 {
            return Ok(Vec::new());
        }

        // Path: /<config>/stream/<type>/<id>.json
        let (kind_str, id_str) = match (q.kind, q.season, q.episode) {
            (crate::tmdb::MediaKind::Series, Some(s), Some(e)) => {
                ("series", format!("{imdb_id}:{s}:{e}"))
            }
            (crate::tmdb::MediaKind::Series, Some(s), None) => {
                // Pack de temporada: Torrentio interpreta `tt<imdb>:S:1`
                // devolviendo el episodio 1 + packs de temporada. Aún
                // sin episodio pedido probamos con E=1 porque los
                // packs se devuelven de todas formas y filtramos
                // client-side por `parsed.season` en `search_all`.
                ("series", format!("{imdb_id}:{s}:1"))
            }
            (crate::tmdb::MediaKind::Series, None, _) => {
                // Sin S/E no sabemos qué pedir. Skipeamos.
                return Ok(Vec::new());
            }
            (crate::tmdb::MediaKind::Movie, _, _) => ("movie", imdb_id.to_string()),
        };
        let path = format!("/{CONFIG}/stream/{kind_str}/{id_str}.json");

        let response = fetch_from_any_host(http, &path).await?;

        let mut out = Vec::with_capacity(response.streams.len());
        for s in response.streams {
            if s.info_hash.is_empty() {
                continue;
            }
            // Extraemos release name y metadata (seeders, size, tracker
            // origen) del `title` multi-línea.
            let parsed_meta = parse_title(&s.title);
            let release_name = parsed_meta.release_name;
            let seeders = parsed_meta.seeders;
            let size_bytes = parsed_meta.size_bytes;
            let source_tracker = parsed_meta.source_tracker;

            let magnet = build_magnet_with_sources(&s.info_hash, &release_name, &s.sources);
            let quality = quality_from_title(&release_name);

            out.push(Torrent {
                title: release_name,
                magnet,
                size_bytes,
                seeders,
                // Torrentio no expone leechers — dejamos 0. El
                // score usa solo seeders.
                leechers: 0,
                quality,
                // Etiquetamos como `torrentio:1337x` cuando el
                // sub-tracker es identificable. Ayuda a depurar
                // ("¿por qué este release solo vino por
                // torrentio?"). Fallback a `torrentio` a secas.
                source: match source_tracker {
                    Some(t) => format!("torrentio:{t}"),
                    None => "torrentio".to_string(),
                },
                match_kind: crate::torrents::MatchKind::default(),
                // Aquí está la joya: fileIdx pre-resuelto para packs.
                // `search_all` lo pasa al Torrent y el player lo usa
                // en `start_with_target` saltándose `select_file`.
                file_hint: s.file_idx,
                infohash: s.info_hash.to_ascii_uppercase(),
            });
        }

        Ok(out)
    }
}

struct ParsedMeta {
    release_name: String,
    seeders: u32,
    size_bytes: u64,
    source_tracker: Option<String>,
}

/// Parsea el `title` multi-línea de Torrentio. Formato típico:
///
///   Fargo.S02E03.1080p.WEB-DL.x264-GRP
///   👤 42 💾 1.2 GB ⚙️ 1337x
///
/// A veces el release name ocupa varias líneas (para nombres largos
/// con `/`) o el metadata line trae más emojis. Tomamos la primera
/// línea como release name y buscamos los emojis conocidos en el
/// resto para extraer números — cualquier cosa distinta se ignora.
fn parse_title(raw: &str) -> ParsedMeta {
    let mut lines = raw.lines();
    let release_name = lines.next().unwrap_or("").trim().to_string();
    let meta_line: String = lines.collect::<Vec<_>>().join(" ");

    let seeders = extract_after_emoji(&meta_line, '\u{1F464}')
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let size_bytes = extract_after_emoji(&meta_line, '\u{1F4BE}')
        .map(parse_size)
        .unwrap_or(0);
    let source_tracker = extract_after_emoji(&meta_line, '\u{2699}').map(|s| s.to_lowercase());

    ParsedMeta {
        release_name,
        seeders,
        size_bytes,
        source_tracker,
    }
}

/// Devuelve el "token" que sigue al emoji dado, hasta el próximo
/// emoji conocido o fin de string. Trim aplicado. `None` si el emoji
/// no aparece o no hay nada después.
///
/// Consume el selector de variación `\u{FE0F}` (VS16) que sigue a
/// algunos emojis "text-default" como ⚙ para forzar presentación
/// emoji ("⚙️"). Sin este skip, el value extraído sería `"️ 1337x"`
/// con un carácter invisible al principio que rompía tests y
/// mostraba raro en logs.
fn extract_after_emoji(s: &str, emoji: char) -> Option<String> {
    let idx = s.find(emoji)?;
    let mut after = &s[idx + emoji.len_utf8()..];
    if let Some(rest) = after.strip_prefix('\u{FE0F}') {
        after = rest;
    }
    // Corta en el próximo emoji conocido: 👤 (seeders), 💾 (size),
    // ⚙ (source), o los que Torrentio pueda añadir en el futuro
    // (banderas 🇺🇸🇪🇸🇮🇳, 🎬, 🌐…). Los regional indicators (`\u{1F1E6}..=\u{1F1FF}`)
    // aparecen tras `⚙️` cuando Torrentio adjunta la bandera del
    // idioma de audio detectado — sin este stop ese emoji entraría
    // en el token del tracker y saldría `"TorrentGalaxy 🇮🇳"`.
    let stop = [
        '\u{1F464}', // 👤
        '\u{1F4BE}', // 💾
        '\u{2699}',  // ⚙
        '\u{1F3AC}', // 🎬
        '\u{1F310}', // 🌐
    ];
    let end = after
        .char_indices()
        .find(|(_, c)| stop.contains(c) || is_flag_regional(*c))
        .map(|(i, _)| i)
        .unwrap_or(after.len());
    let token = after[..end].trim();
    if token.is_empty() {
        None
    } else {
        // Para seeders queremos solo el primer número; para size
        // queremos "1.2 GB". Devolvemos el token entero y que el
        // caller decida — `parse::<u32>()` de un `"42"` va bien;
        // `parse_size` maneja `"1.2 GB"`.
        Some(token.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// Regional Indicator Symbols (🇦..🇿) — la base Unicode de las
/// banderas. Torrentio los adjunta tras `⚙️` como bandera del
/// idioma de audio detectado; sin este stop entran en el token del
/// tracker.
fn is_flag_regional(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

/// "1.2 GB", "540 MB", "1 TB" → bytes. Case-insensitive. Sin unidad
/// asume bytes. Fallback 0 si no se puede parsear.
fn parse_size(s: String) -> u64 {
    let s = s.trim();
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.is_empty() {
        return 0;
    }
    let Some(num) = parts[0].replace(',', ".").parse::<f64>().ok() else {
        return 0;
    };
    let unit = parts
        .get(1)
        .map(|u| u.to_ascii_uppercase())
        .unwrap_or_default();
    let mult: f64 = match unit.as_str() {
        "TB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        "MB" => 1024.0 * 1024.0,
        "KB" => 1024.0,
        _ => 1.0,
    };
    (num * mult) as u64
}

/// Construye el magnet incluyendo trackers extra de Torrentio (los
/// `tracker:udp://…` y `dht:hash` del array `sources[]`). Reutiliza
/// `build_magnet` (que ya añade los trackers "genéricos" universales)
/// y le añade los específicos.
fn build_magnet_with_sources(infohash: &str, name: &str, sources: &[String]) -> String {
    let mut m = super::build_magnet(infohash, name);
    for src in sources {
        // Formato Torrentio: `tracker:<url>` o `dht:<hash>` (ignoramos
        // dht — librqbit ya hace DHT por defecto). Solo interesan los
        // `tracker:` que aportan URL adicional.
        if let Some(url) = src.strip_prefix("tracker:") {
            m.push_str("&tr=");
            m.push_str(&urlencoding::encode(url));
        }
    }
    m
}

/// Prueba cada host de `TORRENTIO_HOSTS` en orden hasta que uno
/// responda 200 con JSON parseable. Mismo patrón que `yts` y `eztv`:
/// errores en un host se loguean y se sigue con el siguiente; solo
/// cuando TODOS fallan devolvemos Err.
async fn fetch_from_any_host(http: &reqwest::Client, path: &str) -> Result<Response> {
    let mut last_err: Option<anyhow::Error> = None;
    for host in TORRENTIO_HOSTS {
        let url = format!("{host}{path}");
        let fut = http.get(&url).send();
        let resp = match tokio::time::timeout(TORRENTIO_HOST_TIMEOUT, fut).await {
            Err(_) => {
                tracing::warn!(target: "torrentio", host, "timeout");
                last_err = Some(anyhow::anyhow!("timeout {host}"));
                continue;
            }
            Ok(Err(e)) => {
                tracing::warn!(target: "torrentio", host, error = %e, "red");
                last_err = Some(anyhow::Error::from(e));
                continue;
            }
            Ok(Ok(r)) => r,
        };
        let status = resp.status();
        if !status.is_success() {
            tracing::warn!(target: "torrentio", host, %status, "HTTP error");
            last_err = Some(anyhow::anyhow!("{host} HTTP {status}"));
            continue;
        }
        match resp.json::<Response>().await {
            Ok(parsed) => return Ok(parsed),
            Err(e) => {
                tracing::warn!(target: "torrentio", host, error = %e, "parse");
                last_err = Some(anyhow::Error::from(e));
                continue;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Sin hosts Torrentio disponibles")))
        .context("Todos los mirrors de Torrentio fallaron")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_title_extracts_release_seeders_size_tracker() {
        let raw = "Fargo.S02E03.1080p.WEB-DL.x264-GRP\n👤 42 💾 1.2 GB ⚙️ 1337x";
        let p = parse_title(raw);
        assert_eq!(p.release_name, "Fargo.S02E03.1080p.WEB-DL.x264-GRP");
        assert_eq!(p.seeders, 42);
        assert_eq!(p.size_bytes, (1.2 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(p.source_tracker.as_deref(), Some("1337x"));
    }

    #[test]
    fn parse_title_handles_missing_metadata_line() {
        let raw = "Some.Release.Name.Only";
        let p = parse_title(raw);
        assert_eq!(p.release_name, "Some.Release.Name.Only");
        assert_eq!(p.seeders, 0);
        assert_eq!(p.size_bytes, 0);
        assert!(p.source_tracker.is_none());
    }

    #[test]
    fn parse_title_stops_tracker_at_flag_emoji() {
        // Caso real observado 2026-07-18: Torrentio añade la bandera
        // del idioma detectado tras `⚙️`. Sin stop on regional
        // indicators el tracker capturaba "TorrentGalaxy 🇮🇳".
        let raw =
            "Planet Earth S02 [Hindi Dub] 1080p BDRip Saicord\n👤 0 💾 1.53 GB ⚙️ TorrentGalaxy 🇮🇳";
        let p = parse_title(raw);
        assert_eq!(p.source_tracker.as_deref(), Some("torrentgalaxy"));
    }

    #[test]
    fn parse_size_units() {
        assert_eq!(parse_size("1 TB".to_string()), 1024u64.pow(4));
        assert_eq!(
            parse_size("1.5 GB".to_string()),
            (1.5 * 1024f64.powi(3)) as u64
        );
        assert_eq!(parse_size("500 MB".to_string()), 500 * 1024 * 1024);
        assert_eq!(parse_size("garbage".to_string()), 0);
        // Comma decimal (es-ES formatting de algunos releases).
        assert_eq!(
            parse_size("1,5 GB".to_string()),
            (1.5 * 1024f64.powi(3)) as u64
        );
    }

    #[test]
    fn build_magnet_with_sources_appends_trackers() {
        let m = build_magnet_with_sources(
            "0123456789abcdef0123456789abcdef01234567",
            "Foo",
            &[
                "tracker:udp://example.com:1337/announce".to_string(),
                "dht:0123456789abcdef".to_string(), // ignorado
            ],
        );
        assert!(m.contains("udp%3A%2F%2Fexample.com%3A1337%2Fannounce"));
        // dht: no debe aparecer como &tr=
        assert!(!m.contains("&tr=dht"));
    }
}
