//! Búsqueda de torrents para películas.
//!
//! Define un trait `TorrentProvider` con implementaciones para varias fuentes
//! (YTS, Apibay, Knaben, Torznab). `search_all` las consulta en paralelo,
//! dedupe por infohash y ordena por seeders × calidad × idioma.

use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub mod apibay;
pub mod knaben;
pub mod release_name;
pub mod torznab;
pub mod yts;

/// Presupuesto máximo por provider (Fase 1b). Un provider colgado no
/// puede retrasar la lista entera — con FuturesUnordered los demás
/// siguen entregando, pero el `while let Some(...)` no cierra hasta
/// que TODOS resuelven. Timeout individual ⇒ el proveedor lento se
/// marca como `ok=false, error="timeout"` y libera el join.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(8);

/// Backoff antes del retry único (Fase 1b). Solo se reintenta si el
/// primer intento devolvió Err distinto de timeout — HTTP 4xx / 5xx
/// se propagan sin retry para no martillear un provider caído.
const PROVIDER_RETRY_BACKOFF: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Torrent {
    pub title: String,
    pub magnet: String,
    pub size_bytes: u64,
    pub seeders: u32,
    pub leechers: u32,
    pub quality: Option<String>,
    /// Nombre del provider que devolvió este release (`"yts"`,
    /// `"apibay"`, `"knaben"`, `"torznab"`). Antes era `&'static str`
    /// pero se cambió a `String` en la Fase 4a del audit para poder
    /// deserializar desde el caché de disco (`torrent_search_cache.json`).
    pub source: String,
    /// Cómo matchea este release contra la query (episodio suelto,
    /// pack de temporada, pack de serie, película). Se rellena en
    /// `search_all` tras parsear el nombre con `release_name::parse`
    /// y comparar con `query.kind/season/episode`. Sirve para:
    ///   * pintar un badge en la UI ("E03" / "Pack S01" / …)
    ///   * modular el score final (`match_multiplier` en `score()`)
    #[serde(default)]
    pub match_kind: MatchKind,
    /// Infohash extraído del magnet (para dedupe). No se serializa al JSON
    /// para no ensuciar la salida.
    #[serde(skip)]
    pub infohash: String,
}

/// Cómo un release matchea contra la query. Por defecto `Movie`
/// (equivale a "no aplica el mundo de series"), lo que preserva el
/// comportamiento pre-audit para películas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// Película o compat legacy — sin distinción de series.
    #[default]
    Movie,
    /// Episodio exacto: parsed.season==query.season y
    /// parsed.episode==query.episode.
    Episode,
    /// Pack de temporada completa: parsed.season matchea la pedida,
    /// sin episodio en el nombre, o frase "Season N".
    SeasonPack,
    /// Pack de serie completa: frases "Complete Series",
    /// "Complete Collection", "Mini-Series", etc.
    SeriesPack,
}

#[derive(Debug, Clone, Default)]
pub struct MovieQuery {
    /// Tipo de contenido (película o serie). Poblado por el caller
    /// desde `TmdbMovie.kind` — cambia la política de matching en
    /// `search_all` (para películas, `is_tv_release ⇒ descartar`;
    /// para series, aceptar episodio exacto / pack).
    ///
    /// Default `Movie` para no romper callers legacy que crean el
    /// struct sin especificarlo.
    #[allow(dead_code)]
    pub kind: crate::tmdb::MediaKind,
    pub title: String,
    pub year: Option<u16>,
    pub imdb_id: Option<String>,
    /// TMDB ID. Actualmente ningún provider lo usa (todos aceptan IMDb o
    /// keywords), pero se acepta en la CLI para futuros providers.
    #[allow(dead_code)]
    pub tmdb_id: Option<u64>,
    /// Idioma original de la película (ISO 639-1: `"en"`, `"es"`, `"ru"`…).
    /// Se usa para rankear los torrents: los que llevan audio en este
    /// idioma (o "Original"/"Multi") suben en el score frente a doblajes.
    pub original_language: Option<String>,
    /// Conjunto de títulos "válidos" para el matching por nombre en
    /// `search_all`. Poblado por el caller con variantes de TMDB
    /// (original, inglés, `alternative_titles` — ver Fase 3 del audit).
    /// Cada variante se compara contra `parsed.title` tras
    /// `release_name::normalize_title` — si el release matchea al
    /// menos UNA, se acepta.
    ///
    /// **Compatibilidad**: si el vector está vacío, `search_all` NO
    /// aplica filtro de título (comportamiento legacy). El filtro
    /// pre-Fase-2 (`filter_by_token_overlap` en knaben.rs) queda
    /// desactivado para dejar TODOS los providers en el mismo embudo.
    pub title_variants: Vec<String>,
    /// Temporada objetivo cuando `kind == Series`. `None` = película
    /// o "cualquier temporada" (poco útil — la GUI siempre lo pone).
    #[allow(dead_code)]
    pub season: Option<u16>,
    /// Episodio objetivo. `Some(N)` = episodio exacto. `None` con
    /// `season = Some(S)` = pack de temporada completo. Ambos `None`
    /// = pack de serie completo o película.
    #[allow(dead_code)]
    pub episode: Option<u16>,
}

impl MovieQuery {
    /// Cadena de búsqueda por defecto (para providers que no soportan IDs).
    #[allow(dead_code)]
    pub fn keywords(&self) -> String {
        match self.year {
            Some(y) => format!("{} {}", self.title, y),
            None => self.title.clone(),
        }
    }
}

#[async_trait]
pub trait TorrentProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>>;
}

/// Telemetría del resultado de UN provider dentro de `search_all`. Se
/// devuelve junto a los torrents para que la GUI/TUI pinte una línea
/// tipo `knaben ✓ 34 · apibay ✗ timeout · yts ✓ 5` y el user vea que
/// los resultados escasos vienen de un provider caído, no de "no hay
/// releases". Antes del audit, `search_all` silenciaba estos errores
/// (`let Ok(items) = res else { continue }`) y la inestabilidad
/// percibida en la lista era consecuencia directa.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub name: String,
    /// `true` si el provider devolvió resultados (aunque sean 0).
    /// `false` si hubo error de red / HTTP / timeout.
    pub ok: bool,
    /// Número de hits DEVUELTOS por el provider (antes de dedupe y
    /// filtros globales de `search_all`). NO es el número final que
    /// verá el user — para eso mira la longitud de `results`.
    pub hits: usize,
    /// Duración total del intento (incluye retry si lo hubo).
    pub elapsed_ms: u64,
    /// Descripción corta del fallo cuando `ok = false`. `None` en éxito.
    /// Se muestra tal cual al user (ej: `"timeout"`, `"HTTP 503"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `true` si el provider necesitó reintento (informativo).
    pub retried: bool,
    /// `true` si el resultado viene del caché en disco en vez de una
    /// consulta viva (Fase 4a del audit). Los callers pintan un badge
    /// distinto para diferenciar (`✓ 34` vs `↺ 34`).
    #[serde(default)]
    pub from_cache: bool,
}

/// Lanza un provider con timeout + reintento único (backoff 500 ms).
/// Devuelve `(hits, status)` — nunca falla en sí misma, el fallo
/// queda reflejado en `status.ok = false`.
///
/// Política de retry (Fase 1b):
/// * Timeout ⇒ NO retry (probablemente el provider está muerto o muy
///   lento; reintentar solo duplica el gasto de latencia).
/// * Error HTTP (`resp.status().is_success()` = false, propagado
///   como anyhow bail) ⇒ NO retry: el server ya nos dijo que no.
/// * Cualquier otro Err (parse JSON, socket, DNS, TLS) ⇒ retry único.
///
/// Distinguir HTTP status de errores transport es imposible fiable
/// solo mirando el anyhow (los providers hacen `bail!` con string).
/// Aproximación: si el mensaje de error contiene `"devolvi\u{f3}"`
/// o `"HTTP"` asumimos HTTP status. Feo pero pragmático — el crate
/// interno del ecosistema podría normalizarse en el futuro.
async fn run_provider(
    provider: Arc<dyn TorrentProvider>,
    http: reqwest::Client,
    query: MovieQuery,
) -> (Vec<Torrent>, ProviderStatus) {
    let name: String = provider.name().to_string();
    let start = Instant::now();
    let mut retried = false;

    let first = tokio::time::timeout(PROVIDER_TIMEOUT, provider.search(&http, &query)).await;
    let outcome = match first {
        Err(_elapsed) => {
            // Timeout — NO retry.
            return (
                Vec::new(),
                ProviderStatus {
                    name,
                    ok: false,
                    hits: 0,
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    error: Some("timeout".to_string()),
                    retried: false,
                    from_cache: false,
                },
            );
        }
        Ok(Ok(hits)) => Ok(hits),
        Ok(Err(err)) => {
            let msg = format!("{err:#}");
            let looks_like_http = msg.contains("devolvi\u{f3}") || msg.contains("HTTP");
            if looks_like_http {
                // El server ya respondió (mal). No reintentamos.
                return (
                    Vec::new(),
                    ProviderStatus {
                        name,
                        ok: false,
                        hits: 0,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                        error: Some(shorten_err(&msg)),
                        retried: false,
                        from_cache: false,
                    },
                );
            }
            // Transport error → retry único tras backoff corto.
            retried = true;
            tokio::time::sleep(PROVIDER_RETRY_BACKOFF).await;
            let second =
                tokio::time::timeout(PROVIDER_TIMEOUT, provider.search(&http, &query)).await;
            match second {
                Err(_) => Err("timeout (retry)".to_string()),
                Ok(Ok(hits)) => Ok(hits),
                Ok(Err(err2)) => Err(shorten_err(&format!("{err2:#}"))),
            }
        }
    };

    match outcome {
        Ok(hits) => {
            let n = hits.len();
            (
                hits,
                ProviderStatus {
                    name,
                    ok: true,
                    hits: n,
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    error: None,
                    retried,
                    from_cache: false,
                },
            )
        }
        Err(msg) => (
            Vec::new(),
            ProviderStatus {
                name,
                ok: false,
                hits: 0,
                elapsed_ms: start.elapsed().as_millis() as u64,
                error: Some(msg),
                retried,
                from_cache: false,
            },
        ),
    }
}

/// Recorta un mensaje de error largo (contexts anidados de anyhow)
/// para que quepa en un badge de UI. Coge la primera línea y trunca
/// a 60 chars (`char` count — safe con multibyte tipo cirílico).
fn shorten_err(msg: &str) -> String {
    let first = msg.lines().next().unwrap_or(msg);
    let mut out: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        out.push('…');
    }
    out
}

/// Compara dos títulos ya normalizados. Acepta:
///   * Igualdad estricta (el caso ideal).
///   * `candidate` empieza por `variant` seguido de separador (los
///     releases suelen añadir sufijos tipo `part-2`, `2`, etc. tras
///     el título — no queremos perder esos por rigor).
///   * `variant` empieza por `candidate` (título recortado en el
///     release — cubre casos como "Spider-Man" release para
///     "Spider-Man Brand New Day": aceptamos porque el año/imdb ya
///     filtra falsos positivos).
///
/// **NO** hace fuzzy matching (Levenshtein, tokens sueltos, etc.).
/// La red de precisión viene de tener variantes exhaustivas de TMDB
/// en el conjunto, no de matcheos laxos.
fn titles_match(variant: &str, candidate: &str) -> bool {
    if variant == candidate {
        return true;
    }
    if candidate.len() > variant.len()
        && candidate.starts_with(variant)
        && candidate.as_bytes().get(variant.len()) == Some(&b' ')
    {
        return true;
    }
    if variant.len() > candidate.len()
        && variant.starts_with(candidate)
        && variant.as_bytes().get(candidate.len()) == Some(&b' ')
    {
        return true;
    }
    false
}

/// Decide si un release matchea la query y con qué categoría
/// (`MatchKind`). Devuelve `None` para descartar; `Some(mk)` para
/// aceptar y etiquetar.
///
/// Reglas:
/// * `query.kind == Movie`:
///     - Rechaza cualquier serie (`parsed.is_tv()` o phrase pack
///       detectada por `is_tv_release`). Comportamiento pre-audit.
///     - Acepta lo demás como `MatchKind::Movie`.
/// * `query.kind == Series`:
///     - Con `episode` pedido: acepta si `parsed.season == query.season
///       && parsed.episode == query.episode` (Episode), o si el
///       release es season pack de esa temporada (SeasonPack), o
///       si es series pack (SeriesPack). Rechaza episodios de otra
///       temporada/episodio.
///     - Sin `episode` (buscando pack de temporada): acepta season
///       pack de la temporada pedida (SeasonPack) y series pack
///       (SeriesPack). Los episodios sueltos se rechazan — el user
///       pidió pack, no episodios sueltos.
///     - Sin `season` ni `episode` (serie entera): acepta season
///       pack de CUALQUIER temporada y series pack. Rechaza
///       episodios sueltos (no sabemos cuál quería).
///     - Películas del universo de la serie (parsed sin S/E) se
///       rechazan siempre — el filtro de título por variantes ya
///       los evita a nivel de query, pero por si algún homónimo
///       cuela, aquí se cae.
fn classify_match(
    query: &MovieQuery,
    parsed: &release_name::ParsedRelease,
    raw_title: &str,
) -> Option<MatchKind> {
    let is_series_pack_phrase =
        is_tv_release(raw_title) && parsed.season.is_none() && parsed.episode.is_none();

    match query.kind {
        crate::tmdb::MediaKind::Movie => {
            if parsed.is_tv() || is_tv_release(raw_title) {
                None
            } else {
                Some(MatchKind::Movie)
            }
        }
        crate::tmdb::MediaKind::Series => {
            match (query.season, query.episode) {
                // Episodio específico
                (Some(qs), Some(qe)) => match (parsed.season, parsed.episode) {
                    (Some(ps), Some(pe)) if ps == qs && pe == qe => Some(MatchKind::Episode),
                    (Some(ps), None) if ps == qs => Some(MatchKind::SeasonPack),
                    _ if is_series_pack_phrase => Some(MatchKind::SeriesPack),
                    _ => None,
                },
                // Pack de temporada
                (Some(qs), None) => match (parsed.season, parsed.episode) {
                    (Some(ps), None) if ps == qs => Some(MatchKind::SeasonPack),
                    _ if is_series_pack_phrase => Some(MatchKind::SeriesPack),
                    _ => None,
                },
                // Serie entera (sin season)
                (None, _) => match (parsed.season, parsed.episode) {
                    (Some(_), None) => Some(MatchKind::SeasonPack),
                    _ if is_series_pack_phrase => Some(MatchKind::SeriesPack),
                    _ => None,
                },
            }
        }
    }
}

/// Combina dos vectores de `ProviderStatus` colapsando por nombre.
/// Se usa cuando `search_all` se llama varias veces con distintos
/// títulos (ej: primary + english + russian en `search_torrents_by_tmdb`)
/// y queremos un único resumen consolidado para la UI.
///
/// Regla: para cada provider, gana el `ok = true` si aparece en
/// cualquier pasada (una respuesta buena tapa un fallo puntual). Si
/// varias pasadas dieron OK, sumamos los `hits` y nos quedamos con el
/// `elapsed_ms` máximo (para reflejar el coste total honesto). Los
/// providers exclusivos de una lista se propagan tal cual.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn merge_provider_statuses(
    mut base: Vec<ProviderStatus>,
    extra: Vec<ProviderStatus>,
) -> Vec<ProviderStatus> {
    for e in extra {
        if let Some(existing) = base.iter_mut().find(|s| s.name == e.name) {
            if e.ok {
                if existing.ok {
                    existing.hits = existing.hits.saturating_add(e.hits);
                } else {
                    // La nueva pasada rescató al provider: reemplaza.
                    existing.ok = true;
                    existing.hits = e.hits;
                    existing.error = None;
                }
                existing.retried = existing.retried || e.retried;
                existing.elapsed_ms = existing.elapsed_ms.max(e.elapsed_ms);
            } else if !existing.ok {
                // Ambos fallaron — nos quedamos con el mensaje de error
                // más reciente y sumamos elapsed.
                existing.error = e.error;
                existing.elapsed_ms = existing.elapsed_ms.max(e.elapsed_ms);
                existing.retried = existing.retried || e.retried;
            }
            // Si existing.ok == true y e.ok == false, nos quedamos con
            // el OK — no hacemos nada.
        } else {
            base.push(e);
        }
    }
    base.sort_by(|a, b| a.name.cmp(&b.name));
    base
}

/// Resultado global de `search_all`: torrents listos para mostrar
/// (ya deduplicados, filtrados y ordenados) + estado por provider
/// para telemetría / UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOutcome {
    pub results: Vec<Torrent>,
    pub providers: Vec<ProviderStatus>,
}

/// Consulta a todos los providers en paralelo, dedupe por infohash, filtra por
/// seeders mínimos y ordena por score descendente. Los errores individuales
/// no abortan: se registran como `ProviderStatus { ok: false, error }` y el
/// resto sigue funcionando. Cada provider tiene un presupuesto máximo de
/// `PROVIDER_TIMEOUT` (8s) y un reintento único con backoff `PROVIDER_RETRY_BACKOFF`
/// (500ms) para fallos de transporte (no para HTTP status).
pub async fn search_all(
    http: &reqwest::Client,
    providers: &[Arc<dyn TorrentProvider>],
    query: &MovieQuery,
    min_seeders: u32,
    limit: usize,
) -> SearchOutcome {
    let mut futs = FuturesUnordered::new();
    for p in providers {
        let p = Arc::clone(p);
        let http = http.clone();
        let query = query.clone();
        futs.push(async move { run_provider(p, http, query).await });
    }

    // Dedupe por infohash, quedándonos con la entrada de más seeders.
    // Se hace en el mismo loop que consume los futures — evita un `Vec`
    // intermedio que en búsquedas amplias (miles de resultados de Knaben)
    // dispara reallocaciones inútiles.
    //
    // Fase 2b — todos los filtros de matching viven ahora aquí, no
    // en los providers individuales. Pipeline por release:
    //   1. `release_name::parse` sobre el título → `ParsedRelease`
    //   2. `classify_match` decide contra `query.kind/season/episode`:
    //      * Movie: rechaza series (parsed.is_tv o phrase pack).
    //      * Series: acepta episodio exacto / season pack / series
    //        pack — con `MatchKind` etiquetado en el `Torrent`.
    //   3. Año: si el query trae `year`, exigimos `parsed.year` ±1.
    //      (Para episodios el año casi nunca aparece en el release;
    //      `parsed.year = None` se acepta automáticamente.)
    //   4. Título: si `title_variants` no-vacío, exigimos que
    //      `normalize_title(parsed.title)` matchee al menos una.
    //   5. Trash quality (CAM/TS/SCR) y tamaño absurdo — sin cambios.
    let variants_normalized: Vec<String> = query
        .title_variants
        .iter()
        .map(|v| release_name::normalize_title(v))
        .filter(|v| !v.is_empty())
        .collect();

    let mut best: HashMap<String, Torrent> = HashMap::new();
    let mut statuses: Vec<ProviderStatus> = Vec::with_capacity(providers.len());
    while let Some((items, status)) = futs.next().await {
        statuses.push(status);
        for mut t in items {
            if t.infohash.is_empty() || t.seeders < min_seeders {
                continue;
            }
            let parsed = release_name::parse(&t.title);
            // ── Match por kind (Fase 2a — audit series)
            let Some(mk) = classify_match(query, &parsed, &t.title) else {
                continue;
            };
            t.match_kind = mk;
            // ── Year filter (parsed vs query, tolerancia ±1)
            if let (Some(target), Some(got)) = (query.year, parsed.year) {
                if (target as i32 - got as i32).unsigned_abs() > 1 {
                    continue;
                }
            }
            // ── Title match contra variantes (si el caller pasó).
            // Un release cuyo `parsed.title` normalizado NO matchea
            // ninguna variante se descarta — arregla el problema
            // histórico de homónimos y películas distintas que se
            // colaban por token-overlap laxo.
            if !variants_normalized.is_empty() {
                let candidate = release_name::normalize_title(&parsed.title);
                if candidate.is_empty()
                    || !variants_normalized
                        .iter()
                        .any(|v| titles_match(v, &candidate))
                {
                    continue;
                }
            }
            if is_trash_quality(&t.title) {
                continue;
            }
            if is_absurd_size(t.size_bytes) {
                continue;
            }
            match best.get_mut(&t.infohash) {
                Some(prev) if prev.seeders < t.seeders => *prev = t,
                Some(_) => {}
                None => {
                    best.insert(t.infohash.clone(), t);
                }
            }
        }
    }

    let mut out: Vec<Torrent> = best.into_values().collect();
    let orig_lang = query.original_language.as_deref();
    out.sort_by(|a, b| {
        score(a, orig_lang)
            .partial_cmp(&score(b, orig_lang))
            .map(|o| o.reverse())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(limit);
    // Orden estable de statuses por nombre para que la UI no cambie
    // el orden entre búsquedas (FuturesUnordered los devuelve en
    // orden de completion).
    statuses.sort_by(|a, b| a.name.cmp(&b.name));
    SearchOutcome {
        results: out,
        providers: statuses,
    }
}

/// score = seeders * peso_calidad * peso_idioma * peso_match_kind.
///
/// El multiplicador de `match_kind` (Fase 2a — audit series) pondera
/// preferencia por episodios sueltos sobre packs sin romper la
/// dominancia por seeders: un pack con 500 seeders sigue ganando a
/// un episodio con 3, pero con seeders iguales el episodio se
/// prefiere. Para películas es siempre 1.0 (no hay distinción).
fn score(t: &Torrent, original_language: Option<&str>) -> f64 {
    let q_weight = match t.quality.as_deref() {
        Some(q) if q.contains("2160") || q.eq_ignore_ascii_case("4k") => 1.00,
        Some(q) if q.contains("1080") => 0.90,
        Some(q) if q.contains("720") => 0.60,
        Some(_) => 0.35,
        None => 0.50,
    };
    let hint = classify_audio(&t.title, original_language);
    let lang_weight = language_multiplier(hint);
    let m_weight = match_kind_multiplier(t.match_kind);
    (t.seeders as f64) * q_weight * lang_weight * m_weight
}

/// Peso del `MatchKind` en el score. Episodios exactos ganan a
/// season packs, y estos a series packs. Movie (compat) queda en
/// 1.0 para no alterar el score de películas.
fn match_kind_multiplier(mk: MatchKind) -> f64 {
    match mk {
        MatchKind::Movie => 1.00,
        MatchKind::Episode => 1.00,
        MatchKind::SeasonPack => 0.80,
        MatchKind::SeriesPack => 0.50,
    }
}

/// Peso de idioma en el score. `Original` y `Multi` son deseables (audio
/// original disponible); los doblajes se castigan para que no dominen el
/// ranking. `Unknown` queda en medio (no penaliza fuerte porque muchos
/// releases scene no marcan idioma en el título).
fn language_multiplier(hint: AudioHint) -> f64 {
    match hint {
        AudioHint::Original => 1.00,
        AudioHint::Multi => 0.90,
        AudioHint::Unknown => 0.55,
        AudioHint::Dubbed(_) => 0.25,
    }
}

/// Devuelve los providers habilitados por defecto. Torznab se activa si están
/// definidas `TORZNAB_URL` y `TORZNAB_APIKEY` en el entorno.
pub fn default_providers() -> Vec<Arc<dyn TorrentProvider>> {
    let mut providers: Vec<Arc<dyn TorrentProvider>> = vec![
        Arc::new(yts::Yts),
        Arc::new(knaben::Knaben),
        Arc::new(apibay::Apibay),
    ];

    if let (Ok(url), Ok(key)) = (
        std::env::var("TORZNAB_URL"),
        std::env::var("TORZNAB_APIKEY"),
    ) {
        providers.push(Arc::new(torznab::Torznab::new(url, key)));
    }

    providers
}

// ── Helpers públicos para los providers ─────────────────────────────────────

/// Extrae el infohash (`xt=urn:btih:...`) de un magnet link. Acepta
/// hex de 40 chars o base32 de 32 chars y valida el formato; devuelve
/// `None` si el magnet no lo tiene bien formado.
///
/// El resultado sale en MINÚSCULAS (convención BitTorrent). Callers
/// que necesiten otro case aplican `.to_ascii_uppercase()`
/// explícitamente. Fuente única de verdad — tanto el cache de disco
/// de streams (`<cache>/streams/<hash>/`) como el dedupe de resultados
/// en providers dependen de esta normalización.
pub fn parse_infohash(magnet: &str) -> Option<String> {
    let rest = magnet.strip_prefix("magnet:?")?;
    for pair in rest.split('&') {
        let Some(v) = pair.strip_prefix("xt=urn:btih:") else {
            continue;
        };
        // El valor puede tener params extra pegados en URLs mal
        // formateados; recortamos hasta el primer separador raro.
        let raw = v.split(&['&', '?'][..]).next().unwrap_or("");
        let hash = raw.to_ascii_lowercase();
        let is_hex40 = hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit());
        let is_b32 = hash.len() == 32 && hash.chars().all(|c| c.is_ascii_alphanumeric());
        if is_hex40 || is_b32 {
            return Some(hash);
        }
    }
    None
}

/// Igual que `parse_infohash` pero devuelve `""` cuando no hay hash
/// válido y normaliza a MAYÚSCULAS. Convención histórica de los
/// providers (`Torrent.infohash` se compara siempre en uppercase para
/// deduplicar entre providers). Nuevo código en el player/stream usa
/// `parse_infohash` directamente.
pub fn infohash_from_magnet(magnet: &str) -> String {
    parse_infohash(magnet)
        .map(|h| h.to_ascii_uppercase())
        .unwrap_or_default()
}

/// Detecta calidad a partir del título del release.
pub fn quality_from_title(title: &str) -> Option<String> {
    let t = title.to_ascii_lowercase();
    for q in ["2160p", "1080p", "720p", "480p"] {
        if t.contains(q) {
            return Some(q.to_string());
        }
    }
    if t.contains("4k") {
        return Some("2160p".to_string());
    }
    None
}

/// Construye un magnet estándar a partir de un infohash y un display name.
pub fn build_magnet(infohash: &str, name: &str) -> String {
    const TRACKERS: &[&str] = &[
        "udp://tracker.opentrackr.org:1337/announce",
        "udp://tracker.openbittorrent.com:6969/announce",
        "udp://open.stealth.si:80/announce",
        "udp://exodus.desync.com:6969/announce",
    ];
    let mut m = format!(
        "magnet:?xt=urn:btih:{}&dn={}",
        infohash,
        urlencoding::encode(name)
    );
    for tr in TRACKERS {
        m.push_str("&tr=");
        m.push_str(&urlencoding::encode(tr));
    }
    m
}

/// Formato humano para bytes: "12.4 GB", "540 MB", "1.2 TB".
pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

// ── Detección de idioma de audio (heurística sobre el título) ───────────────

/// Pista sobre el audio de un release. Heurística basada en tokens habituales
/// del scene/P2P — no es 100% fiable pero acierta en la mayoría de casos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AudioHint {
    /// Muy probable audio original (idioma coincide con el de rodaje).
    Original,
    /// Doblado a un idioma concreto (ISO 639-1 aproximado).
    Dubbed(&'static str),
    /// Release con múltiples pistas de audio (incluye probablemente original).
    Multi,
    /// No hay pistas suficientes en el título.
    Unknown,
}

impl AudioHint {
    /// Etiqueta corta para UI (max 8 chars).
    pub fn badge(&self) -> &'static str {
        match self {
            AudioHint::Original => "orig",
            AudioHint::Dubbed("ru") => "dub-ru",
            AudioHint::Dubbed("es") => "dub-es",
            AudioHint::Dubbed("fr") => "dub-fr",
            AudioHint::Dubbed("it") => "dub-it",
            AudioHint::Dubbed("de") => "dub-de",
            AudioHint::Dubbed(_) => "dub",
            AudioHint::Multi => "multi",
            AudioHint::Unknown => "?",
        }
    }
}

/// Clasifica el audio de un release a partir de su título y del idioma
/// original de la película (del `original_language` de TMDB).
///
/// Reglas clave:
/// * Si el título tiene marcadores multi-audio explícitos (MULTI, dual,
///   `[EN+RUS]`…) → `Multi`. `WEB-DL` y variantes NO cuentan como
///   multi-audio: es un marcador de fuente, no de idioma.
/// * Si el título lleva un idioma detectable, se compara con
///   `original_language`: si coincide es `Original`, si difiere es
///   `Dubbed(iso)`. Esto evita castigar releases castellanos de
///   películas españolas, italianos de películas italianas, etc.
/// * Cirílico en el título se trata como pista de idioma ruso.
/// * Si no aparece ningún marcador y el release no lleva cirílico, se
///   asume audio original (default del scene internacional).
pub fn classify_audio(title: &str, original_language: Option<&str>) -> AudioHint {
    let t = title.to_lowercase();
    let has_cyrillic = title
        .chars()
        .any(|c| ('\u{0400}'..='\u{04FF}').contains(&c));
    let ol = original_language
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    // Multi-audio explícito. NOTA: NO usamos `.dl.` / ` dl ` como se
    // hacía antes: matcheaba WEB.DL (variante extendida de WEB-DL) y
    // marcaba todos esos releases como multi. Si hace falta detectar
    // German Dual, usa marcadores explícitos (`dual`, `.multi.`, etc.).
    if t.contains("multi")
        || t.contains("dual audio")
        || t.contains("dual-audio")
        || t.contains("dualaudio")
        || t.contains(" da2 ")
        || t.contains(" 2audio")
        || multi_language_bracket(&t)
    {
        return AudioHint::Multi;
    }

    // Detectamos el idioma de audio con la primera regla que matchee.
    // Si coincide con el idioma original de la peli → `Original`; si no
    // → `Dubbed(iso)`. Esto arregla el bug histórico de castigar
    // releases castellanos de pelis españolas.
    let detected: Option<&'static str> = if has_cyrillic {
        Some("ru")
    } else if t.contains("castellano")
        || t.contains("espanol")
        || t.contains("español")
        || t.contains("spanish")
        || t.contains(" esp ")
        || t.contains("[esp]")
        || t.contains("latino")
    {
        Some("es")
    } else if t.contains(" ita ") || t.contains("italian") {
        Some("it")
    } else if t.contains(" fra ") || t.contains("french") {
        Some("fr")
    } else if t.contains(" ger ") || t.contains("german") || t.contains("deutsch") {
        Some("de")
    } else {
        None
    };

    if let Some(iso) = detected {
        return if ol == iso {
            AudioHint::Original
        } else {
            AudioHint::Dubbed(iso)
        };
    }

    // Marcador genérico "dub" sin idioma identificado.
    if t.contains(" dub") || t.contains(".dub.") || t.ends_with(" dub") {
        return AudioHint::Dubbed("??");
    }

    // Sin marcadores: asumimos audio original. En releases scene
    // internacionales el default es "idioma original de la peli".
    AudioHint::Original
}

/// Detecta patrones tipo `[ENG+RUS]`, `[EN.RU.ES]`, `[EN/FR]` en el título:
/// dos o más códigos de idioma ISO 639-1/-2 dentro del mismo bracket o
/// grupo entre puntos.
///
/// Exige que cada código tenga *frontera de palabra* delante y detrás
/// (separador o borde de grupo) para no contar coincidencias falsas como
/// `en` dentro de `Golden`. Trabaja sobre bytes ASCII sin re-allocar.
fn multi_language_bracket(t: &str) -> bool {
    // Nota: `t` viene ya en minúsculas del caller. No re-lowercase.
    const LANG_CODES: &[&str] = &[
        "eng", "en", "rus", "ru", "esp", "spa", "es", "fre", "fra", "fr", "ita", "it", "ger",
        "deu", "de", "por", "pt", "jpn", "ja", "chi", "zh", "kor", "ko",
    ];
    let bytes = t.as_bytes();
    let mut in_group = false;
    let mut count = 0u8;
    let mut prev_is_sep = true;

    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch == b'[' || ch == b'(' {
            in_group = true;
            count = 0;
            prev_is_sep = true;
            i += 1;
            continue;
        }
        if ch == b']' || ch == b')' {
            if count >= 2 {
                return true;
            }
            in_group = false;
            prev_is_sep = true;
            i += 1;
            continue;
        }
        if !in_group {
            prev_is_sep = !ch.is_ascii_alphabetic();
            i += 1;
            continue;
        }
        // Dentro de un grupo: intentamos matchear un código de idioma
        // solo si venimos de un separador y este byte es alfabético
        // (para no partir palabras como "Golden").
        let is_alpha = ch.is_ascii_alphabetic();
        let mut advance = 1usize;
        if prev_is_sep && is_alpha {
            for code in LANG_CODES {
                let end = i + code.len();
                if end <= bytes.len() && &bytes[i..end] == code.as_bytes() {
                    let after = bytes.get(end).copied().unwrap_or(b' ');
                    if !after.is_ascii_alphabetic() {
                        count = count.saturating_add(1);
                        advance = code.len();
                        break;
                    }
                }
            }
        }
        // Si consumimos un código completo, el byte que sigue es
        // separador por construcción (lo verificamos arriba con `after`).
        prev_is_sep = if advance == 1 { !is_alpha } else { true };
        i += advance;
    }
    false
}

// ---- Helpers compartidos entre providers / vistas ----

/// Si `s` acaba en un año de 4 dígitos (1888-2100) separado por espacio,
/// devuelve `(título_sin_año, Some(año))`. Si no, `(s, None)`.
///
/// Safe para entradas no-ASCII: usa `rfind(' ')` en lugar de `split_at`
/// por bytes (la variante anterior paniqueaba con títulos cirílicos como
/// "Амели" cuando el offset caía en mitad de un char multibyte).
pub fn split_trailing_year(s: &str) -> (String, Option<u16>) {
    let s = s.trim();
    if let Some(idx) = s.rfind(' ') {
        let tail = &s[idx + 1..];
        if tail.len() == 4 {
            if let Ok(y) = tail.parse::<u16>() {
                if (1888..=2100).contains(&y) {
                    return (s[..idx].trim().to_string(), Some(y));
                }
            }
        }
    }
    (s.to_string(), None)
}

/// Comprueba si el título de un release EMPIEZA con `needle` (case-
/// insensitive). Ignora caracteres no alfanuméricos al principio de
/// ambos lados (comillas, corchetes, guiones…). Usado por el fallback
/// ruso para descartar releases que solo *mencionan* el título en su
/// descripción en lugar de empezar por él.
pub fn release_starts_with(release: &str, needle: &str) -> bool {
    let release = release.to_lowercase();
    let release = release.trim_start_matches(|c: char| !c.is_alphanumeric());
    let needle = needle.to_lowercase();
    let needle = needle.trim_start_matches(|c: char| !c.is_alphanumeric());
    release.starts_with(needle.trim())
}

/// Extrae años (1900-2099) del título del release y comprueba si alguno
/// está dentro de ±`tolerance` del año buscado. Si el release no incluye
/// ningún año, se acepta (no podemos discriminar y es preferible un
/// falso positivo a perder el hit).
///
/// **Legacy** (Fase 2b): esta comparación vive ahora en `search_all`
/// vía `release_name::ParsedRelease.year`. Se mantiene por los tests
/// que fijan la conducta histórica y como fallback si alguien
/// quiere aplicarla desde fuera del pipeline (main.rs / futuros
/// providers). No la borres sin borrar también los tests.
#[allow(dead_code)]
pub fn release_matches_year(title: &str, target: u16, tolerance: u16) -> bool {
    let mut has_year = false;
    for token in title.split(|c: char| !c.is_alphanumeric()) {
        if token.len() != 4 {
            continue;
        }
        if let Ok(y) = token.parse::<u16>() {
            if (1900..=2099).contains(&y) {
                has_year = true;
                if (target as i32 - y as i32).unsigned_abs() as u16 <= tolerance {
                    return true;
                }
            }
        }
    }
    !has_year
}

// ── Filtros anti-basura por título / tamaño ────────────────────────────────

/// Detecta si el título del release corresponde a una serie de TV
/// (temporada completa, episodio suelto, mini-serie). Los TV packs se
/// cuelan mucho en Knaben cuando la película tiene un título común, y
/// no aportan nada al flujo "ver una peli".
///
/// Reglas (todas case-insensitive):
///
///   * `SxxEyy` estilo scene (`S01E03`, `S1E3`, `S01E01-E10`).
///   * `Season N` / `Temporada N` / `Complete Season`.
///   * `Complete Series` / `The Complete Collection` (packs).
///   * `Mini-Series` / `Miniseries` / `Limited Series`.
///
/// Deliberadamente permisivo con "Series" a secas — hay pelis con
/// "series" en el título (`Twilight Series`) que sí queremos ver.
pub fn is_tv_release(title: &str) -> bool {
    let t = title.to_lowercase();
    let bytes = t.as_bytes();

    // Patrón scene SxxEyy: busca `s\d+e\d+`.
    for (i, w) in bytes.windows(2).enumerate() {
        if w[0] == b's' && w[1].is_ascii_digit() {
            // Skip dígitos de la temporada.
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < bytes.len()
                && bytes[j] == b'e'
                && j + 1 < bytes.len()
                && bytes[j + 1].is_ascii_digit()
            {
                return true;
            }
        }
    }

    // Frases explícitas de packs de series.
    let bad_phrases: &[&str] = &[
        "complete series",
        "complete season",
        "the complete collection",
        "mini-series",
        "mini series",
        "miniseries",
        "limited series",
        "season pack",
        " temporada ",
    ];
    for p in bad_phrases {
        if t.contains(p) {
            return true;
        }
    }
    // `Season 1` / `Season 12` como token independiente (no como parte de
    // "Season of the Witch" que es una peli).
    if let Some(idx) = t.find("season ") {
        let rest = &t[idx + "season ".len()..];
        if rest
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .next()
            .is_some()
        {
            return true;
        }
    }
    false
}

/// Detecta releases de calidad "basura" (cammed en cine, screener
/// pirateado, transferencia analógica). Estos releases se filtran
/// SIEMPRE — el user pedirá algo mejor o esperará; nunca queremos
/// servir una versión cammed cuando escoge un torrent por defecto.
///
/// Palabras que buscamos como tokens (rodeadas por separadores scene
/// `. - _` o espacios) para evitar falsos positivos dentro del
/// título de la peli (ej. `Trans` no debe activar `TS`).
pub fn is_trash_quality(title: &str) -> bool {
    // Tokens en mayúsculas exactamente como aparecen en releases scene.
    // Case-sensitive matching sobre versión original: `TS.` es TS
    // (Telesync), `ts` minúscula podría ser cualquier cosa.
    const TRASH_TOKENS: &[&str] = &[
        "CAM",
        "HDCAM",
        "HDCam",
        "CamRip",
        "CAMRip",
        "TS",
        "HDTS",
        "TELESYNC",
        "Telesync",
        "TC",
        "HDTC",
        "TELECINE",
        "Telecine",
        "PDVD",
        "PreDVD",
        "SCR",
        "DVDSCR",
        "SCREENER",
        "Screener",
        "KORSUB",
        "Korsub",
        "WORKPRINT",
        "Workprint",
    ];
    // Splitea por separadores scene comunes.
    for tok in title.split(|c: char| c == '.' || c == '-' || c == '_' || c.is_whitespace()) {
        if TRASH_TOKENS.contains(&tok) {
            return true;
        }
    }
    false
}

/// Filtra tamaños obviamente absurdos para una película. Los packs de
/// muchas pelis o de series pasan de 100GB; un `.txt` decoy o un
/// magnet roto marcan 0/pocos bytes.
///
/// Rango: 80MB (compresión brutal a SD por debajo de eso ya no es
/// película) a 100GB (por encima suele ser BluRay REMUX de una peli
/// concreta, aún aceptable, así que el techo es un poco alto).
pub fn is_absurd_size(bytes: u64) -> bool {
    const MIN_MOVIE_BYTES: u64 = 80 * 1024 * 1024;
    const MAX_MOVIE_BYTES: u64 = 100 * 1024 * 1024 * 1024;
    !(MIN_MOVIE_BYTES..=MAX_MOVIE_BYTES).contains(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_infohash_accepts_hex40() {
        let h = parse_infohash("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x")
            .unwrap();
        assert_eq!(h.len(), 40);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        // Normalizado a minúsculas para que el cache path sea estable.
        assert_eq!(h, h.to_ascii_lowercase());
    }

    #[test]
    fn parse_infohash_normalizes_case() {
        let a =
            parse_infohash("magnet:?xt=urn:btih:ABCDEF0123456789ABCDEF0123456789ABCDEF01").unwrap();
        let b =
            parse_infohash("magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parse_infohash_accepts_base32() {
        let h = parse_infohash("magnet:?xt=urn:btih:ABCDEFGHIJKLMNOPQRSTUVWXYZ234567").unwrap();
        assert_eq!(h.len(), 32);
    }

    #[test]
    fn parse_infohash_rejects_garbage() {
        assert!(parse_infohash("magnet:?xt=urn:btih:notahash").is_none());
        assert!(parse_infohash("magnet:?xt=urn:btih:").is_none());
        assert!(parse_infohash("not-a-magnet").is_none());
    }

    #[test]
    fn infohash_from_magnet_is_uppercase() {
        let h =
            infohash_from_magnet("magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01");
        assert_eq!(h, h.to_ascii_uppercase());
        assert_eq!(h.len(), 40);
    }

    #[test]
    fn infohash_from_magnet_empty_on_invalid() {
        assert_eq!(infohash_from_magnet("magnet:?xt=urn:btih:short"), "");
    }

    // ── Red de seguridad para las heurísticas de matching ─────────────
    //
    // Estos tests fijan la conducta actual de los tres filtros de
    // título (tv, año, overlap) ANTES del refactor a parser
    // estructurado (`release_name.rs`). Cualquier regresión en el
    // refactor debe hacer fallar aquí y quedar visible antes de
    // llegar a producción.
    //
    // Casos cubiertos por diseño del audit:
    //   - homónimo peli vs serie ("Season of the Witch" vs S01E01)
    //   - título corto que hoy pasa por overlap laxo
    //   - año dentro del título (Blade Runner 2049 → año release 2017)
    //   - cirílico (títulos rusos no deben marcarse como TV por
    //     "sezon" cirílico — no lo detectamos, es aceptable ahora)

    #[test]
    fn is_tv_release_catches_scene_sxxexx() {
        assert!(is_tv_release("The.Office.US.S03E12.720p.HDTV.x264-LOL"));
        assert!(is_tv_release("Breaking.Bad.S05E14.1080p.WEB-DL"));
        assert!(is_tv_release("Peaky.Blinders.S1E1.720p"));
    }

    #[test]
    fn is_tv_release_catches_season_packs() {
        assert!(is_tv_release("The Wire Season 3 Complete 1080p"));
        assert!(is_tv_release("Friends Complete Series 720p x264"));
        assert!(is_tv_release("Chernobyl Mini-Series 2019 1080p"));
        assert!(is_tv_release("Vikings Temporada 3 HDTV"));
        // "The Complete Collection" con artículo. "Band of Brothers
        // Complete Collection" (sin `the`) NO se detecta — gap
        // conocido de la heurística; el parser estructurado de Fase 2
        // lo cubrirá vía `season/episode`.
        assert!(is_tv_release("Sopranos The Complete Collection 1080p"));
    }

    #[test]
    fn is_tv_release_does_not_catch_movies_with_season_in_title() {
        // Falsos positivos históricos: pelis cuyo título contiene
        // palabras confundibles con marcadores de serie.
        assert!(!is_tv_release("Season of the Witch 2011 1080p BluRay"));
        assert!(!is_tv_release("Seven Samurai 1954 1080p"));
        assert!(!is_tv_release("The Twilight Series Collection 1080p"));
    }

    #[test]
    fn release_matches_year_accepts_target_and_neighbors() {
        assert!(release_matches_year(
            "Funny.Games.2007.1080p.BluRay.x264",
            2008,
            1
        ));
        assert!(release_matches_year(
            "Funny.Games.2008.1080p.BluRay.x264",
            2008,
            1
        ));
        assert!(!release_matches_year(
            "Funny.Games.2005.1080p.BluRay.x264",
            2008,
            1
        ));
    }

    #[test]
    fn release_matches_year_accepts_when_no_year_present() {
        // Sin año en el título → aceptamos (mejor falso positivo que
        // perder hits reales; el año lo confirma después el infohash).
        assert!(release_matches_year("Some.Random.Release.1080p", 2020, 1));
    }

    #[test]
    fn release_matches_year_year_in_title_also_matches() {
        // Blade Runner 2049 estrenada en 2017 — los releases suelen
        // llevar los DOS años, y basta con que UNO de ellos matchee.
        // NOTA: hoy la heurística acierta este caso porque "2049"
        // está fuera de rango [1900,2099] cuando target=2017 tol=1
        // (2049 fuera). Cambia si el año release cae DENTRO del rango.
        assert!(release_matches_year(
            "Blade.Runner.2049.2017.1080p.UHD",
            2017,
            1
        ));
        // Espacio horrible: solo el año-de-título dentro del rango
        // matchea → falso positivo aceptado por diseño (menos peor
        // que perder hits). Este test lo documenta.
        assert!(release_matches_year("2001.A.Space.Odyssey.1968", 1968, 1));
    }

    #[test]
    fn is_trash_quality_catches_cam_screener() {
        assert!(is_trash_quality("Some.Movie.2024.CAM.x264-GRP"));
        assert!(is_trash_quality("Some.Movie.2024.HDCAM.x264"));
        assert!(is_trash_quality("Some.Movie.2024.TS.x264"));
        assert!(is_trash_quality("Some.Movie.2024.DVDSCR.x264"));
    }

    #[test]
    fn is_trash_quality_does_not_catch_words_containing_ts() {
        // `Trans` no debe activar TS, `Postscript` no debe activar SCR.
        assert!(!is_trash_quality("Transformers.2007.1080p.BluRay"));
        assert!(!is_trash_quality("The.Postscript.2020.1080p"));
    }

    #[test]
    fn is_absurd_size_boundaries() {
        assert!(is_absurd_size(0));
        assert!(is_absurd_size(50 * 1024 * 1024)); // 50 MB → basura
        assert!(!is_absurd_size(500 * 1024 * 1024)); // 500 MB → OK
        assert!(!is_absurd_size(15 * 1024 * 1024 * 1024)); // 15 GB → OK
        assert!(is_absurd_size(200 * 1024 * 1024 * 1024)); // 200 GB → pack
    }

    #[test]
    fn split_trailing_year_handles_unicode() {
        // Regression: la versión byte-based paniqueaba con multibyte.
        let (t, y) = split_trailing_year("Амели 2001");
        assert_eq!(t, "Амели");
        assert_eq!(y, Some(2001));
    }

    #[test]
    fn classify_audio_respects_original_language() {
        // Peli española con audio "castellano" → Original, no Dubbed.
        assert_eq!(
            classify_audio("El.Reino.2018.Castellano.1080p", Some("es")),
            AudioHint::Original
        );
        // Peli inglesa con audio "castellano" → Dubbed(es).
        assert_eq!(
            classify_audio("The.Matrix.1999.Castellano.1080p", Some("en")),
            AudioHint::Dubbed("es")
        );
    }

    #[test]
    fn classify_audio_cyrillic_is_russian() {
        // Título en cirílico → detecta audio ruso.
        assert_eq!(
            classify_audio("Брат 1997 BDRip 1080p", Some("ru")),
            AudioHint::Original
        );
        assert_eq!(
            classify_audio("Брат 1997 BDRip 1080p", Some("en")),
            AudioHint::Dubbed("ru")
        );
    }

    // ── Fase 2a audit series: matching por kind ────────────────────────

    fn mq_movie() -> MovieQuery {
        MovieQuery {
            kind: crate::tmdb::MediaKind::Movie,
            title: "Fargo".to_string(),
            ..MovieQuery::default()
        }
    }

    fn mq_series_episode(s: u16, e: u16) -> MovieQuery {
        MovieQuery {
            kind: crate::tmdb::MediaKind::Series,
            title: "Fargo".to_string(),
            season: Some(s),
            episode: Some(e),
            ..MovieQuery::default()
        }
    }

    fn mq_series_season_pack(s: u16) -> MovieQuery {
        MovieQuery {
            kind: crate::tmdb::MediaKind::Series,
            title: "Fargo".to_string(),
            season: Some(s),
            episode: None,
            ..MovieQuery::default()
        }
    }

    #[test]
    fn movie_query_rejects_tv_releases() {
        // Fargo la peli (1996) vs Fargo la serie: si buscamos peli,
        // el episodio S02E03 SE RECHAZA aunque el título matchee.
        let q = mq_movie();
        let p = release_name::parse("Fargo.S02E03.1080p.WEB-DL.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.S02E03.1080p.WEB-DL.x264-GRP"),
            None
        );
    }

    #[test]
    fn movie_query_accepts_movie_release() {
        let q = mq_movie();
        let p = release_name::parse("Fargo.1996.1080p.BluRay.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.1996.1080p.BluRay.x264-GRP"),
            Some(MatchKind::Movie)
        );
    }

    #[test]
    fn series_episode_query_matches_exact_episode() {
        let q = mq_series_episode(2, 3);
        let p = release_name::parse("Fargo.S02E03.1080p.WEB-DL.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.S02E03.1080p.WEB-DL.x264-GRP"),
            Some(MatchKind::Episode)
        );
    }

    #[test]
    fn series_episode_query_accepts_season_pack_as_container() {
        // Pidiendo S02E03 y encontrando un pack de S02 completa: OK,
        // ese pack contiene el episodio (se elegirá el file dentro
        // del pack en Fase 3).
        let q = mq_series_episode(2, 3);
        let p = release_name::parse("Fargo.S02.1080p.WEB-DL.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.S02.1080p.WEB-DL.x264-GRP"),
            Some(MatchKind::SeasonPack)
        );
    }

    #[test]
    fn series_episode_query_rejects_other_season_episode() {
        // Pidiendo S02E03, un release S03E01 NO cuenta.
        let q = mq_series_episode(2, 3);
        let p = release_name::parse("Fargo.S03E01.1080p.WEB-DL.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.S03E01.1080p.WEB-DL.x264-GRP"),
            None
        );
    }

    #[test]
    fn series_episode_query_rejects_other_season_pack() {
        let q = mq_series_episode(2, 3);
        let p = release_name::parse("Fargo.S01.1080p.WEB-DL.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.S01.1080p.WEB-DL.x264-GRP"),
            None
        );
    }

    #[test]
    fn series_episode_query_accepts_complete_series_pack() {
        let q = mq_series_episode(2, 3);
        let p = release_name::parse("Fargo.Complete.Series.1080p.WEB-DL.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo Complete Series 1080p WEB-DL x264-GRP"),
            Some(MatchKind::SeriesPack)
        );
    }

    #[test]
    fn series_season_pack_query_rejects_single_episodes() {
        // Si el user pide pack, no le sirve un episodio suelto — no
        // le vamos a preguntar cuál del pack quería, ya lo dijo
        // (todos).
        let q = mq_series_season_pack(2);
        let p = release_name::parse("Fargo.S02E03.1080p.WEB-DL.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.S02E03.1080p.WEB-DL.x264-GRP"),
            None
        );
    }

    #[test]
    fn series_season_pack_query_matches_pack() {
        let q = mq_series_season_pack(2);
        let p = release_name::parse("Fargo.S02.1080p.WEB-DL.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.S02.1080p.WEB-DL.x264-GRP"),
            Some(MatchKind::SeasonPack)
        );
    }

    #[test]
    fn series_query_rejects_movie_homonym() {
        // "Fargo 1996" (peli) NO debe matchear la query de serie.
        // parsed.is_tv() = false, is_series_pack_phrase = false → None.
        let q = mq_series_episode(2, 3);
        let p = release_name::parse("Fargo.1996.1080p.BluRay.x264-GRP");
        assert_eq!(
            classify_match(&q, &p, "Fargo.1996.1080p.BluRay.x264-GRP"),
            None
        );
    }
}
