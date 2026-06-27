use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};

use crate::letterboxd::{LetterboxdClient, LogEntry};
use crate::tmdb::{TmdbClient, TmdbMovie};

pub struct Recommendation {
    pub movie: TmdbMovie,
    pub lb_rating: Option<f32>,
    pub score: f32,
}

fn tmdb_id_from_entry(entry: &LogEntry) -> Option<u64> {
    let links = entry.film.links.as_ref()?;
    links
        .iter()
        .find(|l| l.link_type == "tmdb")
        .and_then(|l| l.id.parse::<u64>().ok())
}

fn spinner(msg: &'static str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan}  {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(msg);
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

fn progress_bar(total: u64, msg: &'static str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan}  {msg}  {bar:28.cyan/white.dim}  {pos}/{len}",
        )
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(msg);
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

pub async fn build_recommendations(
    lb_client: &LetterboxdClient<'_>,
    tmdb_client: &TmdbClient<'_>,
    count: usize,
    min_rating: f32,
) -> Result<Vec<Recommendation>> {
    // ── 1. Historial ──────────────────────────────────────────────────────
    let sp = spinner("Cargando historial…");
    let entries: Vec<LogEntry> = lb_client.get_log_entries().await?;
    sp.finish_and_clear();

    let seen: std::collections::HashSet<u64> =
        entries.iter().filter_map(tmdb_id_from_entry).collect();

    let seeds: Vec<u64> = entries
        .iter()
        .filter(|e| e.rating.map(|r| r >= min_rating).unwrap_or(false))
        .filter_map(tmdb_id_from_entry)
        .collect();

    // ── 2. Recomendaciones TMDB ───────────────────────────────────────────
    let mut candidate_movies: HashMap<u64, TmdbMovie> = HashMap::new();
    let mut freq: HashMap<u64, u32> = HashMap::new();

    let pb = progress_bar(seeds.len() as u64, "Consultando TMDB…");
    for seed_id in &seeds {
        let recs = tmdb_client.get_recommendations(*seed_id).await?;
        for movie in recs {
            if seen.contains(&movie.id) {
                continue;
            }
            freq.entry(movie.id).and_modify(|c| *c += 1).or_insert_with(|| {
                candidate_movies.insert(movie.id, movie.clone());
                1
            });
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    // Pre-selección por freq × TMDB (TMDB en escala 0-10 → normalizo a 0-5)
    let fetch_count = (count * 3).min(candidate_movies.len());
    let pre_scored: Vec<u64> = {
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
    };

    // ── 3. Ratings de Letterboxd → re-score final ─────────────────────────
    let pb2 = progress_bar(fetch_count as u64, "Obteniendo ratings de Letterboxd…");
    let mut scored: Vec<Recommendation> = Vec::with_capacity(fetch_count);

    for id in &pre_scored {
        let movie = candidate_movies.remove(id).unwrap();
        let f = *freq.get(id).unwrap_or(&1) as f32;
        let lb_rating = lb_client.get_lb_rating(*id).await;
        let score = lb_rating
            .map(|r| f * r)
            .unwrap_or_else(|| f * movie.vote_average / 2.0);
        scored.push(Recommendation { movie, lb_rating, score });
        pb2.inc(1);
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    pb2.finish_and_clear();

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(count);

    Ok(scored)
}
