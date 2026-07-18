//! Provider apibay.org — API pública de The Pirate Bay. JSON simple, sin
//! auth, sin rate-limit agresivo, muy fiable. Endpoint:
//!
//!   GET https://apibay.org/q.php?q=<url-encoded query>&cat=<category>
//!
//! Devuelve JSON como `[{ id, name, info_hash, seeders, leechers, ... }]`.
//! Si no hay resultados devuelve una lista con un único item de infohash
//! `"0000000000000000000000000000000000000000"` — hay que descartarlo.
//!
//! Cubre lo mismo que Knaben pero desde otra fuente: útil como
//! redundancia cuando Knaben rate-limitea o cae.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;

use super::{build_magnet, quality_from_title, MovieQuery, Torrent, TorrentProvider};

/// Categorías TPB de vídeo (200 = Video, 201 = Movies, 202 = Movies DVDR,
/// 205 = TV shows, 207 = HD movies, 208 = HD TV, 211 = 3D). Usamos el
/// paraguas 200 para no perder releases mal categorizados.
const CATEGORY_VIDEO: &str = "200";
const BASE: &str = "https://apibay.org/q.php";
/// Infohash que apibay devuelve cuando no hay hits.
const EMPTY_HASH: &str = "0000000000000000000000000000000000000000";

pub struct Apibay;

#[derive(Debug, Deserialize)]
struct ApibayHit {
    #[serde(default)]
    name: String,
    #[serde(default)]
    info_hash: String,
    #[serde(default)]
    seeders: String,
    #[serde(default)]
    leechers: String,
    #[serde(default)]
    size: String,
}

#[async_trait]
impl TorrentProvider for Apibay {
    fn name(&self) -> &'static str {
        "apibay"
    }

    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>> {
        // Política unificada con Knaben: NUNCA metemos el año en la
        // query (los grupos etiquetan el año de estreno del país
        // original y no el USA que TMDB reporta). Filtramos por año
        // ±1 después.
        let query = q.title.trim().to_string();

        let url = format!(
            "{BASE}?q={}&cat={CATEGORY_VIDEO}",
            urlencoding::encode(&query)
        );

        let resp = http
            .get(&url)
            .send()
            .await
            .context("Error de red hacia apibay")?;

        if !resp.status().is_success() {
            anyhow::bail!("apibay devolvió {}", resp.status());
        }

        let hits: Vec<ApibayHit> = resp
            .json()
            .await
            .context("Error al parsear respuesta de apibay")?;

        Ok(hits
            .into_iter()
            .filter(|h| h.info_hash != EMPTY_HASH && !h.info_hash.is_empty())
            .filter_map(|h| {
                let seeders = h.seeders.parse::<u32>().ok()?;
                let leechers = h.leechers.parse::<u32>().unwrap_or(0);
                let size_bytes = h.size.parse::<u64>().unwrap_or(0);
                let quality = quality_from_title(&h.name);
                let magnet = build_magnet(&h.info_hash, &h.name);
                Some(Torrent {
                    title: h.name,
                    magnet,
                    size_bytes,
                    seeders,
                    leechers,
                    quality,
                    source: "apibay".to_string(),
                    match_kind: crate::torrents::MatchKind::default(),
                    infohash: h.info_hash.to_ascii_uppercase(),
                })
            })
            .collect())
    }
}
