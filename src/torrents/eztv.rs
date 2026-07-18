//! Provider EZTV (eztv.re). API JSON pública, sin auth. Solo series.
//! Docs: <https://eztv.re/api/>
//!
//! Endpoint:
//!
//!   GET https://eztv.re/api/get-torrents?imdb_id=<digits>&limit=100&page=N
//!
//! Devuelve JSON con `torrents[] { hash, filename, title, season, episode,
//! magnet_url, seeds, peers, size_bytes, ... }`. Todos los campos
//! season/episode vienen YA parseados por EZTV (no hay que adivinar
//! desde el filename), lo que hace este provider particularmente
//! preciso para packs que otros indexers etiquetan mal.
//!
//! Es el análogo de YTS para series: direccionable por IMDb, ruido
//! bajo, sin auth. Para películas devuelve vacío en silencio.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;

use super::{build_magnet, MovieQuery, Torrent, TorrentProvider};

const BASE: &str = "https://eztv.re/api/get-torrents";

pub struct Eztv;

#[derive(Debug, Deserialize)]
struct EztvResponse {
    #[serde(default)]
    torrents: Vec<EztvItem>,
}

#[derive(Debug, Deserialize)]
struct EztvItem {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    magnet_url: String,
    /// EZTV los expone como strings ("1", "01") — parseamos ambos.
    #[serde(default)]
    season: Option<serde_json::Value>,
    #[serde(default)]
    episode: Option<serde_json::Value>,
    #[serde(default)]
    seeds: u32,
    #[serde(default)]
    peers: u32,
    #[serde(default)]
    size_bytes: SizeBytes,
}

/// EZTV a veces devuelve `size_bytes` como string ("734003200") en
/// vez de número. Aceptamos ambos.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum SizeBytes {
    Int(u64),
    Str(String),
    #[default]
    Missing,
}

impl SizeBytes {
    fn into_u64(self) -> u64 {
        match self {
            SizeBytes::Int(n) => n,
            SizeBytes::Str(s) => s.parse().unwrap_or(0),
            SizeBytes::Missing => 0,
        }
    }
}

fn parse_num(v: &Option<serde_json::Value>) -> Option<u16> {
    let v = v.as_ref()?;
    if let Some(n) = v.as_u64() {
        return u16::try_from(n).ok();
    }
    if let Some(s) = v.as_str() {
        return s.parse().ok();
    }
    None
}

#[async_trait]
impl TorrentProvider for Eztv {
    fn name(&self) -> &'static str {
        "eztv"
    }

    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>> {
        // Solo series. Cualquier otra kind devuelve vacío en silencio
        // — no queremos ensuciar el ProviderStatus con un fallo que
        // es simplemente "fuera de scope".
        if !matches!(q.kind, crate::tmdb::MediaKind::Series) {
            return Ok(Vec::new());
        }
        // EZTV solo direcciona por IMDb (y sin el prefijo "tt"). Sin
        // ID no hay forma de pegarle — mejor devolver vacío que
        // hacer una petición sin foco.
        let Some(imdb) = q.imdb_id.as_deref() else {
            return Ok(Vec::new());
        };
        let imdb_num = imdb.trim_start_matches("tt");
        if imdb_num.is_empty() || !imdb_num.chars().all(|c| c.is_ascii_digit()) {
            return Ok(Vec::new());
        }

        let url = format!("{BASE}?imdb_id={imdb_num}&limit=100&page=1");
        let resp = http
            .get(&url)
            .send()
            .await
            .context("Error de red hacia EZTV")?
            .error_for_status()
            .context("EZTV devolvió error HTTP")?;
        let parsed: EztvResponse = resp
            .json()
            .await
            .context("Error al parsear respuesta de EZTV")?;

        let mut out = Vec::with_capacity(parsed.torrents.len());
        for it in parsed.torrents {
            // Filtro por season/episode pedidos: si el user pidió
            // S02E03, no devolvemos S01E04. Si pidió S02 (pack),
            // aceptamos episodios de S02 + el propio pack (episode
            // ausente o 0). Si no pidió nada, todo pasa.
            let s = parse_num(&it.season);
            let e = parse_num(&it.episode);
            if let Some(qs) = q.season {
                match s {
                    Some(ss) if ss == qs => {}
                    _ => continue,
                }
                if let Some(qe) = q.episode {
                    match e {
                        Some(ee) if ee == qe => {}
                        // Episode 0 en EZTV suele ser season pack.
                        Some(0) | None => {}
                        _ => continue,
                    }
                }
            }

            let magnet = if it.magnet_url.starts_with("magnet:") {
                it.magnet_url
            } else if !it.hash.is_empty() {
                build_magnet(&it.hash, &it.title)
            } else {
                continue;
            };
            let hash_up = it.hash.to_ascii_uppercase();
            if hash_up.is_empty() {
                continue;
            }

            out.push(Torrent {
                title: it.title.clone(),
                magnet,
                size_bytes: it.size_bytes.into_u64(),
                seeders: it.seeds,
                leechers: it.peers,
                // EZTV no expone quality como campo — se infiere del
                // título con `quality_from_title` (720p/1080p/2160p).
                quality: super::quality_from_title(&it.title),
                source: "eztv".to_string(),
                match_kind: crate::torrents::MatchKind::default(),
                infohash: hash_up,
            });
        }

        Ok(out)
    }
}
