//! Provider YTS (yts.mx). API JSON pública, sin auth. Solo cine.
//! Docs: <https://yts.mx/api>

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;

use super::{build_magnet, MovieQuery, Torrent, TorrentProvider};

const BASE: &str = "https://yts.mx/api/v2/list_movies.json";

pub struct Yts;

#[derive(Debug, Deserialize)]
struct YtsResponse {
    data: YtsData,
}

#[derive(Debug, Deserialize, Default)]
struct YtsData {
    #[serde(default)]
    movies: Option<Vec<YtsMovie>>,
}

#[derive(Debug, Deserialize)]
struct YtsMovie {
    title_long: String,
    #[serde(default)]
    year: Option<u16>,
    #[serde(default)]
    imdb_code: String,
    #[serde(default)]
    torrents: Vec<YtsTorrent>,
}

#[derive(Debug, Deserialize)]
struct YtsTorrent {
    hash: String,
    #[serde(default)]
    quality: String,
    #[serde(default)]
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    seeds: u32,
    #[serde(default)]
    peers: u32,
    #[serde(default)]
    size_bytes: u64,
}

#[async_trait]
impl TorrentProvider for Yts {
    fn name(&self) -> &'static str {
        "yts"
    }

    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>> {
        // Preferimos IMDb ID (mucho más preciso). Si no, caemos al título.
        let query_term = q.imdb_id.clone().unwrap_or_else(|| q.title.clone());

        let url = format!(
            "{BASE}?query_term={}&limit=5&sort_by=seeds&order_by=desc",
            urlencoding::encode(&query_term)
        );

        let resp: YtsResponse = http
            .get(&url)
            .send()
            .await
            .context("Error de red hacia YTS")?
            .json()
            .await
            .context("Error al parsear respuesta de YTS")?;

        let mut out = Vec::new();
        for m in resp.data.movies.unwrap_or_default() {
            // Si el usuario pidió un año concreto y YTS reporta otro,
            // toleramos ±1 (cine internacional se estrena en distintos
            // años según país; TMDB suele dar la fecha USA).
            if let (Some(want), Some(got)) = (q.year, m.year) {
                if (want as i32 - got as i32).abs() > 1 {
                    continue;
                }
            }

            for t in m.torrents {
                let display = format!("{} [{}] {}", m.title_long, t.quality, t.kind);
                let magnet = build_magnet(&t.hash, &display);
                out.push(Torrent {
                    title: display,
                    magnet,
                    size_bytes: t.size_bytes,
                    seeders: t.seeds,
                    leechers: t.peers,
                    quality: Some(t.quality),
                    source: "yts",
                    infohash: t.hash.to_ascii_uppercase(),
                });
            }

            // Log de qué peli acertamos, útil si el usuario ve resultados raros.
            if !m.imdb_code.is_empty() && q.imdb_id.as_deref() == Some(&m.imdb_code) {
                // match perfecto por IMDb; no seguimos comparando otras pelis.
                break;
            }
        }

        Ok(out)
    }
}
