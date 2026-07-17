use std::collections::{HashMap, HashSet};

use anyhow::Result;
use futures::stream::{self, StreamExt};
use serde::Serialize;

use crate::letterboxd::{Film, LetterboxdClient, LogEntry};
use crate::progress::Progress;
use crate::tmdb::{TmdbClient, TmdbMovie};

const TMDB_CONCURRENCY: usize = 8;
const LB_CONCURRENCY: usize = 6;

#[derive(Serialize, Clone)]
pub struct Recommendation {
    pub movie: TmdbMovie,
    pub lb_rating: Option<f32>,
    pub score: f32,
}

fn tmdb_id_from_film(film: &Film) -> Option<u64> {
    let links = film.links.as_ref()?;
    links
        .iter()
        .find(|l| l.link_type == "tmdb")
        .and_then(|l| l.id.parse::<u64>().ok())
}

/// Películas a excluir de las recomendaciones (ya vistas o en watchlist) y
/// películas semilla (vistas con rating >= min_rating).
///
/// Las semillas se dedupean por TMDB id: en Letterboxd cada visionado es
/// un log entry distinto, así que sin dedupe un rewatch triplicaba las
/// llamadas a TMDB y sesgaba `freq` (una peli vecina de un rewatch
/// contaría 3 veces).
fn compute_seen_and_seeds(
    entries: &[LogEntry],
    watchlist: &[Film],
    min_rating: f32,
) -> (HashSet<u64>, Vec<u64>) {
    let mut seen: HashSet<u64> = entries
        .iter()
        .filter_map(|e| tmdb_id_from_film(&e.film))
        .collect();
    seen.extend(watchlist.iter().filter_map(tmdb_id_from_film));

    let mut seed_set: HashSet<u64> = HashSet::new();
    let mut seeds: Vec<u64> = Vec::new();
    for e in entries {
        if !e.rating.map(|r| r >= min_rating).unwrap_or(false) {
            continue;
        }
        let Some(id) = tmdb_id_from_film(&e.film) else {
            continue;
        };
        if seed_set.insert(id) {
            seeds.push(id);
        }
    }

    (seen, seeds)
}

/// Pre-selecciona las `fetch_count` candidatas con mejor freq × TMDB (0-10 → 0-5)
/// antes de gastar llamadas a Letterboxd para el score final. Devuelve las
/// películas (drenadas del map) junto con su frecuencia — evita recorrer el
/// map otra vez después.
fn pre_score_candidates(
    candidate_movies: HashMap<u64, TmdbMovie>,
    freq: &HashMap<u64, u32>,
    fetch_count: usize,
) -> Vec<(TmdbMovie, f32)> {
    let mut v: Vec<(TmdbMovie, f32)> = candidate_movies
        .into_iter()
        .map(|(id, movie)| {
            let f = *freq.get(&id).unwrap_or(&1) as f32;
            (movie, f)
        })
        .collect();
    v.sort_unstable_by(|a, b| {
        let sa = a.1 * a.0.vote_average / 2.0;
        let sb = b.1 * b.0.vote_average / 2.0;
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    v.truncate(fetch_count);
    v
}

/// Score final: rating de Letterboxd si está disponible, si no el de TMDB
/// normalizado a escala 0.5–5.0, ponderado por la frecuencia de aparición.
fn final_score(lb_rating: Option<f32>, freq: f32, tmdb_vote_average: f32) -> f32 {
    lb_rating
        .map(|r| freq * r)
        .unwrap_or_else(|| freq * tmdb_vote_average / 2.0)
}

pub async fn build_recommendations<P: Progress>(
    lb_client: &LetterboxdClient<'_>,
    tmdb_client: &TmdbClient<'_>,
    count: usize,
    min_rating: f32,
    progress: &P,
) -> Result<Vec<Recommendation>> {
    // Fast path para callers que quieren todo de una vez (CLI/TUI):
    // build_candidate_pool → enrich_batch(count * 3) → sort → truncate.
    // La GUI usa las dos funciones por separado para servir por páginas.
    let candidates =
        build_candidate_pool(lb_client, tmdb_client, min_rating, count * 3, progress).await?;
    let fetch_count = (count * 3).min(candidates.len());
    let slice = &candidates[..fetch_count];
    progress.stage("Obteniendo ratings de Letterboxd…", fetch_count as u64);
    let mut scored = enrich_batch(lb_client, slice, Some(progress)).await;
    progress.finish();
    scored.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(count);
    Ok(scored)
}

/// Pipeline steps 1-3: historial Letterboxd + recomendaciones TMDB +
/// pre-score por freq × TMDB. Devuelve la lista de candidatos ya
/// ordenada, lista para que `enrich_batch` la consuma por trozos.
///
/// `cap` limita cuántos candidatos se retienen (los mejores por
/// pre-score); el resto se descartan sin ir a Letterboxd. Es el
/// techo real del scroll infinito — si el user llega al final es
/// porque ya no hay más candidatos plausibles.
pub async fn build_candidate_pool<P: Progress>(
    lb_client: &LetterboxdClient<'_>,
    tmdb_client: &TmdbClient<'_>,
    min_rating: f32,
    cap: usize,
    progress: &P,
) -> Result<Vec<(TmdbMovie, f32)>> {
    progress.stage("Cargando historial…", 0);
    let (entries, watchlist) =
        tokio::try_join!(lb_client.get_log_entries(), lb_client.get_watchlist())?;
    progress.finish();

    let (seen, seeds) = compute_seen_and_seeds(&entries, &watchlist, min_rating);

    let mut candidate_movies: HashMap<u64, TmdbMovie> = HashMap::new();
    let mut freq: HashMap<u64, u32> = HashMap::new();

    let seed_msg = format!(
        "Consultando TMDB…  ({} semillas con ★ ≥ {})",
        seeds.len(),
        min_rating
    );
    progress.stage(&seed_msg, seeds.len() as u64);

    let mut tmdb_stream = stream::iter(seeds.iter().copied())
        .map(|seed_id| async move {
            let result = tmdb_client.get_recommendations(seed_id).await;
            progress.inc();
            result
        })
        .buffer_unordered(TMDB_CONCURRENCY);

    while let Some(result) = tmdb_stream.next().await {
        for movie in result? {
            if seen.contains(&movie.id) {
                continue;
            }
            let count = freq.entry(movie.id).or_insert(0);
            *count += 1;
            candidate_movies.entry(movie.id).or_insert(movie);
        }
    }
    progress.finish();
    tmdb_client.save_cache();

    let fetch_count = cap.min(candidate_movies.len());
    Ok(pre_score_candidates(candidate_movies, &freq, fetch_count))
}

/// Pipeline step 4 aplicado a un batch: pide ratings LB para cada
/// candidato en paralelo, ordena por score final DESC. No trunca —
/// el caller decide cuántos servir.
///
/// `progress` es opcional para no ensuciar la UX de la GUI (donde
/// mostramos un spinner, no una progress bar). La CLI lo pasa
/// siempre para que la barra avance por batch.
pub async fn enrich_batch<P: Progress>(
    lb_client: &LetterboxdClient<'_>,
    candidates: &[(TmdbMovie, f32)],
    progress: Option<&P>,
) -> Vec<Recommendation> {
    let mut scored: Vec<Recommendation> = stream::iter(candidates.iter().cloned())
        .map(|(movie, f)| async move {
            let lb_rating = lb_client.get_lb_rating(movie.id).await;
            if let Some(p) = progress {
                p.inc();
            }
            let score = final_score(lb_rating, f, movie.vote_average);
            Recommendation {
                movie,
                lb_rating,
                score,
            }
        })
        .buffer_unordered(LB_CONCURRENCY)
        .collect()
        .await;
    scored.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::letterboxd::FilmLink;

    fn film(id: &str, tmdb_id: Option<&str>) -> Film {
        Film {
            id: id.to_string(),
            name: id.to_string(),
            links: tmdb_id.map(|t| {
                vec![FilmLink {
                    link_type: "tmdb".to_string(),
                    id: t.to_string(),
                }]
            }),
        }
    }

    fn entry(tmdb_id: &str, rating: Option<f32>) -> LogEntry {
        LogEntry {
            film: film(tmdb_id, Some(tmdb_id)),
            rating,
        }
    }

    fn movie(id: u64, vote_average: f32) -> TmdbMovie {
        TmdbMovie {
            id,
            title: format!("Movie {id}"),
            vote_average,
            popularity: 0.0,
            release_date: None,
            poster_path: None,
            imdb_id: None,
            kind: crate::tmdb::MediaKind::Movie,
        }
    }

    #[test]
    fn tmdb_id_from_film_returns_none_without_links() {
        assert_eq!(tmdb_id_from_film(&film("1", None)), None);
    }

    #[test]
    fn tmdb_id_from_film_parses_tmdb_link() {
        assert_eq!(tmdb_id_from_film(&film("1", Some("42"))), Some(42));
    }

    #[test]
    fn seeds_only_include_entries_at_or_above_min_rating() {
        let entries = vec![
            entry("1", Some(5.0)),
            entry("2", Some(3.0)),
            entry("3", None),
        ];
        let (seen, seeds) = compute_seen_and_seeds(&entries, &[], 4.0);
        assert_eq!(seeds, vec![1]);
        // Todas las vistas cuentan como "seen", tengan rating o no.
        assert_eq!(seen, HashSet::from([1, 2, 3]));
    }

    #[test]
    fn seen_also_includes_watchlist() {
        let entries = vec![entry("1", Some(5.0))];
        let watchlist = vec![film("w1", Some("99"))];
        let (seen, _) = compute_seen_and_seeds(&entries, &watchlist, 4.0);
        assert!(seen.contains(&99));
    }

    #[test]
    fn pre_score_orders_by_frequency_times_tmdb_rating() {
        let mut candidates = HashMap::new();
        candidates.insert(1, movie(1, 6.0)); // score 1 * 3.0
        candidates.insert(2, movie(2, 8.0)); // score 2 * 4.0
        let mut freq = HashMap::new();
        freq.insert(1, 1);
        freq.insert(2, 2);

        let top = pre_score_candidates(candidates, &freq, 1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].0.id, 2);
    }

    #[test]
    fn final_score_prefers_letterboxd_rating_over_tmdb() {
        assert_eq!(final_score(Some(4.0), 2.0, 10.0), 8.0);
        assert_eq!(final_score(None, 2.0, 10.0), 10.0); // 10.0 / 2.0 * 2.0
    }
}
