//! Provider Knaben (api.knaben.org). Agregador que consulta decenas de
//! indexers (1337x, TPB, YTS, TorrentGalaxy, Nyaa, RuTracker…) y devuelve
//! JSON limpio. Sin auth.
//! Docs: <https://knaben.org/api>

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{infohash_from_magnet, quality_from_title, MovieQuery, Torrent, TorrentProvider};

const BASE: &str = "https://api.knaben.org/v1";

pub struct Knaben;

#[derive(Debug, Serialize)]
struct KnabenRequest<'a> {
    /// Modo de búsqueda: `"100%"` = exact match, `"score"` = fuzzy. Empezamos
    /// con score porque tolera pequeñas variantes de título (dos puntos,
    /// artículos, etc.).
    search_type: &'a str,
    search_field: &'a str,
    query: String,
    order_by: &'a str,
    order_direction: &'a str,
    size: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    categories: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct KnabenResponse {
    #[serde(default)]
    hits: Vec<KnabenHit>,
}

#[derive(Debug, Deserialize)]
struct KnabenHit {
    #[serde(default)]
    title: String,
    #[serde(default)]
    seeders: Option<u32>,
    #[serde(default)]
    peers: Option<u32>,
    #[serde(default)]
    bytes: Option<u64>,
    #[serde(rename = "magnetUrl", default)]
    magnet_url: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    tracker: Option<String>,
}

#[async_trait]
impl TorrentProvider for Knaben {
    fn name(&self) -> &'static str {
        "knaben"
    }

    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>> {
        // Nunca metemos el año en el query de Knaben: TMDB puede devolver
        // el año de estreno USA (2008 para Funny Games US) pero las
        // releases scene lo etiquetan con el año de Cannes (2007). Buscar
        // "Funny Games 2008" devuelve solo 2 releases raros con 0 seeders
        // porque los grupos usan el otro año. Buscamos solo el título y
        // filtramos por año ±1 después.
        //
        // Para SERIES generamos hasta 3 queries paralelas con las
        // variantes que los grupos scene usan realmente:
        //   * "Title SxxEyy" — episodio exacto
        //   * "Title Sxx"    — season packs (o releases del pack completo)
        //   * "Title Season N" — packs de HDTV/WEB que etiquetan largo
        // Merge por infohash. NUNCA solo el título — el filtro central
        // tira el 95% pero el recall de las queries específicas es muy
        // superior a un título a secas.
        //
        // Para PELÍCULAS: 100% (exact) + score (fuzzy) en paralelo
        // sobre el título, como hasta ahora.
        let queries: Vec<(String, &'static str)> =
            if matches!(q.kind, crate::tmdb::MediaKind::Series) {
                series_query_variants(&q.title, q.season, q.episode)
                    .into_iter()
                    .map(|s| (s, "100%"))
                    .collect()
            } else {
                vec![(q.title.clone(), "100%"), (q.title.clone(), "score")]
            };

        let futs = queries
            .into_iter()
            .map(|(query, st)| async move { knaben_query(http, &query, st).await });
        let results = futures::future::join_all(futs).await;

        let mut merged: Vec<KnabenHit> = Vec::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for hit in results.into_iter().flatten().flatten() {
            // Dedup por hash (o por título si el hash no vino).
            let key = hit
                .hash
                .clone()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| hit.title.clone());
            if seen.insert(key) {
                merged.push(hit);
            }
        }

        // Fase 2b: los filtros de título (overlap), año y TV VIVEN
        // AHORA en `search_all` (ver `mod.rs`), aplicados sobre
        // `ParsedRelease`. El provider devuelve TODOS los hits
        // crudos que Knaben nos dio — así todos los providers pasan
        // por el mismo embudo y no diverge la política. El overlap
        // por token queda desactivado a propósito: era la fuente
        // principal de los homónimos que se colaban.
        Ok(hits_to_torrents(merged))
    }
}

/// Genera variantes de query textual para una serie según qué
/// season/episode traiga la MovieQuery. Los grupos scene etiquetan
/// las mismas releases con formas distintas (SxxEyy, Sxx, "Season N"),
/// así que un query único deja fuera el resto.
fn series_query_variants(title: &str, season: Option<u16>, episode: Option<u16>) -> Vec<String> {
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

/// Palabras cortas o vacías que no aportan discriminación. Idiomas: EN + ES.
///
/// **Legacy** (Fase 2b): el filtro por overlap de tokens vive ahora
/// en `search_all` con matching por `title_variants`. Se conservan
/// `STOPWORDS`, `tokenize` y `filter_by_token_overlap` únicamente
/// como referencia + tests — no se llaman desde el pipeline vivo.
#[allow(dead_code)]
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "una", "uno", "unos", "unas", "los", "las", "que", "por",
    "para", "con", "del",
];

/// Tokeniza un título: pasa a minúsculas, parte por cualquier carácter no
/// alfanumérico (releases scene usan `.` `-` `_` como separadores), y se
/// queda con tokens de ≥3 caracteres que no sean stopwords.
#[allow(dead_code)]
fn tokenize(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .filter(|w| !STOPWORDS.contains(w))
        .map(|s| s.to_string())
        .collect()
}

/// Filtra hits fuzzy quedándose solo con los cuyo título contiene todas (o
/// la gran mayoría) de las palabras significativas del título buscado.
///
/// Reglas:
/// * Si tras filtrar tokens `<3` chars y stopwords no queda ninguna
///   `needle`, se aceptan todos los hits sin filtrar (títulos como "It",
///   "Up", "Us" caerían a lista vacía de otra forma). El resto de
///   filtros posteriores (año, seeders) ya bajan la basura.
/// * Título con ≤3 tokens significativos → se exige match COMPLETO. Evita
///   falsos positivos en títulos cortos como "Play Dead".
/// * Título con más tokens → basta con matchear ≥2/3 de ellos. Los títulos
///   largos suelen aparecer abreviados en releases.
#[allow(dead_code)]
fn filter_by_token_overlap(hits: Vec<KnabenHit>, title: &str) -> Vec<KnabenHit> {
    let needles = tokenize(title);
    if needles.is_empty() {
        return hits;
    }
    let need_all = needles.len() <= 3;
    let threshold = if need_all {
        needles.len()
    } else {
        needles.len() * 2 / 3
    };

    hits.into_iter()
        .filter(|h| {
            let hit_tokens = tokenize(&h.title);
            let overlap = needles.intersection(&hit_tokens).count();
            overlap >= threshold
        })
        .collect()
}

async fn knaben_query(
    http: &reqwest::Client,
    query: &str,
    search_type: &str,
) -> Result<Vec<KnabenHit>> {
    let body = KnabenRequest {
        search_type,
        search_field: "title",
        query: query.to_string(),
        order_by: "seeders",
        order_direction: "desc",
        // Subido a 100 en Fase 2 del audit: con exact + fuzzy y los
        // filtros centralizados de `search_all` filtrando fuerte
        // después, 40 dejaba fuera hits legítimos de pelis con
        // muchos releases (blockbusters). La API acepta hasta 100
        // sin coste apreciable.
        size: 100,
        categories: vec![3_000_000],
    };

    let resp = http
        .post(BASE)
        .json(&body)
        .send()
        .await
        .context("Error de red hacia Knaben")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Knaben devolvió {status}: {body}");
    }

    let parsed: KnabenResponse = resp
        .json()
        .await
        .context("Error al parsear respuesta de Knaben")?;
    Ok(parsed.hits)
}

fn hits_to_torrents(hits: Vec<KnabenHit>) -> Vec<Torrent> {
    let mut out = Vec::new();
    for h in hits {
        let magnet = match h.magnet_url {
            Some(m) if !m.is_empty() => m,
            _ => continue,
        };
        let infohash = h
            .hash
            .map(|s| s.to_ascii_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| infohash_from_magnet(&magnet));
        if infohash.is_empty() {
            continue;
        }

        let seeders = h.seeders.unwrap_or(0);
        let leechers = h.peers.unwrap_or(0);
        let size_bytes = h.bytes.unwrap_or(0);
        let quality = quality_from_title(&h.title);
        // El nombre del tracker (1337x, TorrentGalaxy, YTS...) lo dejamos
        // implícito porque `source` es `&'static str`; se puede exponer más
        // adelante como `provider_detail` si hace falta.
        let _ = h.tracker;

        out.push(Torrent {
            title: h.title,
            magnet,
            size_bytes,
            seeders,
            leechers,
            quality,
            source: "knaben".to_string(),
            match_kind: crate::torrents::MatchKind::default(),
            infohash,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(title: &str) -> KnabenHit {
        KnabenHit {
            title: title.to_string(),
            seeders: Some(10),
            peers: Some(0),
            bytes: Some(1024 * 1024 * 1024),
            magnet_url: Some(format!(
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn={}",
                urlencoding::encode(title)
            )),
            hash: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            tracker: None,
        }
    }

    #[test]
    fn overlap_short_title_requires_full_match() {
        // "Play Dead" tiene 2 tokens significativos → need_all.
        let hits = vec![
            hit("Play.Dead.2022.1080p.BluRay.x264"),
            hit("Deadly.Visitor.2018.1080p"), // solo "dead" (substring, no token)
            hit("Play.Time.2020.1080p"),      // solo "play"
        ];
        let filtered = filter_by_token_overlap(hits, "Play Dead");
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].title.starts_with("Play.Dead"));
    }

    #[test]
    fn overlap_short_title_survives_when_all_tokens_stopwords() {
        // "It" y "Up" no dejan needles significativos → aceptamos todo
        // (el filtro por año/seeders/dedup hace el resto).
        let hits = vec![hit("Anything.2020.1080p"), hit("Whatever.2019.720p")];
        let filtered = filter_by_token_overlap(hits, "It");
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn overlap_long_title_tolerates_partial_match() {
        // "The Lord of the Rings Fellowship of the Ring" → tras
        // stopwords/short → {"lord", "rings", "fellowship", "ring"}
        // → 4 tokens → threshold = 4*2/3 = 2.
        let hits = vec![
            hit("The.Lord.of.the.Rings.Fellowship.1080p"), // {lord, rings, fellowship} → 3/4 ✓
            hit("Fellowship.Ring.2001.1080p"),             // {fellowship, ring} → 2/4 ✓
            hit("Lord.of.War.2005.1080p"),                 // {lord} → 1/4 ✗
        ];
        let filtered =
            filter_by_token_overlap(hits, "The Lord of the Rings Fellowship of the Ring");
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn overlap_uses_word_boundaries_not_substrings() {
        // "Dead" (needle) NO debe matchear "deadly" (token del hit)
        // porque tokenize parte por caracteres no alfanuméricos, no
        // hace substring search. Este comportamiento es lo que salva
        // "Play Dead" de matchear "Deadly Visitor" en el primer test.
        let a: std::collections::HashSet<String> = tokenize("Play Dead");
        let b: std::collections::HashSet<String> = tokenize("Deadly.Visitor");
        assert_eq!(a.intersection(&b).count(), 0);
    }

    #[test]
    fn series_variants_episode_produces_sxxexx_and_sxx() {
        let v = series_query_variants("Fargo", Some(2), Some(3));
        assert_eq!(v, vec!["Fargo S02E03".to_string(), "Fargo S02".to_string()]);
    }

    #[test]
    fn series_variants_season_pack_produces_sxx_and_season_n() {
        let v = series_query_variants("Fargo", Some(2), None);
        assert_eq!(
            v,
            vec!["Fargo S02".to_string(), "Fargo Season 2".to_string()]
        );
    }

    #[test]
    fn series_variants_no_season_falls_back_to_title() {
        let v = series_query_variants("Fargo", None, None);
        assert_eq!(v, vec!["Fargo".to_string()]);
    }
}
