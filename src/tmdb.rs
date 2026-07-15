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
    #[serde(default)]
    pub release_date: Option<String>, // "YYYY-MM-DD"
    /// Ruta relativa del poster en TMDB (ej. `/abc123.jpg`). Se combina
    /// con `https://image.tmdb.org/t/p/w500` en el frontend para mostrar
    /// la miniatura en la GUI.
    #[serde(default)]
    pub poster_path: Option<String>,
}

impl TmdbMovie {
    /// Año extraído de `release_date`, si está presente y es parseable.
    pub fn year(&self) -> Option<u16> {
        self.release_date
            .as_deref()
            .and_then(|s| s.get(..4))
            .and_then(|s| s.parse().ok())
    }
}

#[derive(Debug, Deserialize)]
struct RecommendationsResponse {
    results: Vec<TmdbMovie>,
}

// ── Búsqueda por IMDb ID ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct FindResponse {
    #[serde(default)]
    movie_results: Vec<FindMovie>,
}

#[derive(Debug, Deserialize)]
struct FindMovie {
    #[allow(dead_code)]
    id: u64,
    title: String,
    #[serde(default)]
    release_date: String, // "YYYY-MM-DD"
}

/// Título y año resueltos desde un IMDb ID.
#[derive(Debug, Clone)]
pub struct ImdbLookup {
    pub title: String,
    pub year: Option<u16>,
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
        .join("videodrome");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(RECS_CACHE_FILE))
}

fn load_cache() -> HashMap<u64, CachedRecs> {
    let Ok(path) = cache_path() else {
        return HashMap::new();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
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
            if now_unix().saturating_sub(cached.timestamp) < RECS_CACHE_TTL_SECS {
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

        let status = resp.status();
        if !status.is_success() {
            // 404 (película no encontrada) es benigno — la ignoramos como
            // fuente. 401 / 429 / 5xx en cambio son señales de que la
            // config está rota o el rate-limit ha saltado: hay que
            // propagar para que el user lo vea, no devolver [] silencioso
            // que se lee como "no hay recomendaciones".
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(vec![]);
            }
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "TMDB devolvi\u{f3} {status} para tmdb_id={tmdb_id}: {}",
                body.chars().take(200).collect::<String>()
            );
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

    /// Resuelve un IMDb ID a título + año usando el endpoint `/find`.
    /// Devuelve `None` si TMDB no conoce ese ID.
    pub async fn find_by_imdb(&self, imdb_id: &str) -> Result<Option<ImdbLookup>> {
        let clean = imdb_id.trim();
        let url = format!("{BASE_URL}/find/{clean}?external_source=imdb_id&language=en-US");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .context("Error al llamar a TMDB /find")?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let body: FindResponse = resp
            .json()
            .await
            .context("Error al parsear respuesta de TMDB /find")?;

        Ok(body.movie_results.into_iter().next().map(|m| ImdbLookup {
            year: m.release_date.get(..4).and_then(|s| s.parse::<u16>().ok()),
            title: m.title,
        }))
    }

    /// Busca películas por texto libre en TMDB (`/search/movie`). Usado en la
    /// GUI para mostrar una pantalla intermedia con posters cuando el user
    /// teclea una query imprecisa, antes de disparar la búsqueda de
    /// torrents. Ordenado por popularidad tal cual lo devuelve TMDB.
    #[cfg(feature = "gui")]
    pub async fn search_movies(&self, query: &str) -> Result<Vec<TmdbMovie>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let url = format!(
            "{BASE_URL}/search/movie?query={}&language=es-ES&include_adult=false&page=1",
            urlencoding::encode(q)
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| format!("Error al llamar a TMDB /search/movie para '{q}'"))?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let body: RecommendationsResponse = resp
            .json()
            .await
            .context("Error al parsear respuesta de TMDB /search/movie")?;
        Ok(body.results)
    }

    /// Consulta `/movie/{tmdb_id}?append_to_response=external_ids,translations`
    /// para obtener en una sola llamada:
    /// * `imdb_id` — imprescindible para providers Torznab que lo aceptan.
    /// * `original_title` — para buscar torrents en el idioma original (el
    ///   que suelen usar las releases scene/P2P internacionales).
    /// * `russian_title` — usado como fallback: si Knaben no da hits con el
    ///   título original, muchos torrents rusos (RuTracker, rutor...) están
    ///   indexados con el título en cirílico.
    /// * `original_language` — código ISO 639-1 (`"en"`, `"es"`, `"ru"`...).
    ///   Se usa para heurística de detección de audio original vs doblaje.
    /// * `release_date` — para extraer el año.
    pub async fn get_movie_details(&self, tmdb_id: u64) -> Result<Option<MovieDetails>> {
        #[derive(Deserialize)]
        struct DetailsResponse {
            #[serde(default)]
            imdb_id: Option<String>,
            #[serde(default)]
            original_title: Option<String>,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            release_date: Option<String>,
            #[serde(default)]
            original_language: Option<String>,
            #[serde(default)]
            runtime: Option<u32>,
            #[serde(default)]
            external_ids: Option<ExternalIdsNested>,
            #[serde(default)]
            translations: Option<TranslationsNested>,
        }
        #[derive(Deserialize)]
        struct ExternalIdsNested {
            #[serde(default)]
            imdb_id: Option<String>,
        }
        #[derive(Deserialize)]
        struct TranslationsNested {
            #[serde(default)]
            translations: Vec<Translation>,
        }
        #[derive(Deserialize)]
        struct Translation {
            #[serde(default)]
            iso_639_1: String,
            #[serde(default)]
            data: TranslationData,
        }
        #[derive(Deserialize, Default)]
        struct TranslationData {
            #[serde(default)]
            title: String,
        }

        let url = format!(
            "{BASE_URL}/movie/{tmdb_id}?append_to_response=external_ids,translations&language=en-US"
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| format!("Error al llamar a TMDB /movie/{tmdb_id}"))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let body: DetailsResponse = resp
            .json()
            .await
            .context("Error al parsear TMDB /movie details")?;

        let imdb_id = body
            .imdb_id
            .or_else(|| body.external_ids.and_then(|e| e.imdb_id))
            .filter(|s| !s.is_empty() && s.starts_with("tt"));
        let original_title = body.original_title.filter(|s| !s.is_empty());
        let year = body
            .release_date
            .as_deref()
            .and_then(|s| s.get(..4))
            .and_then(|s| s.parse::<u16>().ok());
        let russian_title = body.translations.and_then(|t| {
            t.translations
                .into_iter()
                .find(|tr| tr.iso_639_1 == "ru")
                .map(|tr| tr.data.title)
                .filter(|s| !s.is_empty())
        });

        Ok(Some(MovieDetails {
            imdb_id,
            original_title,
            fallback_title: body.title,
            russian_title,
            original_language: body.original_language.filter(|s| !s.is_empty()),
            year,
            runtime: body.runtime.filter(|r| *r > 0),
        }))
    }

    /// Vista de detalle para el modal estilo Stremio: sinopsis, backdrop,
    /// runtime, géneros, etc. Endpoint distinto de `get_movie_details`
    /// para no acoplar la búsqueda de torrents con la UI de detalle.
    #[cfg(feature = "gui")]
    pub async fn get_movie_view(&self, tmdb_id: u64) -> Result<Option<MovieView>> {
        #[derive(Deserialize)]
        struct Resp {
            id: u64,
            title: String,
            #[serde(default)]
            original_title: Option<String>,
            #[serde(default)]
            overview: Option<String>,
            #[serde(default)]
            tagline: Option<String>,
            #[serde(default)]
            poster_path: Option<String>,
            #[serde(default)]
            backdrop_path: Option<String>,
            #[serde(default)]
            release_date: Option<String>,
            #[serde(default)]
            runtime: Option<u32>,
            #[serde(default)]
            vote_average: f32,
            #[serde(default)]
            genres: Vec<Genre>,
        }
        #[derive(Deserialize)]
        struct Genre {
            #[serde(default)]
            name: String,
        }

        let url = format!("{BASE_URL}/movie/{tmdb_id}?language=es-ES");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| format!("Error al llamar a TMDB /movie/{tmdb_id} (view)"))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let body: Resp = resp
            .json()
            .await
            .context("Error al parsear TMDB /movie (view)")?;

        Ok(Some(MovieView {
            id: body.id,
            title: body.title,
            original_title: body.original_title.filter(|s| !s.is_empty()),
            overview: body.overview.filter(|s| !s.is_empty()),
            tagline: body.tagline.filter(|s| !s.is_empty()),
            poster_path: body.poster_path,
            backdrop_path: body.backdrop_path,
            release_date: body.release_date.filter(|s| !s.is_empty()),
            runtime: body.runtime.filter(|r| *r > 0),
            vote_average: body.vote_average,
            genres: body
                .genres
                .into_iter()
                .map(|g| g.name)
                .filter(|s| !s.is_empty())
                .collect(),
        }))
    }
}

/// Vista de detalle de una película para el modal de la GUI.
#[cfg(feature = "gui")]
#[derive(Debug, Serialize, Clone)]
pub struct MovieView {
    pub id: u64,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub release_date: Option<String>,
    pub runtime: Option<u32>,
    pub vote_average: f32,
    pub genres: Vec<String>,
}

/// Detalles útiles de una película para búsquedas en providers de torrents.
pub struct MovieDetails {
    pub imdb_id: Option<String>,
    /// Título en el idioma original (típicamente inglés) — el que aparece en
    /// releases de scene/P2P. Puede faltar en pelis muy oscuras.
    pub original_title: Option<String>,
    /// Título en el idioma de la petición (fallback si `original_title` es
    /// None).
    pub fallback_title: Option<String>,
    /// Título en ruso (cirílico), útil como fallback para torrents rusos.
    pub russian_title: Option<String>,
    /// Idioma original de la película (ISO 639-1: `"en"`, `"es"`, ...).
    pub original_language: Option<String>,
    pub year: Option<u16>,
    /// Runtime en minutos (para calcular resume-seconds desde una
    /// fracción de bytes). `None` cuando TMDB no lo expone o es 0.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub runtime: Option<u32>,
}
