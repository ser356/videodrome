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
        // Lanzamos los dos search_type en paralelo (100% = exact match,
        // score = fuzzy). Antes eran secuenciales (score solo se
        // consultaba si 100% venía vacío), lo que perdía hits: releases
        // con puntuación rara que sí aparecerían con fuzzy pero que 100%
        // devolvía como 1-2 hits basura que impedían el fallback.
        // Merge + dedup por infohash resuelve ambos problemas y añade
        // ~10-20% de recall sin coste de latencia (misma latencia que
        // 100% solo, gracias al paralelismo).
        let (exact, fuzzy) = tokio::join!(
            knaben_query(http, &q.title, "100%"),
            knaben_query(http, &q.title, "score"),
        );

        let mut merged: Vec<KnabenHit> = Vec::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for hit in exact.into_iter().flatten().chain(fuzzy.into_iter().flatten()) {
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

        // Post-filtro por overlap de tokens (palabras completas, no
        // substrings — así "Deadly Visitor" no matchea a "Play Dead" por
        // contener "dead"). Se aplica siempre — imprescindible con
        // `score` porque devuelve fuzzy matches muy laxos.
        let mut filtered = filter_by_token_overlap(merged, &q.title);

        // Filtro adicional por año con tolerancia ±1: los releases suelen
        // llevar el año en el nombre, y para pelis internacionales el año
        // del scene puede ir 1 año antes o después del que TMDB reporta.
        if let Some(target) = q.year {
            filtered.retain(|h| release_matches_year(&h.title, target));
        }

        Ok(hits_to_torrents(filtered))
    }
}

/// Extrae años (1900-2099) del título del release y comprueba si alguno
/// está dentro de ±1 del año buscado. Si el release no incluye ningún año,
/// se acepta (no podemos discriminar y es preferible un falso positivo a
/// perder el hit).
fn release_matches_year(title: &str, target: u16) -> bool {
    let mut has_year = false;
    for token in title.split(|c: char| !c.is_alphanumeric()) {
        if token.len() != 4 {
            continue;
        }
        if let Ok(y) = token.parse::<u16>() {
            if (1900..=2099).contains(&y) {
                has_year = true;
                if (target as i32 - y as i32).abs() <= 1 {
                    return true;
                }
            }
        }
    }
    !has_year
}

/// Palabras cortas o vacías que no aportan discriminación. Idiomas: EN + ES.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "una", "uno", "unos", "unas", "los", "las", "que", "por",
    "para", "con", "del",
];

/// Tokeniza un título: pasa a minúsculas, parte por cualquier carácter no
/// alfanumérico (releases scene usan `.` `-` `_` como separadores), y se
/// queda con tokens de ≥3 caracteres que no sean stopwords.
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
/// * Título con ≤3 tokens significativos → se exige match COMPLETO. Evita
///   falsos positivos en títulos cortos como "Play Dead".
/// * Título con más tokens → basta con matchear ≥2/3 de ellos. Los títulos
///   largos suelen aparecer abreviados en releases.
fn filter_by_token_overlap(hits: Vec<KnabenHit>, title: &str) -> Vec<KnabenHit> {
    let needles = tokenize(title);
    if needles.is_empty() {
        return Vec::new();
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
        size: 40,
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
            source: "knaben",
            infohash,
        });
    }
    out
}
