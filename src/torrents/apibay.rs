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
        //
        // Series: mismas variantes que Knaben (SxxEyy / Sxx / "Season N")
        // en paralelo, dedup por infohash. La categoría paraguas 200
        // ya cubre TV shows (205/208), no hay que cambiarla.
        let queries: Vec<String> = if matches!(q.kind, crate::tmdb::MediaKind::Series) {
            apibay_series_variants(&q.title, q.season, q.episode)
        } else {
            vec![q.title.trim().to_string()]
        };

        let futs = queries
            .into_iter()
            .map(|query| async move { apibay_query(http, &query).await });
        let results = futures::future::join_all(futs).await;

        let mut seen = std::collections::HashSet::<String>::new();
        let mut out: Vec<Torrent> = Vec::new();
        for hit in results.into_iter().flatten().flatten() {
            if hit.info_hash == EMPTY_HASH || hit.info_hash.is_empty() {
                continue;
            }
            let hash_up = hit.info_hash.to_ascii_uppercase();
            if !seen.insert(hash_up.clone()) {
                continue;
            }
            let Some(seeders) = hit.seeders.parse::<u32>().ok() else {
                continue;
            };
            let leechers = hit.leechers.parse::<u32>().unwrap_or(0);
            let size_bytes = hit.size.parse::<u64>().unwrap_or(0);
            let quality = quality_from_title(&hit.name);
            let magnet = build_magnet(&hit.info_hash, &hit.name);
            out.push(Torrent {
                title: hit.name,
                magnet,
                size_bytes,
                seeders,
                leechers,
                quality,
                source: "apibay".to_string(),
                match_kind: crate::torrents::MatchKind::default(),
                file_hint: None,
                infohash: hash_up,
            });
        }
        Ok(out)
    }
}

/// Variantes de query para series en apibay — mismo criterio que
/// knaben::series_query_variants, se mantienen separadas por si algún
/// backend prefiere distinta forma en el futuro.
fn apibay_series_variants(title: &str, season: Option<u16>, episode: Option<u16>) -> Vec<String> {
    let t = title.trim();
    match (season, episode) {
        (Some(s), Some(e)) => vec![
            format!("{} S{:02}E{:02}", t, s, e),
            format!("{} S{:02}", t, s),
        ],
        (Some(s), None) => vec![format!("{} S{:02}", t, s), format!("{} Season {}", t, s)],
        (None, _) => vec![t.to_string()],
    }
}

async fn apibay_query(http: &reqwest::Client, query: &str) -> Result<Vec<ApibayHit>> {
    let url = format!(
        "{BASE}?q={}&cat={CATEGORY_VIDEO}",
        urlencoding::encode(query)
    );
    let resp = http
        .get(&url)
        .send()
        .await
        .context("Error de red hacia apibay")?;
    if !resp.status().is_success() {
        anyhow::bail!("apibay devolvió {}", resp.status());
    }
    resp.json::<Vec<ApibayHit>>()
        .await
        .context("Error al parsear respuesta de apibay")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_variants_for_episode() {
        let v = apibay_series_variants("Fargo", Some(2), Some(3));
        assert_eq!(v, vec!["Fargo S02E03", "Fargo S02"]);
    }

    #[test]
    fn series_variants_for_season_pack() {
        let v = apibay_series_variants("Fargo", Some(2), None);
        assert_eq!(v, vec!["Fargo S02", "Fargo Season 2"]);
    }

    #[test]
    fn series_variants_for_whole_series() {
        let v = apibay_series_variants("Fargo", None, None);
        assert_eq!(v, vec!["Fargo"]);
    }

    #[test]
    fn series_variants_trims_title() {
        let v = apibay_series_variants("  Fargo  ", None, None);
        assert_eq!(v, vec!["Fargo"]);
    }

    #[test]
    fn series_variants_pads_season_and_episode_with_zero() {
        let v = apibay_series_variants("Show", Some(1), Some(9));
        assert_eq!(v[0], "Show S01E09");
    }

    #[test]
    fn series_variants_never_empty() {
        // Contrato: incluso sin season se emite el título tal cual.
        for (s, e) in [(None, None), (Some(1), Some(1)), (Some(1), None)] {
            let v = apibay_series_variants("X", s, e);
            assert!(!v.is_empty());
        }
    }
}
