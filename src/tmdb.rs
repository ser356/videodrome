// POLÍTICA DE MUTEX: Si un Mutex<T> está envenenado significa que un hilo
// entró en pánico mientras lo sostenía — invariante del proceso rota. En un
// proceso de un solo WebView (Tauri) la única recuperación sensata es propagar
// el pánico. Por eso todos los `lock()` usan `.expect("mutex poisoned")` en
// lugar de `.unwrap()`: semánticamente equivalentes pero el mensaje documenta
// el invariante. Ver también `letterboxd.rs` con la misma política.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.themoviedb.org/3";
const RECS_CACHE_FILE: &str = "tmdb_recs_cache.json";
// TTL de 7 días para el endpoint `/movie/{id}/recommendations`. Antes
// era 24h y la lista completa se re-mezclaba cada día (TMDB reordena
// según señales globales) → una peli que aparecía ayer podía caerse
// del top-N hoy sin que el user hubiese hecho nada. Con 7 días la
// lista es estable durante una semana, y las nuevas semillas
// (películas que el user ve mientras tanto) la invalidan de manera
// natural al añadir freq nueva sobre pelis vecinas.
const RECS_CACHE_TTL_SECS: u64 = 7 * 24 * 3600;

// Caches adicionales anti-caída de TMDB. TTL más largo (7 días) porque
// los metadatos de una peli son ~inmutables: título, runtime, imdb_id,
// idioma original no cambian tras el estreno. Solo `overview` o
// `tagline` pueden ir mejorando con revisiones editoriales, pero eso
// es cosmético.
#[cfg(feature = "gui")]
const SEARCH_CACHE_FILE: &str = "tmdb_search_cache.json";
#[cfg(feature = "gui")]
const VIEW_CACHE_FILE: &str = "tmdb_view_cache.json";
#[cfg(feature = "gui")]
const DETAILS_CACHE_FILE: &str = "tmdb_details_cache.json";
/// Cache de `/tv/{id}` (detalles de serie): imdb_id, número de
/// temporadas, mapa season_number → episode_count, etc. TTL largo:
/// series canceladas o terminadas no cambian, y para series en
/// emisión un TTL de 7 días es aceptable (el user refresca
/// manualmente si añaden temporada nueva).
#[cfg(feature = "gui")]
const SERIES_DETAILS_CACHE_FILE: &str = "tmdb_series_details_cache.json";
/// Cache de `/tv/{id}/season/{n}` (lista de episodios de una
/// temporada). Cache pesada porque puede llegar a MB por temporada
/// larga, pero perfectamente estable — los episodios ya emitidos no
/// cambian. TTL largo igual que series details.
#[cfg(feature = "gui")]
const SEASON_CACHE_FILE: &str = "tmdb_season_cache.json";
#[cfg(feature = "gui")]
const LONG_CACHE_TTL_SECS: u64 = 7 * 24 * 3600;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TmdbMovie {
    /// ID interno de TMDB. `0` cuando el hit viene de un fallback que no
    /// pasa por TMDB (Cinemeta) y no hay TMDB id resoluble — en ese caso
    /// la GUI enruta la búsqueda de torrents por `imdb_id` / query directa.
    pub id: u64,
    pub title: String,
    /// Título en el idioma original (típicamente inglés — lo que
    /// scene/P2P usa en el naming). Puede coincidir con `title` si
    /// la peli es del idioma que TMDB nos devuelve. Se puebla desde
    /// `/search/movie` (siempre presente) y `/search/multi`. Crítico
    /// para el probe de `torrent_count` en `search_movies_tmdb`:
    /// buscar "La pasión de China Blue" en scene devuelve 0 hits,
    /// buscar "Crimes of Passion" devuelve decenas. `#[serde(default)]`
    /// para compat con cache disk pre-Fase.
    #[serde(default)]
    pub original_title: Option<String>,
    #[serde(default)]
    pub vote_average: f32,
    #[allow(dead_code)]
    #[serde(default)]
    pub popularity: f32,
    #[serde(default)]
    pub release_date: Option<String>, // "YYYY-MM-DD"
    /// Ruta relativa del poster en TMDB (ej. `/abc123.jpg`), o URL absoluta
    /// cuando el hit viene de Cinemeta. La GUI (`tmdbPoster`) detecta si
    /// empieza por `http` y en ese caso lo usa tal cual.
    #[serde(default)]
    pub poster_path: Option<String>,
    /// IMDb ID (`tt…`) cuando lo conocemos. TMDB no lo devuelve en
    /// `/search/movie`; se rellena para hits de Cinemeta (que sí lo dan
    /// nativamente) y sirve para búsquedas de torrents por IMDb id
    /// cuando TMDB no está disponible.
    #[serde(default)]
    pub imdb_id: Option<String>,
    /// Discriminador Movie/Series. Todos los endpoints
    /// pre-series poblaban implícitamente `Movie`; para no romper
    /// caches ni callers legacy usamos `#[serde(default)]` que da
    /// `MediaKind::Movie`. `search_multi` sí lo rellena según
    /// `media_type` de TMDB.
    #[serde(default)]
    pub kind: MediaKind,
}

/// Discriminador de tipo de contenido. Se serializa como
/// `"movie"` / `"series"` para que sea directo de consumir desde
/// TypeScript (matches el `media_type` de la API de TMDB salvo por
/// el rename `tv` → `series`, más natural en el dominio del user).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    #[default]
    Movie,
    Series,
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

// ── Caches genéricos anti-caída ────────────────────────────────────────────
//
// Cada endpoint de TMDB que consumimos se cachea en disco con un TTL
// largo. Cuando TMDB tiene un incidente (como el del 2026-07 que nos
// motivó esta iteración), las queries que el user ya había hecho
// alguna vez siguen respondiendo desde caché — así el flujo de
// "Cartelera → clic → torrents" no se rompe entero.
//
// Write-through: cada `insert` guarda el HashMap completo en disco.
// Los JSONs son pequeños (<200KB tras uso normal), la escritura es
// insignificante comparada con la latencia del propio TMDB.

#[cfg(feature = "gui")]
fn generic_cache_path(filename: &str) -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("videodrome");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(filename))
}

#[cfg(feature = "gui")]
fn load_generic<K, V>(filename: &str) -> HashMap<K, V>
where
    K: std::hash::Hash + Eq + serde::de::DeserializeOwned,
    V: serde::de::DeserializeOwned,
{
    let Ok(path) = generic_cache_path(filename) else {
        return HashMap::new();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

#[cfg(feature = "gui")]
fn save_generic<K, V>(filename: &str, map: &HashMap<K, V>)
where
    K: std::hash::Hash + Eq + Serialize,
    V: Serialize,
{
    if let Ok(path) = generic_cache_path(filename) {
        if let Ok(json) = serde_json::to_string(map) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// Envoltorio serializable con timestamp para poder aplicar TTL sin
/// depender de mtime del fichero (que se toca en cada save).
#[cfg(feature = "gui")]
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Timestamped<T> {
    timestamp: u64,
    value: T,
}

/// Devuelve `Some(v)` si la entrada existe y no ha expirado.
#[cfg(feature = "gui")]
fn get_fresh<K, V>(map: &HashMap<K, Timestamped<V>>, key: &K, ttl_secs: u64) -> Option<V>
where
    K: std::hash::Hash + Eq,
    V: Clone,
{
    let entry = map.get(key)?;
    if now_unix().saturating_sub(entry.timestamp) < ttl_secs {
        Some(entry.value.clone())
    } else {
        None
    }
}

pub struct TmdbClient<'a> {
    http: &'a reqwest::Client,
    bearer_token: &'a str,
    /// Código BCP47 que se pasa como `language=` en cada request
    /// para que TMDB devuelva `title`, `overview`, `genres` y
    /// `translations` en el idioma de la UI. Se pobla desde
    /// `Preferences.ui_language` (mapeado ISO 639-1 → BCP47 con
    /// `bcp47_locale`). Excepciones: los endpoints que necesitan
    /// EN por razones de matching de releases scene mantienen
    /// `en-US` hardcodeado — están documentados in-line.
    locale: String,
    cache: Mutex<HashMap<u64, CachedRecs>>,
    /// Cache de `/search/movie` — clave: query normalizada
    /// (`trim().to_lowercase()`), valor: lista de hits + timestamp.
    /// Solo se popula cuando TMDB responde OK; si TMDB peta y hay
    /// entrada fresca en caché, la usamos. Sin ella, cae a Cinemeta.
    #[cfg(feature = "gui")]
    search_cache: Mutex<HashMap<String, Timestamped<Vec<TmdbMovie>>>>,
    /// Cache de `/movie/{id}` (vista de detalle del modal).
    #[cfg(feature = "gui")]
    view_cache: Mutex<HashMap<u64, Timestamped<MovieView>>>,
    /// Cache de `/movie/{id}?append_to_response=external_ids,translations`
    /// (detalles usados por la búsqueda de torrents: imdb_id,
    /// original_title, russian_title, language, runtime).
    #[cfg(feature = "gui")]
    details_cache: Mutex<HashMap<u64, Timestamped<MovieDetails>>>,
    /// Cache de `/tv/{id}` — detalles de serie (imdb, temporadas,
    /// idioma). Comparte semántica con `details_cache` (misma
    /// política stale-serves-on-error).
    #[cfg(feature = "gui")]
    #[allow(dead_code)]
    series_details_cache: Mutex<HashMap<u64, Timestamped<SeriesDetails>>>,
    /// Cache de `/tv/{id}/season/{n}` — key compuesta `(tmdb_id,
    /// season_number)`. Se usa `String` como key para round-trip
    /// serde disco fácil (JSON no soporta tuplas como key).
    #[cfg(feature = "gui")]
    #[allow(dead_code)]
    season_cache: Mutex<HashMap<String, Timestamped<Vec<SeriesEpisode>>>>,
}

/// Mapea un código ISO 639-1 (o BCP47 ya normalizado) a un locale
/// BCP47 aceptado por TMDB. `""`, `None` o codigos desconocidos
/// devuelven `"en-US"` (por defecto neutro para el catálogo).
///
/// TMDB acepta cualquier locale BCP47 pero solo los "principales"
/// están traducidos completamente (title, overview, genres). Los
/// menos comunes cae al inglés silenciosamente — no rompe la app.
pub fn bcp47_locale(ui_lang: Option<&str>) -> String {
    let raw = ui_lang.unwrap_or("").trim().to_lowercase();
    // Ya está en formato BCP47 (contiene guion): úsalo tal cual.
    if raw.contains('-') && raw.len() >= 5 {
        return raw;
    }
    match raw.as_str() {
        "es" => "es-ES".to_string(),
        "en" => "en-US".to_string(),
        "fr" => "fr-FR".to_string(),
        "de" => "de-DE".to_string(),
        "it" => "it-IT".to_string(),
        "pt" => "pt-PT".to_string(),
        _ => "en-US".to_string(),
    }
}

impl<'a> TmdbClient<'a> {
    /// Constructor con locale explícito. Los formatos aceptados son
    /// ISO 639-1 (`"es"`, `"en"`…) o BCP47 (`"es-ES"`). Ver
    /// `bcp47_locale` para el mapeo.
    pub fn new(http: &'a reqwest::Client, bearer_token: &'a str, ui_lang: Option<&str>) -> Self {
        Self {
            http,
            bearer_token,
            locale: bcp47_locale(ui_lang),
            cache: Mutex::new(load_cache()),
            #[cfg(feature = "gui")]
            search_cache: Mutex::new(load_generic(SEARCH_CACHE_FILE)),
            #[cfg(feature = "gui")]
            view_cache: Mutex::new(load_generic(VIEW_CACHE_FILE)),
            #[cfg(feature = "gui")]
            details_cache: Mutex::new(load_generic(DETAILS_CACHE_FILE)),
            #[cfg(feature = "gui")]
            series_details_cache: Mutex::new(load_generic(SERIES_DETAILS_CACHE_FILE)),
            #[cfg(feature = "gui")]
            season_cache: Mutex::new(load_generic(SEASON_CACHE_FILE)),
        }
    }

    /// Recomendaciones de TMDB para una película, cacheadas en disco (TTL 24h)
    /// para no repetir la misma consulta en ejecuciones sucesivas.
    pub async fn get_recommendations(&self, tmdb_id: u64) -> Result<Vec<TmdbMovie>> {
        if let Some(cached) = self.cache.lock().expect("mutex poisoned").get(&tmdb_id) {
            if now_unix().saturating_sub(cached.timestamp) < RECS_CACHE_TTL_SECS {
                return Ok(cached.movies.clone());
            }
        }

        let url = format!(
            "{BASE_URL}/movie/{tmdb_id}/recommendations?language={loc}&page=1",
            loc = self.locale,
        );

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

        self.cache.lock().expect("mutex poisoned").insert(
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
        save_cache(&self.cache.lock().expect("mutex poisoned"));
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

    /// Busca películas por texto libre, resiliente a caídas de TMDB.
    ///
    /// Estrategia (equivalente a lo que hace Stremio al no depender solo
    /// de TMDB):
    ///
    /// 1. TMDB `/search/movie` → matches por título.
    /// 2. Si vienen pocos resultados y TMDB está vivo, se enriquece con
    ///    TMDB `/search/person` + `/person/{id}/movie_credits` filtrado
    ///    por `job = "Director"`. Esto permite que el user teclee un
    ///    nombre de director ("tarantino") y obtenga su filmografía.
    /// 3. Si TMDB entero está caído (paso 1 devuelve `Err`), se cae a
    ///    Cinemeta (`v3-cinemeta.strem.io`, el backend público de
    ///    Stremio) usando IMDb IDs para no romper la búsqueda.
    ///
    /// El orden de relevancia de TMDB se preserva; los hits de director
    /// se anexan al final; los de Cinemeta se usan solo si no hay
    /// respuesta de TMDB.
    ///
    /// **Legacy** (§7 audit series): la GUI usa ahora `search_multi`
    /// para poder devolver movie + tv en el mismo request. Este
    /// método queda como referencia + fallback API pero no está
    /// wired en el flujo actual — de ahí el `allow(dead_code)`.
    #[cfg(feature = "gui")]
    #[allow(dead_code)]
    pub async fn search_movies(&self, query: &str) -> Result<Vec<TmdbMovie>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }

        // Fast path: cache fresco en disco (TTL 7d). Evita ida a TMDB
        // en queries repetidas y sobrevive caídas de TMDB para
        // términos ya buscados. Si hay cache fresco, ya no probamos
        // director tampoco — el director se resolvió en la primera
        // llamada y quedó en el mismo cache.
        if let Some(cached) = self.cached_search(q) {
            return Ok(cached);
        }

        // Paso 1: título. Si TMDB responde error, saltamos a Cinemeta.
        let title_res = self.search_movies_by_title(q).await;
        let title_hits = match title_res {
            Ok(hits) => hits,
            Err(err) => {
                // TMDB inalcanzable (DNS, 5xx, rate-limit persistente).
                // Cinemeta funciona sin API key y suele estar arriba
                // aunque TMDB esté caído.
                if let Ok(cine) = search_cinemeta_movies(self.http, q).await {
                    if !cine.is_empty() {
                        return Ok(cine);
                    }
                }
                return Err(err);
            }
        };

        // Paso 2: si hay pocos matches por título, probamos director.
        // Umbral bajo para no gastar 2 llamadas extra en queries obvias.
        const DIRECTOR_THRESHOLD: usize = 3;
        if title_hits.len() >= DIRECTOR_THRESHOLD {
            return Ok(title_hits);
        }

        let dir_hits = self.search_movies_by_director(q).await.unwrap_or_default();

        // Dedup por TMDB id preservando el orden (título primero,
        // director después). Sin director hits, devolvemos title tal
        // cual — no queremos gastar Cinemeta cuando TMDB SÍ respondió.
        if dir_hits.is_empty() {
            return Ok(title_hits);
        }
        let mut seen: std::collections::HashSet<u64> = title_hits.iter().map(|m| m.id).collect();
        let mut merged = title_hits;
        for m in dir_hits {
            if m.id != 0 && seen.insert(m.id) {
                merged.push(m);
            }
        }

        // Sobrescribe el cache con la lista mergeada (título + director).
        // Así en la siguiente búsqueda no gastamos las 2 llamadas
        // extra del director.
        {
            let key = q.to_lowercase();
            let mut guard = self.search_cache.lock().expect("mutex poisoned");
            guard.insert(
                key,
                Timestamped {
                    timestamp: now_unix(),
                    value: merged.clone(),
                },
            );
            save_generic(SEARCH_CACHE_FILE, &guard);
        }
        Ok(merged)
    }

    /// TMDB `/search/movie` — búsqueda por título. Puerto directo del
    /// comportamiento anterior de `search_movies`, extraído para poder
    /// componer fallbacks encima. Cache disk 7d anti-caída: cuando TMDB
    /// responde OK guardamos; cuando peta y hay entrada fresca la
    /// servimos en su lugar (evita que Cinemeta se dispare por queries
    /// que ya conocíamos).
    ///
    /// `include_adult=true` porque videodrome es un cliente personal:
    /// películas censuradas, NC-17, o marcadas como adult en TMDB
    /// (Salò, Irreversible, Antichrist, mucho cine de autor europeo,
    /// documentales explícitos) las quiere ver el user, no hay que
    /// filtrarlas silenciosamente. Sin esto, esos títulos NO
    /// aparecen en /search/movie por defecto.
    #[cfg(feature = "gui")]
    async fn search_movies_by_title(&self, query: &str) -> Result<Vec<TmdbMovie>> {
        let key = query.trim().to_lowercase();
        let encoded = urlencoding::encode(query);
        let url = format!(
            "{BASE_URL}/search/movie?query={q}&language={loc}&include_adult=true&page=1",
            q = encoded,
            loc = self.locale,
        );
        let resp_result = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await;

        let resp = match resp_result {
            Ok(r) => r,
            Err(e) => {
                // Fallo de red antes de tener respuesta. Intentamos
                // servir cache aunque haya expirado (TTL infinito en
                // modo desespero) — el user prefiere resultados viejos
                // a un error.
                if let Some(cached) = self
                    .search_cache
                    .lock()
                    .expect("mutex poisoned")
                    .get(&key)
                    .cloned()
                {
                    return Ok(cached.value);
                }
                return Err(anyhow::Error::new(e).context(format!(
                    "Error al llamar a TMDB /search/movie para '{query}'"
                )));
            }
        };
        if !resp.status().is_success() {
            // 4xx/5xx: mismo fallback, sirve cache aunque expire.
            if let Some(cached) = self
                .search_cache
                .lock()
                .expect("mutex poisoned")
                .get(&key)
                .cloned()
            {
                return Ok(cached.value);
            }
            anyhow::bail!("TMDB /search/movie devolvi\u{f3} {}", resp.status());
        }
        let body: RecommendationsResponse = resp
            .json()
            .await
            .context("Error al parsear respuesta de TMDB /search/movie")?;

        // Solo guardamos hits no vacíos: si la query no matchea nada
        // en TMDB, guardarlo en caché nos impediría volver a probar la
        // siguiente vez (por si el user tecleó mal o TMDB indexa la
        // peli después).
        if !body.results.is_empty() {
            let mut guard = self.search_cache.lock().expect("mutex poisoned");
            guard.insert(
                key,
                Timestamped {
                    timestamp: now_unix(),
                    value: body.results.clone(),
                },
            );
            save_generic(SEARCH_CACHE_FILE, &guard);
        }
        Ok(body.results)
    }

    /// Sirve resultados de `/search/movie` desde cache disk si están
    /// frescos (dentro del TTL). Se llama ANTES del fetch en
    /// `search_movies` para saltarse la ida a TMDB entera en queries
    /// repetidas. Devuelve `None` si no hay cache o está expirado.
    #[cfg(feature = "gui")]
    fn cached_search(&self, query: &str) -> Option<Vec<TmdbMovie>> {
        let key = query.trim().to_lowercase();
        get_fresh(
            &self.search_cache.lock().expect("mutex poisoned"),
            &key,
            LONG_CACHE_TTL_SECS,
        )
    }

    /// Busca personas en TMDB y devuelve la filmografía como director
    /// del hit más relevante (si es de departamento "Directing"). Dos
    /// llamadas: `/search/person` + `/person/{id}/movie_credits`.
    ///
    /// Devuelve `Ok(vec![])` cuando no hay persona relevante — no es un
    /// error, simplemente el texto no era un nombre de director.
    #[cfg(feature = "gui")]
    async fn search_movies_by_director(&self, query: &str) -> Result<Vec<TmdbMovie>> {
        #[derive(Deserialize)]
        struct PersonSearchResp {
            #[serde(default)]
            results: Vec<PersonHit>,
        }
        #[derive(Deserialize)]
        struct PersonHit {
            id: u64,
            #[serde(default)]
            known_for_department: Option<String>,
            #[serde(default)]
            popularity: f32,
        }

        let encoded = urlencoding::encode(query);
        let url = format!(
            "{BASE_URL}/search/person?query={q}&language={loc}&include_adult=true&page=1",
            q = encoded,
            loc = self.locale,
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| format!("Error al llamar a TMDB /search/person para '{query}'"))?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let body: PersonSearchResp = resp
            .json()
            .await
            .context("Error al parsear respuesta de TMDB /search/person")?;

        // Solo consideramos personas cuyo departamento es "Directing".
        // Si el más popular no lo es, no seguimos: probablemente el user
        // buscó un actor y ya salió en title search.
        let Some(person) = body
            .results
            .into_iter()
            .filter(|p| {
                p.known_for_department.as_deref() == Some("Directing") && p.popularity > 0.0
            })
            .max_by(|a, b| {
                a.popularity
                    .partial_cmp(&b.popularity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        else {
            return Ok(vec![]);
        };

        #[derive(Deserialize)]
        struct CreditsResp {
            #[serde(default)]
            crew: Vec<CrewCredit>,
        }
        #[derive(Deserialize)]
        struct CrewCredit {
            id: u64,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            release_date: Option<String>,
            #[serde(default)]
            poster_path: Option<String>,
            #[serde(default)]
            vote_average: f32,
            #[serde(default)]
            popularity: f32,
            #[serde(default)]
            job: String,
        }

        let credits_url = format!(
            "{BASE_URL}/person/{pid}/movie_credits?language={loc}",
            pid = person.id,
            loc = self.locale,
        );
        let credits_resp = self
            .http
            .get(&credits_url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| {
                format!("Error al llamar a TMDB /person/{}/movie_credits", person.id)
            })?;
        if !credits_resp.status().is_success() {
            return Ok(vec![]);
        }
        let credits: CreditsResp = credits_resp
            .json()
            .await
            .context("Error al parsear TMDB /person/movie_credits")?;

        // Ordenados por año descendente (más recientes primero) para que
        // el user vea la filmografía reciente sin scrollear.
        let mut movies: Vec<TmdbMovie> = credits
            .crew
            .into_iter()
            .filter(|c| c.job == "Director")
            .filter_map(|c| {
                let title = c.title?;
                Some(TmdbMovie {
                    id: c.id,
                    title,
                    original_title: None,
                    vote_average: c.vote_average,
                    popularity: c.popularity,
                    release_date: c.release_date,
                    poster_path: c.poster_path,
                    imdb_id: None,
                    kind: MediaKind::Movie,
                })
            })
            .collect();
        movies.sort_by_key(|m| std::cmp::Reverse(m.year()));
        Ok(movies)
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
        // Fast path: cache disk fresco.
        #[cfg(feature = "gui")]
        {
            if let Some(cached) = get_fresh(
                &self.details_cache.lock().expect("mutex poisoned"),
                &tmdb_id,
                LONG_CACHE_TTL_SECS,
            ) {
                return Ok(Some(cached));
            }
        }

        match self.fetch_movie_details_uncached(tmdb_id).await {
            Ok(Some(details)) => {
                #[cfg(feature = "gui")]
                {
                    let mut guard = self.details_cache.lock().expect("mutex poisoned");
                    guard.insert(
                        tmdb_id,
                        Timestamped {
                            timestamp: now_unix(),
                            value: details.clone(),
                        },
                    );
                    save_generic(DETAILS_CACHE_FILE, &guard);
                }
                Ok(Some(details))
            }
            Ok(None) => Ok(None),
            Err(err) => {
                // TMDB caído: sirve cache expirado si lo hay. La info
                // de detalles no cambia entre incidentes, no
                // arriesgamos casi nada dando algo viejo.
                #[cfg(feature = "gui")]
                {
                    if let Some(stale) = self
                        .details_cache
                        .lock()
                        .expect("mutex poisoned")
                        .get(&tmdb_id)
                        .cloned()
                    {
                        return Ok(Some(stale.value));
                    }
                }
                Err(err)
            }
        }
    }

    /// Versión "fina" sin cache — solo pega a TMDB. Extraída para que
    /// `get_movie_details` pueda cachear + implementar fallback stale.
    async fn fetch_movie_details_uncached(&self, tmdb_id: u64) -> Result<Option<MovieDetails>> {
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
            #[serde(default)]
            alternative_titles: Option<AlternativeTitlesNested>,
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
        // Titulos alternativos: TMDB devuelve un array con
        // `iso_3166_1` (país) + `title` (+ opcional `type`).
        // Filtramos por país en Fase 3a: EN (US/GB), ES y el país
        // que coincida con el idioma original de la peli.
        #[derive(Deserialize)]
        struct AlternativeTitlesNested {
            #[serde(default)]
            titles: Vec<AltTitle>,
        }
        #[derive(Deserialize)]
        struct AltTitle {
            #[serde(default)]
            iso_3166_1: String,
            #[serde(default)]
            title: String,
        }

        let url = format!(
            "{BASE_URL}/movie/{tmdb_id}?append_to_response=external_ids,translations,alternative_titles&language=en-US"
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| format!("Error al llamar a TMDB /movie/{tmdb_id}"))?;
        if !resp.status().is_success() {
            // 404 real (peli no existe) → Ok(None), sin fallback.
            // 5xx / 429 → propagar como Err para activar stale cache.
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            anyhow::bail!("TMDB /movie/{tmdb_id} devolvi\u{f3} {}", resp.status());
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

        // Fase 3a: títulos alternativos. Filtramos por los países que
        // realmente nos ayudan a encontrar torrents:
        //   * `US`, `GB` — títulos en inglés (los que usa scene por
        //     defecto).
        //   * `ES` — títulos españoles (mercado hispano).
        //   * País mapeado al `original_language` (`FR` para
        //     `original_language = "fr"`, etc.) — el título nativo
        //     suele aparecer en indexers regionales.
        // Deduplicamos por título normalizado (con `to_lowercase +
        // trim`) para no meter la misma variante dos veces.
        let mut wanted_countries: Vec<&str> = vec!["US", "GB", "ES"];
        if let Some(orig) = body.original_language.as_deref() {
            let mapped = match orig {
                "fr" => Some("FR"),
                "it" => Some("IT"),
                "de" => Some("DE"),
                "ja" => Some("JP"),
                "ko" => Some("KR"),
                "zh" => Some("CN"),
                "pt" => Some("BR"),
                _ => None,
            };
            if let Some(c) = mapped {
                if !wanted_countries.contains(&c) {
                    wanted_countries.push(c);
                }
            }
        }
        let mut alt_titles: Vec<String> = Vec::new();
        let mut seen_alt: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Some(alt) = body.alternative_titles {
            for t in alt.titles {
                if t.title.is_empty() {
                    continue;
                }
                if !wanted_countries.contains(&t.iso_3166_1.as_str()) {
                    continue;
                }
                let key = t.title.trim().to_lowercase();
                if seen_alt.insert(key) {
                    alt_titles.push(t.title);
                }
                // Cap a 6 para no soplar `title_variants` — la Fase 3b
                // limita a ≤3 variantes en `search_all` de todas
                // formas, pero mantener aquí un tope bajo mantiene la
                // caché ligera.
                if alt_titles.len() >= 6 {
                    break;
                }
            }
        }

        Ok(Some(MovieDetails {
            imdb_id,
            original_title,
            fallback_title: body.title,
            russian_title,
            original_language: body.original_language.filter(|s| !s.is_empty()),
            year,
            runtime: body.runtime.filter(|r| *r > 0),
            release_date: body.release_date.filter(|s| !s.is_empty()),
            alt_titles,
        }))
    }

    /// Vista de detalle para el modal estilo Stremio: sinopsis, backdrop,
    /// runtime, géneros, etc. Endpoint distinto de `get_movie_details`
    /// para no acoplar la búsqueda de torrents con la UI de detalle.
    ///
    /// Cache disk 7d anti-caída de TMDB. Cuando TMDB falla y no hay
    /// cache, intenta Cinemeta si tenemos `imdb_id` cacheado desde
    /// `get_movie_details` para este mismo `tmdb_id` — mapeamos la
    /// respuesta de Cinemeta a `MovieView` (menos rica: sin backdrop
    /// generalmente, poster en URL absoluta de metahub, textos en
    /// inglés) pero suficiente para que el modal se abra.
    #[cfg(feature = "gui")]
    pub async fn get_movie_view(&self, tmdb_id: u64) -> Result<Option<MovieView>> {
        // Fast path: cache fresco.
        if let Some(cached) = get_fresh(
            &self.view_cache.lock().expect("mutex poisoned"),
            &tmdb_id,
            LONG_CACHE_TTL_SECS,
        ) {
            return Ok(Some(cached));
        }

        match self.fetch_movie_view_uncached(tmdb_id).await {
            Ok(Some(view)) => {
                let mut guard = self.view_cache.lock().expect("mutex poisoned");
                guard.insert(
                    tmdb_id,
                    Timestamped {
                        timestamp: now_unix(),
                        value: view.clone(),
                    },
                );
                save_generic(VIEW_CACHE_FILE, &guard);
                Ok(Some(view))
            }
            Ok(None) => Ok(None),
            Err(err) => {
                // 1) Sirve stale cache si lo hay.
                if let Some(stale) = self
                    .view_cache
                    .lock()
                    .expect("mutex poisoned")
                    .get(&tmdb_id)
                    .cloned()
                {
                    return Ok(Some(stale.value));
                }
                // 2) Cinemeta fallback: solo posible si tenemos imdb_id
                //    cacheado desde una llamada previa a get_movie_details.
                let imdb = self
                    .details_cache
                    .lock()
                    .expect("mutex poisoned")
                    .get(&tmdb_id)
                    .and_then(|d| d.value.imdb_id.clone());
                if let Some(imdb_id) = imdb {
                    if let Ok(Some(view)) = fetch_cinemeta_view(self.http, tmdb_id, &imdb_id).await
                    {
                        return Ok(Some(view));
                    }
                }
                Err(err)
            }
        }
    }

    #[cfg(feature = "gui")]
    async fn fetch_movie_view_uncached(&self, tmdb_id: u64) -> Result<Option<MovieView>> {
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

        let url = format!(
            "{BASE_URL}/movie/{tmdb_id}?language={loc}",
            loc = self.locale,
        );
        // Timeout específico corto (4s). El HTTP client global tiene
        // 20s, demasiado para el modal — el user prefiere ver "sin
        // sinopsis" al instante que un spinner colgado. Si TMDB
        // tarda >4s, `get_movie_view` cae a Cinemeta o stale cache.
        let start = std::time::Instant::now();
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            self.http.get(&url).bearer_auth(self.bearer_token).send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("TMDB /movie/{tmdb_id} (view) timeout tras 4s"))?
        .with_context(|| format!("Error al llamar a TMDB /movie/{tmdb_id} (view)"))?;
        tracing::debug!(
            target: "tmdb",
            tmdb_id,
            status = %resp.status(),
            elapsed_ms = start.elapsed().as_millis() as u64,
            "get_movie_view"
        );
        if !resp.status().is_success() {
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            anyhow::bail!(
                "TMDB /movie/{tmdb_id} (view) devolvi\u{f3} {}",
                resp.status()
            );
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

    // ── Series ────────────────────────────────────────────────────────

    /// `GET /search/multi` — búsqueda de películas + series a la vez.
    /// TMDB mezcla ambos en un solo array `results` etiquetados por
    /// `media_type` (`movie`/`tv`/`person`). Descartamos `person` y
    /// mapeamos `tv → MediaKind::Series`. El ranking (popularidad) ya
    /// viene mezclado — no reordenamos.
    ///
    /// La misma política de cache que `search_movies` no se aplica
    /// aquí por simplicidad: el caller (la GUI vía cache tsx) ya
    /// hace su propio memo/dedupe entre búsquedas repetidas.
    #[cfg(feature = "gui")]
    #[allow(dead_code)]
    pub async fn search_multi(&self, query: &str) -> Result<Vec<TmdbMovie>> {
        // Response mixto: cada hit trae `media_type` que discrimina.
        // Los campos también divergen: pelis usan `title`+`release_date`,
        // series usan `name`+`first_air_date`. Deserializamos con un
        // struct laxo que aceptа ambos y luego reconciliamos.
        #[derive(Deserialize)]
        struct MultiResp {
            #[serde(default)]
            results: Vec<MultiHit>,
        }
        #[derive(Deserialize)]
        struct MultiHit {
            #[serde(default)]
            id: u64,
            #[serde(default)]
            media_type: String,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            name: Option<String>,
            /// Título original (idioma nativo) — pelis: `original_title`,
            /// series: `original_name`. Necesario para el probe de
            /// `torrent_count` en `search_movies_tmdb`: buscar el
            /// título localizado en apibay/knaben da 0 hits, buscar
            /// el original da los reales del scene.
            #[serde(default)]
            original_title: Option<String>,
            #[serde(default)]
            original_name: Option<String>,
            #[serde(default)]
            release_date: Option<String>,
            #[serde(default)]
            first_air_date: Option<String>,
            #[serde(default)]
            vote_average: f32,
            #[serde(default)]
            popularity: f32,
            #[serde(default)]
            poster_path: Option<String>,
        }

        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let encoded = urlencoding::encode(q);
        let url = format!(
            "{BASE_URL}/search/multi?query={query}&language={loc}&include_adult=true&page=1",
            query = encoded,
            loc = self.locale,
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| "Error al llamar a TMDB /search/multi".to_string())?;
        if !resp.status().is_success() {
            anyhow::bail!("TMDB /search/multi devolvi\u{f3} {}", resp.status());
        }
        let body: MultiResp = resp
            .json()
            .await
            .context("Error al parsear TMDB /search/multi")?;

        let mut out = Vec::with_capacity(body.results.len());
        for hit in body.results {
            let kind = match hit.media_type.as_str() {
                "movie" => MediaKind::Movie,
                "tv" => MediaKind::Series,
                _ => continue, // person u otro
            };
            let title = match kind {
                MediaKind::Movie => hit.title.unwrap_or_default(),
                MediaKind::Series => hit.name.unwrap_or_default(),
            };
            if title.is_empty() {
                continue;
            }
            let original_title = match kind {
                MediaKind::Movie => hit.original_title,
                MediaKind::Series => hit.original_name,
            }
            .filter(|s| !s.is_empty() && s != &title);
            let release_date = match kind {
                MediaKind::Movie => hit.release_date,
                MediaKind::Series => hit.first_air_date,
            };
            out.push(TmdbMovie {
                id: hit.id,
                title,
                original_title,
                vote_average: hit.vote_average,
                popularity: hit.popularity,
                release_date: release_date.filter(|s| !s.is_empty()),
                poster_path: hit.poster_path,
                imdb_id: None,
                kind,
            });
        }
        Ok(out)
    }

    /// `GET /tv/{id}` con append de external_ids + alternative_titles.
    /// Cache disco 7d (`SERIES_DETAILS_CACHE_FILE`); stale-serves
    /// cuando TMDB da error. Análogo a `get_movie_details`.
    #[cfg(feature = "gui")]
    #[allow(dead_code)]
    pub async fn get_series_details(&self, tmdb_id: u64) -> Result<Option<SeriesDetails>> {
        if let Some(cached) = get_fresh(
            &self.series_details_cache.lock().expect("mutex poisoned"),
            &tmdb_id,
            LONG_CACHE_TTL_SECS,
        ) {
            return Ok(Some(cached));
        }
        match self.fetch_series_details_uncached(tmdb_id).await {
            Ok(Some(details)) => {
                let mut guard = self.series_details_cache.lock().expect("mutex poisoned");
                guard.insert(
                    tmdb_id,
                    Timestamped {
                        timestamp: now_unix(),
                        value: details.clone(),
                    },
                );
                save_generic(SERIES_DETAILS_CACHE_FILE, &guard);
                Ok(Some(details))
            }
            Ok(None) => Ok(None),
            Err(err) => {
                if let Some(stale) = self
                    .series_details_cache
                    .lock()
                    .expect("mutex poisoned")
                    .get(&tmdb_id)
                    .cloned()
                {
                    return Ok(Some(stale.value));
                }
                Err(err)
            }
        }
    }

    #[cfg(feature = "gui")]
    async fn fetch_series_details_uncached(&self, tmdb_id: u64) -> Result<Option<SeriesDetails>> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            id: u64,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            original_name: Option<String>,
            #[serde(default)]
            original_language: Option<String>,
            #[serde(default)]
            overview: Option<String>,
            #[serde(default)]
            first_air_date: Option<String>,
            #[serde(default)]
            poster_path: Option<String>,
            #[serde(default)]
            backdrop_path: Option<String>,
            #[serde(default)]
            number_of_seasons: u16,
            #[serde(default)]
            status: Option<String>,
            #[serde(default)]
            seasons: Vec<SeasonRaw>,
            #[serde(default)]
            external_ids: Option<ExternalIds>,
            #[serde(default)]
            alternative_titles: Option<AltTitlesTv>,
        }
        #[derive(Deserialize)]
        struct SeasonRaw {
            #[serde(default)]
            season_number: u16,
            #[serde(default)]
            episode_count: u16,
            #[serde(default)]
            air_date: Option<String>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            poster_path: Option<String>,
        }
        #[derive(Deserialize)]
        struct ExternalIds {
            #[serde(default)]
            imdb_id: Option<String>,
        }
        // `/tv/{id}/alternative_titles` devuelve `results[]` (a
        // diferencia de `/movie/{id}/alternative_titles` que usa
        // `titles[]`). Cada entrada trae `iso_3166_1 + title` (y
        // opcional `type` — ej. "original title"); filtramos por
        // países útiles como en la rama de películas.
        #[derive(Deserialize)]
        struct AltTitlesTv {
            #[serde(default)]
            results: Vec<AltTitleTv>,
        }
        #[derive(Deserialize)]
        struct AltTitleTv {
            #[serde(default)]
            iso_3166_1: String,
            #[serde(default)]
            title: String,
        }

        let url = format!(
            "{BASE_URL}/tv/{tmdb_id}?append_to_response=external_ids,alternative_titles&language={loc}",
            loc = self.locale,
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| format!("Error al llamar a TMDB /tv/{tmdb_id}"))?;
        if !resp.status().is_success() {
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            anyhow::bail!("TMDB /tv/{tmdb_id} devolvi\u{f3} {}", resp.status());
        }
        let body: Resp = resp
            .json()
            .await
            .context("Error al parsear TMDB /tv details")?;

        let name = body.name.filter(|s| !s.is_empty()).unwrap_or_default();
        if name.is_empty() {
            return Ok(None);
        }
        let imdb_id = body
            .external_ids
            .and_then(|e| e.imdb_id)
            .filter(|s| !s.is_empty() && s.starts_with("tt"));
        let mut seasons: Vec<SeriesSeasonSummary> = body
            .seasons
            .into_iter()
            .map(|s| SeriesSeasonSummary {
                season_number: s.season_number,
                episode_count: s.episode_count,
                air_date: s.air_date.filter(|d| !d.is_empty()),
                name: s.name.filter(|n| !n.is_empty()),
                poster_path: s.poster_path,
            })
            .collect();
        seasons.sort_by_key(|s| s.season_number);

        // Alt titles: mismo criterio de países que la rama de pelis
        // — US/GB (inglés scene) + ES (mercado hispano) + país
        // mapeado desde `original_language` (JP/CN/KR/FR/IT/DE/BR).
        // Cap a 6, dedup por lowercase trim.
        let mut wanted_countries: Vec<&str> = vec!["US", "GB", "ES"];
        if let Some(orig) = body.original_language.as_deref() {
            let mapped = match orig {
                "fr" => Some("FR"),
                "it" => Some("IT"),
                "de" => Some("DE"),
                "ja" => Some("JP"),
                "ko" => Some("KR"),
                "zh" => Some("CN"),
                "pt" => Some("BR"),
                _ => None,
            };
            if let Some(c) = mapped {
                if !wanted_countries.contains(&c) {
                    wanted_countries.push(c);
                }
            }
        }
        let mut alt_titles: Vec<String> = Vec::new();
        let mut seen_alt: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Some(alt) = body.alternative_titles {
            for t in alt.results {
                if t.title.is_empty() {
                    continue;
                }
                if !wanted_countries.contains(&t.iso_3166_1.as_str()) {
                    continue;
                }
                let key = t.title.trim().to_lowercase();
                if seen_alt.insert(key) {
                    alt_titles.push(t.title);
                }
                if alt_titles.len() >= 6 {
                    break;
                }
            }
        }

        Ok(Some(SeriesDetails {
            id: body.id,
            name,
            original_name: body.original_name.filter(|s| !s.is_empty()),
            imdb_id,
            original_language: body.original_language.filter(|s| !s.is_empty()),
            overview: body.overview.filter(|s| !s.is_empty()),
            first_air_date: body.first_air_date.filter(|s| !s.is_empty()),
            poster_path: body.poster_path,
            backdrop_path: body.backdrop_path,
            seasons,
            number_of_seasons: body.number_of_seasons,
            status: body.status.filter(|s| !s.is_empty()),
            alt_titles,
        }))
    }

    /// `GET /tv/{id}/season/{n}` — lista de episodios. Cache 7d
    /// (los episodios ya emitidos no cambian). Devuelve `Ok(vec![])`
    /// si TMDB devuelve 404 (temporada inexistente).
    #[cfg(feature = "gui")]
    #[allow(dead_code)]
    pub async fn get_season(&self, tmdb_id: u64, season: u16) -> Result<Vec<SeriesEpisode>> {
        let key = format!("{tmdb_id}:{season}");
        if let Some(cached) = get_fresh(
            &self.season_cache.lock().expect("mutex poisoned"),
            &key,
            LONG_CACHE_TTL_SECS,
        ) {
            return Ok(cached);
        }

        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            episodes: Vec<EpisodeRaw>,
        }
        #[derive(Deserialize)]
        struct EpisodeRaw {
            #[serde(default)]
            season_number: u16,
            #[serde(default)]
            episode_number: u16,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            overview: Option<String>,
            #[serde(default)]
            air_date: Option<String>,
            #[serde(default)]
            still_path: Option<String>,
            #[serde(default)]
            runtime: Option<u32>,
        }

        let url = format!(
            "{BASE_URL}/tv/{tmdb_id}/season/{season}?language={loc}",
            loc = self.locale,
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token)
            .send()
            .await
            .with_context(|| format!("Error al llamar a TMDB /tv/{tmdb_id}/season/{season}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !resp.status().is_success() {
            // Servir stale si lo hay, igual que las otras APIs.
            if let Some(stale) = self
                .season_cache
                .lock()
                .expect("mutex poisoned")
                .get(&key)
                .cloned()
            {
                return Ok(stale.value);
            }
            anyhow::bail!(
                "TMDB /tv/{tmdb_id}/season/{season} devolvi\u{f3} {}",
                resp.status()
            );
        }
        let body: Resp = resp
            .json()
            .await
            .context("Error al parsear TMDB /tv/{tmdb_id}/season/{season}")?;

        let episodes: Vec<SeriesEpisode> = body
            .episodes
            .into_iter()
            .map(|e| SeriesEpisode {
                season_number: e.season_number,
                episode_number: e.episode_number,
                name: e.name.filter(|n| !n.is_empty()),
                overview: e.overview.filter(|o| !o.is_empty()),
                air_date: e.air_date.filter(|d| !d.is_empty()),
                still_path: e.still_path,
                runtime: e.runtime.filter(|r| *r > 0),
            })
            .collect();

        let mut guard = self.season_cache.lock().expect("mutex poisoned");
        guard.insert(
            key,
            Timestamped {
                timestamp: now_unix(),
                value: episodes.clone(),
            },
        );
        save_generic(SEASON_CACHE_FILE, &guard);
        Ok(episodes)
    }
}

/// Vista de detalle de una película para el modal de la GUI. Se
/// deserializa también para poder round-tripear a través del cache
/// en disco (`tmdb_view_cache.json`).
#[cfg(feature = "gui")]
#[derive(Debug, Serialize, Deserialize, Clone)]
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
/// Se serializa para poder cachear en disco (`tmdb_details_cache.json`)
/// — así, si TMDB se cae después de que el user haya abierto una peli
/// alguna vez, la búsqueda de torrents sigue funcionando desde caché.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Fecha de estreno TMDB (`YYYY-MM-DD`). Se usa para el mensaje
    /// "todavía en cines" cuando la búsqueda de torrents da vacío
    /// (Fase 4b del audit — sirve para distinguir "no hay releases"
    /// de "aún no ha salido en digital"). `None` si TMDB no la expone.
    #[serde(default)]
    pub release_date: Option<String>,
    /// Títulos alternativos filtrados (endpoint
    /// `/movie/{id}/alternative_titles`). Se guardan los del país
    /// original + ES/US/GB, deduplicados normalizados. Alimenta el
    /// `title_variants` de `MovieQuery` en la búsqueda de torrents
    /// (Fase 3a/3b del audit — mejora recall en pelis no inglesas
    /// o con subtítulos largos).
    ///
    /// `#[serde(default)]` para compatibilidad con caches antiguos
    /// (`tmdb_details_cache.json` pre-Fase 3): al deserializar una
    /// entrada vieja quedará vacío y la próxima vez que se refresque
    /// se poblará.
    #[serde(default)]
    pub alt_titles: Vec<String>,
}

// ── Series: tipos del dominio ──────────────────────────────────────────────
//
// TmdbMovie ya cubre "resultado listable de TMDB" con el campo
// `kind` — no duplicamos ese struct. Aquí solo lo específico de
// series: detalles enriquecidos (imdb, temporadas, idioma) y
// episodios de una temporada. Se serializa para cache disco.
//
// Todo el bloque queda gated por `feature = "gui"` porque los
// métodos que los consumen (`get_series_details`, `get_season`,
// `search_multi`) también lo están: la CLI/TUI aún no soporta
// series, solo la GUI. Sin este gate salían warnings dead_code en
// builds no-gui.

#[cfg(feature = "gui")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesDetails {
    pub id: u64,
    /// Nombre localizado (según language de la petición).
    pub name: String,
    /// Nombre en el idioma original. Fallback a `name` si vacío.
    pub original_name: Option<String>,
    /// IMDb id DE LA SERIE (no del episodio). Es la clave para las
    /// búsquedas por id en EZTV/Torznab (`tt` prefix incluido).
    pub imdb_id: Option<String>,
    /// Idioma original (`en`, `es`, `ja`…). Se propaga a MediaQuery
    /// para el ranking multi-idioma del score.
    pub original_language: Option<String>,
    /// Sinopsis general. `None` si TMDB no la expone.
    pub overview: Option<String>,
    /// Fecha del primer episodio de la serie completa (`YYYY-MM-DD`).
    /// Se usa como "año" en badges y como argumento para el matcher
    /// de año cuando el user busca la serie por texto.
    pub first_air_date: Option<String>,
    /// Poster + backdrop (paths relativos TMDB o URL absoluta si
    /// vienen de Cinemeta).
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    /// Temporadas. Ordenadas por `season_number`. TMDB expone la
    /// "Season 0" (specials) — la incluimos porque a veces la gente
    /// quiere ver los especiales, aunque el default de la UI puede
    /// ocultarla.
    pub seasons: Vec<SeriesSeasonSummary>,
    /// Número total de temporadas según TMDB (excluye specials).
    pub number_of_seasons: u16,
    /// Estado de emisión (`Returning Series`, `Ended`, `Canceled`,
    /// `In Production`…). Se muestra en la UI como badge.
    pub status: Option<String>,
    /// Títulos alternativos filtrados por país
    /// (`/tv/{id}/alternative_titles`). Alimenta `title_variants`
    /// de `MovieQuery` en la búsqueda de torrents — clave para
    /// series no anglosajonas (CJK, cirílico) donde `original_name`
    /// por sí solo no matchea releases scene con transliteración.
    /// `#[serde(default)]` para cache disk compat con versiones
    /// pre-alt_titles-series.
    #[serde(default)]
    pub alt_titles: Vec<String>,
}

/// Resumen de una temporada tal cual lo lista `/tv/{id}` en
/// `seasons[]`. Sin lista de episodios (para eso está
/// `get_season`) — solo lo que la UI necesita para pintar los
/// tabs de selección de temporada.
#[cfg(feature = "gui")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesSeasonSummary {
    pub season_number: u16,
    pub episode_count: u16,
    pub air_date: Option<String>,
    pub name: Option<String>,
    pub poster_path: Option<String>,
}

/// Episodio individual dentro de una temporada. Datos mínimos para
/// listar en la UI y para el matching de torrents por (S,E).
#[cfg(feature = "gui")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesEpisode {
    pub season_number: u16,
    pub episode_number: u16,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub still_path: Option<String>,
    /// Runtime del episodio en minutos, cuando TMDB lo expone.
    /// Los procedurals suelen tenerlo, las prestige a menudo no.
    pub runtime: Option<u32>,
}

// ── Cinemeta (fallback anti-caída de TMDB) ─────────────────────────────────
//
// Cinemeta es el catálogo público de metadatos de Stremio
// (`v3-cinemeta.strem.io`). Sirve búsqueda por título indexada por IMDb ID,
// sin API key ni auth, y sigue funcionando cuando TMDB tiene un incidente.
// Solo se usa como fallback: si TMDB responde OK (aunque sea con 0 hits),
// nos quedamos con TMDB — Cinemeta no tiene búsqueda por director.

#[cfg(feature = "gui")]
const CINEMETA_BASE: &str = "https://v3-cinemeta.strem.io";

#[cfg(feature = "gui")]
#[derive(Debug, Deserialize)]
struct CinemetaResp {
    #[serde(default)]
    metas: Vec<CinemetaMeta>,
}

#[cfg(feature = "gui")]
#[derive(Debug, Deserialize)]
struct CinemetaMeta {
    /// IMDb ID (`tt…`).
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    poster: Option<String>,
    /// Cinemeta puede devolver el año como string ("1999") o como
    /// rango ("1999-2003") en series. Tratamos ambos como texto y
    /// extraemos los primeros 4 chars.
    #[serde(default)]
    year: Option<String>,
    /// Rating IMDb como string ("8.7"). En su ausencia queda 0.0.
    #[serde(default, rename = "imdbRating")]
    imdb_rating: Option<String>,
}

/// Busca películas en Cinemeta por texto libre. Devuelve `TmdbMovie`s con
/// `id = 0` (no hay TMDB id) y `imdb_id = Some("tt…")` para que la GUI
/// pueda encaminar la búsqueda de torrents por IMDb / query directa.
#[cfg(feature = "gui")]
pub async fn search_cinemeta_movies(http: &reqwest::Client, query: &str) -> Result<Vec<TmdbMovie>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let url = format!(
        "{CINEMETA_BASE}/catalog/movie/top/search={}.json",
        urlencoding::encode(q)
    );
    let resp = http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Error al llamar a Cinemeta para '{q}'"))?;
    if !resp.status().is_success() {
        anyhow::bail!("Cinemeta devolvi\u{f3} {}", resp.status());
    }
    let body: CinemetaResp = resp
        .json()
        .await
        .context("Error al parsear respuesta de Cinemeta")?;

    Ok(body
        .metas
        .into_iter()
        .filter(|m| m.id.starts_with("tt") && !m.name.is_empty())
        .map(|m| {
            let year = m
                .year
                .as_deref()
                .and_then(|s| s.get(..4))
                .and_then(|s| s.parse::<u16>().ok());
            let vote = m
                .imdb_rating
                .as_deref()
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0);
            TmdbMovie {
                id: 0,
                title: m.name,
                original_title: None,
                vote_average: vote,
                popularity: 0.0,
                release_date: year.map(|y| format!("{y}-01-01")),
                poster_path: m.poster,
                imdb_id: Some(m.id),
                kind: MediaKind::Movie,
            }
        })
        .collect())
}

/// Cinemeta `/meta/movie/{imdbId}.json` → mapeado a `MovieView`. Es el
/// fallback del modal de detalle cuando TMDB está caído y no hay
/// cache. Perdemos calidad respecto a TMDB:
///
///   * Textos en inglés (Cinemeta no localiza).
///   * `poster_path` viene como URL absoluta de metahub — el frontend
///     ya detecta URLs con `http://` en `tmdbPoster()` y pasa a través.
///   * `backdrop_path` a veces ausente.
///   * `runtime` viene como string ("136 min"), lo parseamos.
///
/// El `tmdb_id` viene solo por preservar el identificador del caller
/// (Cinemeta no lo conoce).
#[cfg(feature = "gui")]
async fn fetch_cinemeta_view(
    http: &reqwest::Client,
    tmdb_id: u64,
    imdb_id: &str,
) -> Result<Option<MovieView>> {
    #[derive(Deserialize)]
    struct Wrap {
        #[serde(default)]
        meta: Option<Meta>,
    }
    #[derive(Deserialize)]
    struct Meta {
        #[serde(default)]
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        tagline: Option<String>,
        #[serde(default)]
        poster: Option<String>,
        #[serde(default)]
        background: Option<String>,
        #[serde(default)]
        year: Option<String>,
        #[serde(default)]
        runtime: Option<String>,
        #[serde(default, rename = "imdbRating")]
        imdb_rating: Option<String>,
        #[serde(default)]
        genres: Vec<String>,
    }

    let url = format!("{CINEMETA_BASE}/meta/movie/{imdb_id}.json");
    let resp = http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Error al llamar a Cinemeta /meta para {imdb_id}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("Cinemeta /meta devolvi\u{f3} {}", resp.status());
    }
    let body: Wrap = resp
        .json()
        .await
        .context("Error al parsear Cinemeta /meta")?;
    let Some(meta) = body.meta else {
        return Ok(None);
    };
    if meta.name.is_empty() {
        return Ok(None);
    }

    let runtime = meta
        .runtime
        .as_deref()
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|r| *r > 0);
    let release_date = meta
        .year
        .as_deref()
        .and_then(|y| y.get(..4))
        .map(|y| format!("{y}-01-01"));
    let vote_average = meta
        .imdb_rating
        .as_deref()
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);

    Ok(Some(MovieView {
        id: tmdb_id,
        title: meta.name,
        original_title: None,
        overview: meta.description.filter(|s| !s.is_empty()),
        tagline: meta.tagline.filter(|s| !s.is_empty()),
        poster_path: meta.poster,
        backdrop_path: meta.background,
        release_date,
        runtime,
        vote_average,
        genres: meta.genres.into_iter().filter(|s| !s.is_empty()).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmdb_movie_year_parses_from_release_date() {
        let m = TmdbMovie {
            id: 1,
            title: "X".into(),
            original_title: None,
            vote_average: 0.0,
            popularity: 0.0,
            release_date: Some("2020-05-15".into()),
            poster_path: None,
            imdb_id: None,
            kind: MediaKind::Movie,
        };
        assert_eq!(m.year(), Some(2020));
    }

    #[test]
    fn tmdb_movie_year_none_when_missing_or_bad() {
        let mut m = TmdbMovie {
            id: 1,
            title: "X".into(),
            original_title: None,
            vote_average: 0.0,
            popularity: 0.0,
            release_date: None,
            poster_path: None,
            imdb_id: None,
            kind: MediaKind::Movie,
        };
        assert_eq!(m.year(), None);
        m.release_date = Some("bad".into());
        assert_eq!(m.year(), None);
        m.release_date = Some("".into());
        assert_eq!(m.year(), None);
    }

    #[test]
    fn bcp47_locale_maps_iso_639() {
        assert_eq!(bcp47_locale(Some("es")), "es-ES");
        assert_eq!(bcp47_locale(Some("en")), "en-US");
        assert_eq!(bcp47_locale(Some("fr")), "fr-FR");
        assert_eq!(bcp47_locale(Some("de")), "de-DE");
        assert_eq!(bcp47_locale(Some("it")), "it-IT");
        assert_eq!(bcp47_locale(Some("pt")), "pt-PT");
    }

    #[test]
    fn bcp47_locale_defaults_to_en_us() {
        assert_eq!(bcp47_locale(None), "en-US");
        assert_eq!(bcp47_locale(Some("")), "en-US");
        assert_eq!(bcp47_locale(Some("ja")), "en-US");
        assert_eq!(bcp47_locale(Some("zh")), "en-US");
    }

    #[test]
    fn bcp47_locale_passes_through_bcp47() {
        assert_eq!(bcp47_locale(Some("pt-BR")), "pt-br");
        assert_eq!(bcp47_locale(Some("en-GB")), "en-gb");
        // 4-char raro no lo consideramos bcp47 completo → fallback.
        assert_eq!(bcp47_locale(Some("es-x")), "en-US");
    }

    #[test]
    fn bcp47_locale_trims_and_lowercases() {
        assert_eq!(bcp47_locale(Some("  ES  ")), "es-ES");
    }

    #[test]
    fn media_kind_default_is_movie() {
        assert_eq!(MediaKind::default(), MediaKind::Movie);
    }

    #[test]
    fn media_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&MediaKind::Movie).unwrap(),
            "\"movie\""
        );
        assert_eq!(
            serde_json::to_string(&MediaKind::Series).unwrap(),
            "\"series\""
        );
        let back: MediaKind = serde_json::from_str("\"series\"").unwrap();
        assert_eq!(back, MediaKind::Series);
    }
}
