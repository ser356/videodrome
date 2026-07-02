use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.themoviedb.org/3";
const RECS_CACHE_FILE: &str = "tmdb_recs_cache.json";
const RECS_CACHE_TTL_SECS: u64 = 24 * 3600;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TmdbMovie {
    pub id: u64,
    pub title: String,
    pub vote_average: f32,
    #[allow(dead_code)]
    pub popularity: f32,
}

#[derive(Debug, Deserialize)]
struct RecommendationsResponse {
    results: Vec<TmdbMovie>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CachedRecs {
    timestamp: u64,
    movies: Vec<TmdbMovie>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("El tiempo no puede ir hacia atrás")
        .as_secs()
}

fn cache_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("letterboxd-cli");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(RECS_CACHE_FILE))
}

fn load_cache() -> HashMap<u64, CachedRecs> {
    (|| -> Option<HashMap<u64, CachedRecs>> {
        let path = cache_path().ok()?;
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    })()
    .unwrap_or_default()
}

fn save_cache(cache: &HashMap<u64, CachedRecs>) {
    if let Ok(path) = cache_path() {
        if let Ok(json) = serde_json::to_string(cache) {
            let _ = std::fs::write(path, json);
        }
    }
}

pub struct TmdbClient<'a> {
    http: &'a reqwest::Client,
    bearer_token: &'a str,
    cache: Mutex<HashMap<u64, CachedRecs>>,
}

impl<'a> TmdbClient<'a> {
    pub fn new(http: &'a reqwest::Client, bearer_token: &'a str) -> Self {
        Self {
            http,
            bearer_token,
            cache: Mutex::new(load_cache()),
        }
    }

    /// Recomendaciones de TMDB para una película, cacheadas en disco (TTL 24h)
    /// para no repetir la misma consulta en ejecuciones sucesivas.
    pub async fn get_recommendations(&self, tmdb_id: u64) -> Result<Vec<TmdbMovie>> {
        if let Some(cached) = self.cache.lock().unwrap().get(&tmdb_id) {
            if now_unix() - cached.timestamp < RECS_CACHE_TTL_SECS {
                return Ok(cached.movies.clone());
            }
        }

        let url = format!("{BASE_URL}/movie/{tmdb_id}/recommendations?language=es-ES&page=1");

        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| format!("Error al obtener recomendaciones para tmdb_id={tmdb_id}"))?;

        if !resp.status().is_success() {
            // Película no encontrada u otro error: devolver lista vacía silenciosamente
            return Ok(vec![]);
        }

        let body: RecommendationsResponse = resp
            .json()
            .await
            .context("Error al parsear respuesta de TMDB")?;

        self.cache.lock().unwrap().insert(
            tmdb_id,
            CachedRecs {
                timestamp: now_unix(),
                movies: body.results.clone(),
            },
        );

        Ok(body.results)
    }

    /// Persiste en disco la caché de recomendaciones acumulada en esta sesión.
    pub fn save_cache(&self) {
        save_cache(&self.cache.lock().unwrap());
    }
}
