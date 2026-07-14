//! Búsqueda de torrents para películas.
//!
//! Define un trait `TorrentProvider` con implementaciones para varias fuentes
//! (YTS, Knaben, Torznab). `search_all` las consulta en paralelo, dedupe por
//! infohash y ordena por seeders × calidad.

use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

pub mod knaben;
pub mod torznab;
pub mod yts;

#[derive(Debug, Clone, Serialize)]
pub struct Torrent {
    pub title: String,
    pub magnet: String,
    pub size_bytes: u64,
    pub seeders: u32,
    pub leechers: u32,
    pub quality: Option<String>,
    pub source: &'static str,
    /// Infohash extraído del magnet (para dedupe). No se serializa al JSON
    /// para no ensuciar la salida.
    #[serde(skip)]
    pub infohash: String,
}

#[derive(Debug, Clone, Default)]
pub struct MovieQuery {
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
}

impl MovieQuery {
    /// Cadena de búsqueda por defecto (para providers que no soportan IDs).
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

/// Consulta a todos los providers en paralelo, dedupe por infohash, filtra por
/// seeders mínimos y ordena por score descendente. Los errores individuales
/// no abortan: se registran como warnings pero no rompen la búsqueda global.
pub async fn search_all(
    http: &reqwest::Client,
    providers: &[Arc<dyn TorrentProvider>],
    query: &MovieQuery,
    min_seeders: u32,
    limit: usize,
) -> Vec<Torrent> {
    let mut futs = FuturesUnordered::new();
    for p in providers {
        let p = Arc::clone(p);
        let http = http.clone();
        let query = query.clone();
        futs.push(async move {
            let name = p.name();
            let res = p.search(&http, &query).await;
            (name, res)
        });
    }

    // Dedupe por infohash, quedándonos con la entrada de más seeders.
    // Se hace en el mismo loop que consume los futures — evita un `Vec`
    // intermedio que en búsquedas amplias (miles de resultados de Knaben)
    // dispara reallocaciones inútiles.
    let mut best: HashMap<String, Torrent> = HashMap::new();
    while let Some((_name, res)) = futs.next().await {
        // Silenciamos errores individuales: si un provider está caído
        // (YTS a menudo, un Torznab local mal configurado, etc.) el
        // resto sigue funcionando. En la TUI no podemos hacer eprintln
        // porque corromperíamos la pantalla alternativa.
        let Ok(items) = res else { continue };
        for t in items {
            if t.infohash.is_empty() || t.seeders < min_seeders {
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
    out
}

/// score = seeders * peso_calidad * peso_idioma.
/// Prioriza calidad razonable sin descartar releases con muchos seeders
/// aunque sean 720p/SD, y ANTEPONE audio original / multi a los doblajes
/// (los rusos de RuTracker son numerosos y saturan la lista si no se
/// castigan).
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
    (t.seeders as f64) * q_weight * lang_weight
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
    let mut providers: Vec<Arc<dyn TorrentProvider>> =
        vec![Arc::new(yts::Yts), Arc::new(knaben::Knaben)];

    if let (Ok(url), Ok(key)) = (
        std::env::var("TORZNAB_URL"),
        std::env::var("TORZNAB_APIKEY"),
    ) {
        providers.push(Arc::new(torznab::Torznab::new(url, key)));
    }

    providers
}

// ── Helpers públicos para los providers ─────────────────────────────────────

/// Extrae el infohash de un magnet link. Soporta btih hex y base32.
pub fn infohash_from_magnet(magnet: &str) -> String {
    // Formato típico: magnet:?xt=urn:btih:<HASH>&...
    magnet
        .split(&['?', '&'][..])
        .find_map(|kv| kv.strip_prefix("xt=urn:btih:"))
        .unwrap_or("")
        .split('&')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
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
pub fn classify_audio(title: &str, original_language: Option<&str>) -> AudioHint {
    let t_orig = title;
    let t = title.to_lowercase();
    let has_cyrillic = t_orig
        .chars()
        .any(|c| ('\u{0400}'..='\u{04FF}').contains(&c));

    // Multi-audio explícito. Cubre los tags más frecuentes del scene:
    //   MULTI, MULTi, MULTI4, MULTi5, MULTI+, dual audio, dual-audio,
    //   da2, 2audio, DL (release alemán con audio dual), y combos
    //   entre corchetes tipo [ENG+RUS], [EN.RU], [EN/RU/ES].
    if t.contains("multi")
        || t.contains("dual audio")
        || t.contains("dual-audio")
        || t.contains("dualaudio")
        || t.contains(" da2 ")
        || t.contains(" 2audio")
        || t.contains(".dl.")
        || t.contains(" dl ")
        || multi_language_bracket(&t)
    {
        return AudioHint::Multi;
    }

    // Doblajes rusos (muy comunes en RuTracker): Dub, MVO, DVO, AVO
    let ru_dub_markers = [" dub", " mvo", " dvo", " avo", "duo)", "dub ", "dub]"];
    if has_cyrillic || ru_dub_markers.iter().any(|m| t.contains(m)) || t.contains("dub (") {
        // Ojo: "dub" en un título en inglés sin cirílico suele ser doblaje
        // no-ruso (LATAM/ES/IT). Reservamos ru solo si hay cirílico.
        if has_cyrillic {
            return AudioHint::Dubbed("ru");
        }
    }

    // Doblajes castellano/latino
    if t.contains("castellano")
        || t.contains("espanol")
        || t.contains("español")
        || t.contains("spanish")
        || t.contains(" esp ")
        || t.contains("[esp]")
        || t.contains("latino")
    {
        return AudioHint::Dubbed("es");
    }

    // Doblajes en otros idiomas europeos comunes
    for (marker, lang) in [
        (" ita ", "it"),
        ("italian", "it"),
        (" fra ", "fr"),
        ("french", "fr"),
        (" ger ", "de"),
        ("german", "de"),
        ("deutsch", "de"),
    ] {
        if t.contains(marker) {
            return AudioHint::Dubbed(lang);
        }
    }

    // Marcador genérico "dub" en cualquier release scene
    if t.contains(" dub") || t.contains(".dub.") || t.ends_with(" dub") {
        return AudioHint::Dubbed("??");
    }

    // Si no aparece ningún marcador de doblaje y el título del release es
    // "en inglés simple" (sin cirílico) y la peli es originalmente en inglés,
    // asumimos audio original — es el default del scene internacional.
    let ol = original_language.unwrap_or("");
    if !has_cyrillic && ol == "en" {
        return AudioHint::Original;
    }

    // Si tenemos original_language pero el release parece no llevar audio no
    // original, también podemos marcarlo como original (poco fiable pero
    // razonable).
    if !has_cyrillic && !ol.is_empty() {
        return AudioHint::Original;
    }

    AudioHint::Unknown
}

/// Detecta patrones tipo `[ENG+RUS]`, `[EN.RU.ES]`, `[EN/FR]` en el título:
/// dos o más códigos de idioma ISO 639-1/-2 dentro del mismo bracket o
/// grupo entre puntos. Es un fallback pragmático — el scene marca
/// multi-audio de mil formas distintas.
fn multi_language_bracket(t: &str) -> bool {
    // Escaneamos ventanas cortas y contamos cuántos códigos de idioma
    // conocidos aparecen juntos (separados por +/./,/-///espacio).
    const LANG_CODES: &[&str] = &[
        "eng", "en", "rus", "ru", "esp", "spa", "es", "fre", "fra", "fr", "ita", "it", "ger",
        "deu", "de", "por", "pt", "jpn", "ja", "chi", "zh", "kor", "ko",
    ];
    let mut count_in_group = 0;
    let mut current_group_start = None;
    for (i, ch) in t.char_indices() {
        if ch == '[' || ch == '(' {
            current_group_start = Some(i + 1);
            count_in_group = 0;
        } else if ch == ']' || ch == ')' {
            if count_in_group >= 2 {
                return true;
            }
            current_group_start = None;
        } else if let Some(_start) = current_group_start {
            // Cuando cerramos el grupo miramos si tenía múltiples idiomas.
            // Aquí solo contamos codes visibles.
            for code in LANG_CODES {
                if t[i..].to_ascii_lowercase().starts_with(code) {
                    // Comprobamos que no sea parte de otra palabra (bordes
                    // simples: separador antes y después).
                    let end = i + code.len();
                    let after = t.as_bytes().get(end).copied().unwrap_or(b']');
                    if !after.is_ascii_alphabetic() {
                        count_in_group += 1;
                    }
                    break;
                }
            }
        }
    }
    false
}
