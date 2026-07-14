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

    let seeds: Vec<u64> = entries
        .iter()
        .filter(|e| e.rating.map(|r| r >= min_rating).unwrap_or(false))
        .filter_map(|e| tmdb_id_from_film(&e.film))
        .collect();

    (seen, seeds)
}

/// Pre-selecciona las `fetch_count` candidatas con mejor freq × TMDB (0-10 → 0-5)
/// antes de gastar llamadas a Letterboxd para el score final.
fn pre_score_candidates(
    candidate_movies: &HashMap<u64, TmdbMovie>,
    freq: &HashMap<u64, u32>,
    fetch_count: usize,
) -> Vec<u64> {
    let mut v: Vec<(u64, f32)> = candidate_movies
        .keys()
        .map(|id| {
            let f = *freq.get(id).unwrap_or(&1) as f32;
            let t = candidate_movies[id].vote_average / 2.0;
            (*id, f * t)
        })
        .collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(fetch_count);
    v.into_iter().map(|(id, _)| id).collect()
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
    // ── 1. Historial + watchlist ────────────────────────────────────────────
    progress.stage("Cargando historial…", 0);
    let (entries, watchlist) =
        tokio::try_join!(lb_client.get_log_entries(), lb_client.get_watchlist())?;
    progress.finish();

    let (seen, seeds) = compute_seen_and_seeds(&entries, &watchlist, min_rating);

    // ── 2. Recomendaciones TMDB (en paralelo, con caché) ────────────────────
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
            freq.entry(movie.id)
                .and_modify(|c| *c += 1)
                .or_insert_with(|| {
                    candidate_movies.insert(movie.id, movie.clone());
                    1
                });
        }
    }
    progress.finish();
    tmdb_client.save_cache();

    // Pre-selección por freq × TMDB
    let fetch_count = (count * 3).min(candidate_movies.len());
    let pre_scored = pre_score_candidates(&candidate_movies, &freq, fetch_count);

    let candidates: Vec<(TmdbMovie, f32)> = pre_scored
        .iter()
        .map(|id| {
            let movie = candidate_movies.remove(id).unwrap();
            let f = *freq.get(id).unwrap_or(&1) as f32;
            (movie, f)
        })
        .collect();

    // ── 3. Ratings de Letterboxd (en paralelo) → re-score final ─────────────
    progress.stage("Obteniendo ratings de Letterboxd…", fetch_count as u64);

    let mut lb_stream = stream::iter(candidates)
        .map(|(movie, f)| async move {
            let lb_rating = lb_client.get_lb_rating(movie.id).await;
            progress.inc();
            let score = final_score(lb_rating, f, movie.vote_average);
            Recommendation {
                movie,
                lb_rating,
                score,
            }
        })
        .buffer_unordered(LB_CONCURRENCY);

    let mut scored: Vec<Recommendation> = Vec::with_capacity(fetch_count);
    while let Some(rec) = lb_stream.next().await {
        scored.push(rec);
    }
    progress.finish();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(count);

    Ok(scored)
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

        let top = pre_score_candidates(&candidates, &freq, 1);
        assert_eq!(top, vec![2]);
    }

    #[test]
    fn final_score_prefers_letterboxd_rating_over_tmdb() {
        assert_eq!(final_score(Some(4.0), 2.0, 10.0), 8.0);
        assert_eq!(final_score(None, 2.0, 10.0), 10.0); // 10.0 / 2.0 * 2.0
    }
}
