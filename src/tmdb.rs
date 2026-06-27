use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

const BASE_URL: &str = "https://api.themoviedb.org/3";

#[derive(Debug, Deserialize, Clone)]
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

pub struct TmdbClient<'a> {
    http: &'a reqwest::Client,
    bearer_token: &'a str,
}

impl<'a> TmdbClient<'a> {
    pub fn new(http: &'a reqwest::Client, bearer_token: &'a str) -> Self {
        Self { http, bearer_token }
    }

    pub async fn get_recommendations(&self, tmdb_id: u64) -> Result<Vec<TmdbMovie>> {
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

        tokio::time::sleep(Duration::from_millis(100)).await;

        Ok(body.results)
    }
}
