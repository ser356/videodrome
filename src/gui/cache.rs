use super::{
    CachedSearch, CachedTorrentSearch, TorrentSearchResult, SEARCH_CACHE_FILE, TORRENT_CACHE_FILE,
    TORRENT_CACHE_TTL_EMPTY, TORRENT_CACHE_TTL_HITS, TORRENT_CACHE_TTL_PARTIAL_FAIL,
};
use anyhow::Context;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("El tiempo no puede ir hacia atrás")
        .as_secs()
}

pub(super) fn current_ui_lang() -> Option<String> {
    crate::preferences::load().ui_language
}

pub(super) fn config_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("videodrome");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub(super) fn search_cache_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join(SEARCH_CACHE_FILE))
}

pub(super) fn load_search_cache() -> HashMap<String, CachedSearch> {
    let Ok(path) = search_cache_path() else {
        return HashMap::new();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

pub(super) fn save_search_cache(cache: &HashMap<String, CachedSearch>) {
    if let Ok(path) = search_cache_path() {
        if let Ok(json) = serde_json::to_string(cache) {
            let _ = std::fs::write(path, json);
        }
    }
}

pub(super) fn normalize_query(q: &str) -> String {
    q.trim().to_lowercase()
}

// ── Caché de búsqueda de torrents (Fase 4a) ─────────────────────────────────

pub(super) fn torrent_cache_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join(TORRENT_CACHE_FILE))
}

pub(super) fn load_torrent_cache() -> HashMap<String, CachedTorrentSearch> {
    let Ok(path) = torrent_cache_path() else {
        return HashMap::new();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

pub(super) fn save_torrent_cache(cache: &HashMap<String, CachedTorrentSearch>) {
    if let Ok(path) = torrent_cache_path() {
        if let Ok(json) = serde_json::to_string(cache) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// Key estable para el caché de torrents. Prefiere el `imdb_id`
/// (canónico, cross-idioma); si no lo hay, cae a `direct:<norm>:<year>`.
///
/// Series (§7 audit): añade sufijo `:sSSeEE` o `:sSS` cuando aplica
/// para que un episodio no colisione con otro del mismo IMDb, ni
/// con la peli homónima si TMDB reportara el mismo imdb (raro).
pub(super) fn torrent_cache_key(imdb_id: Option<&str>, title: &str, year: Option<u16>) -> String {
    torrent_cache_key_with_ep(imdb_id, title, year, None, None)
}

pub(super) fn torrent_cache_key_with_ep(
    imdb_id: Option<&str>,
    title: &str,
    year: Option<u16>,
    season: Option<u16>,
    episode: Option<u16>,
) -> String {
    let base = if let Some(id) = imdb_id.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        id.to_string()
    } else {
        let norm = normalize_query(title);
        match year {
            Some(y) => format!("direct:{norm}:{y}"),
            None => format!("direct:{norm}:-"),
        }
    };
    match (season, episode) {
        (Some(s), Some(e)) => format!("{base}:s{s:02}e{e:02}"),
        (Some(s), None) => format!("{base}:s{s:02}"),
        _ => base,
    }
}

/// TTL aplicable a una entrada según qué contiene:
///   * Vacío (sin ningún result) → `TTL_EMPTY` (5 min). Evita
///     martillear providers cuando el user vuelve a una peli sin
///     releases (estrenos futuros).
///   * Algún provider falló (`ok=false`) → `TTL_PARTIAL_FAIL`
///     (60s). Los errores transitorios NO deben clavarse 30 min en
///     la UI; una nueva request poco después verá el estado sano.
///   * Todo OK y hay results → `TTL_HITS` (30 min).
pub(super) fn torrent_cache_ttl(entry: &CachedTorrentSearch) -> u64 {
    if entry.result.results.is_empty() {
        return TORRENT_CACHE_TTL_EMPTY;
    }
    let any_failed = entry.result.providers.iter().any(|p| !p.ok);
    if any_failed {
        TORRENT_CACHE_TTL_PARTIAL_FAIL
    } else {
        TORRENT_CACHE_TTL_HITS
    }
}

/// Devuelve `Some(result)` si el caché tiene una entrada fresca para
/// la key dada. Marca los providers como `from_cache = true` para que
/// la UI pueda diferenciarlos del sondeo vivo.
pub(super) fn torrent_cache_get_fresh(
    cache: &HashMap<String, CachedTorrentSearch>,
    key: &str,
) -> Option<TorrentSearchResult> {
    let entry = cache.get(key)?;
    let age = now_unix().saturating_sub(entry.timestamp);
    if age > torrent_cache_ttl(entry) {
        return None;
    }
    let mut result = entry.result.clone();
    for p in &mut result.providers {
        p.from_cache = true;
    }
    Some(result)
}

/// Persiste (o refresca) una entrada en el caché de torrents.
pub(super) fn torrent_cache_put(
    cache: &mut HashMap<String, CachedTorrentSearch>,
    key: String,
    result: &TorrentSearchResult,
) {
    // Guardamos una copia CON `from_cache = false` en los providers
    // — el flag se aplica solo cuando se lee, no cuando se guarda.
    // (Un round-trip vía caché seguiría marcándolos correctamente).
    let mut snapshot = result.clone();
    for p in &mut snapshot.providers {
        p.from_cache = false;
    }
    cache.insert(
        key,
        CachedTorrentSearch {
            timestamp: now_unix(),
            result: snapshot,
        },
    );
    save_torrent_cache(cache);
}
