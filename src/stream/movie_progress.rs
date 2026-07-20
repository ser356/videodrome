//! Progreso de reproducción a nivel de PELÍCULA (o episodio),
//! independiente del torrent concreto que se usó.
//!
//! El módulo `resume` almacena posición por `<infohash>/<file_id>` —
//! útil para el warmup del propio torrent (VLC startup, dialog de
//! resume por magnet). Pero el user espera que "seguir viendo" sea
//! una propiedad de la PELÍCULA: si vio 40 min de "Blade Runner" con
//! un torrent 1080p y decide bajar el 4K, quiere continuar en el
//! minuto 40 sin depender del torrent anterior.
//!
//! Este store vive en `<cache>/movie_progress.json` y se keyea por
//! `tmdb_id` (o `<tmdb_id>:S<n>E<m>` para episodios de serie). Se
//! escribe en paralelo al resume por-infohash cada vez que
//! `report_position` recibe un `tmdb_id`. Se lee desde:
//!
//!   * `get_resume` (backend) — prioridad si el caller pasa tmdb_id.
//!   * `list_watch_progress` (backend) — feed de la sección "Seguir
//!     viendo" en Home.
//!
//! Al llegar al 95% del runtime la entrada se borra (peli terminada),
//! mismo umbral que `resume::COMPLETION_THRESHOLD` para consistencia.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cache::{cache_dir, now_unix};

pub(super) const MOVIE_PROGRESS_FILE: &str = "movie_progress.json";
const COMPLETION_THRESHOLD: f64 = 0.95;

/// Entrada de progreso por película/episodio. Guarda snapshot de
/// metadata del título para que la sección "Seguir viendo" pueda
/// pintarse SIN pegarle a TMDB otra vez (arranque instantáneo,
/// funciona offline si la caché de TMDB ya expiró).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovieProgress {
    pub tmdb_id: u64,
    /// `"movie"` o `"series"`. Serializado explícito para poder
    /// interpretar `season` y `episode` sin ambigüedad.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u16>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// Último magnet que se usó para reproducir la peli. Permite
    /// "resume rápido" desde Home (evita reabrir la lista de
    /// torrents). Si el user cambia de torrent, se sobrescribe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_magnet: Option<String>,
    /// Snapshot de la pista de subs activa cuando el user salió de
    /// la peli. Se re-hidrata al reentrar para que el sub siga
    /// activo (feature "los subs deben mantenerse al reentrar").
    /// `None` = sin subs / user los desactivó explícitamente.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sub: Option<LastSub>,
    pub seconds: f64,
    pub duration_seconds: f64,
    pub updated_at: u64,
}

/// Snapshot de la pista de subs para persistencia entre sesiones.
/// Discriminated union por `source` en el JSON — mismo shape que el
/// `ActiveSub` del frontend para evitar traducción.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "camelCase")]
pub enum LastSub {
    /// Fichero descargado a `<tmp>/videodrome-subs/`. `path` puede
    /// desaparecer si el SO limpia temp entre sesiones; el frontend
    /// debe re-fetchear el `.vtt` con tolerancia a fallo (si falla,
    /// hidrata el `SubsPanel` con el release para que el user re-click).
    #[serde(rename_all = "camelCase")]
    OpenSubs {
        path: String,
        release: String,
        language: String,
    },
    /// Pista embedded del contenedor (idx dentro del sub-array
    /// `kind='subtitle'` de ffprobe). Estable entre reproducciones
    /// del mismo torrent.
    #[serde(rename_all = "camelCase")]
    Embedded {
        idx: usize,
        release: String,
        language: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct MovieProgressStore {
    #[serde(default)]
    pub(super) entries: HashMap<String, MovieProgress>,
}

fn store_path() -> Option<PathBuf> {
    cache_dir().ok().map(|d| d.join(MOVIE_PROGRESS_FILE))
}

fn load_store(path: &Path) -> MovieProgressStore {
    let Ok(data) = std::fs::read_to_string(path) else {
        return MovieProgressStore::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn write_store_atomic(path: &Path, store: &MovieProgressStore) -> std::io::Result<()> {
    let json = serde_json::to_string(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// Clave estable dentro del store. Los episodios de serie NUNCA
/// colisionan con la peli tmdb del mismo id porque llevan el sufijo
/// `:S<n>E<m>` (TMDB series y movies tienen namespaces distintos
/// pero técnicamente los ids pueden solaparse — la key explícita
/// evita cualquier ambigüedad).
fn make_key(tmdb_id: u64, season: Option<u16>, episode: Option<u16>) -> String {
    match (season, episode) {
        (Some(s), Some(e)) => format!("{tmdb_id}:S{s}E{e}"),
        _ => tmdb_id.to_string(),
    }
}

/// Snapshot de metadata para la entrada. Se pasa desde `gui::report_position`
/// con lo que la vista Player ya tenía (title/imdb) + lo que resuelva
/// el backend con TMDB si hace falta rellenar poster.
#[derive(Debug, Clone, Default)]
pub struct MovieProgressMeta {
    pub kind: String,
    pub title: String,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub imdb_id: Option<String>,
    pub year: Option<u16>,
    pub last_magnet: Option<String>,
    /// `Some(...)` para actualizar la pista, `None` para NO tocar el
    /// campo (preservar valor previo). Para BORRAR el sub activo
    /// pasar `Some(LastSubUpdate::Clear)`.
    pub last_sub: Option<LastSubUpdate>,
}

/// Update explícito del `last_sub` en `save`. Distinguimos entre "no
/// toques este campo" (variante ausente en el `Option` outer) y
/// "borra el sub activo" (`Clear`).
#[derive(Debug, Clone)]
pub enum LastSubUpdate {
    Set(LastSub),
    Clear,
}

/// Persiste (o borra si supera COMPLETION_THRESHOLD) la posición de
/// una peli/episodio. Errores silenciosos — no debemos romper el
/// flujo del player si no podemos escribir el JSON.
pub fn save(
    tmdb_id: u64,
    season: Option<u16>,
    episode: Option<u16>,
    seconds: f64,
    duration_seconds: f64,
    meta: MovieProgressMeta,
) {
    let Some(path) = store_path() else { return };
    let mut store = load_store(&path);
    let key = make_key(tmdb_id, season, episode);

    if duration_seconds > 0.0 && seconds / duration_seconds >= COMPLETION_THRESHOLD {
        if store.entries.remove(&key).is_some() {
            if store.entries.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else if let Err(e) = write_store_atomic(&path, &store) {
                tracing::warn!(target: "resume", error = %e, "movie_progress save after complete");
            }
        }
        return;
    }

    // Merge: preservamos los campos de metadata que el caller no
    // conoce (p.ej. una llamada sin poster_path no debe borrar el
    // poster guardado la primera vez).
    let mut entry = store.entries.remove(&key).unwrap_or(MovieProgress {
        tmdb_id,
        kind: meta.kind.clone(),
        season,
        episode,
        title: meta.title.clone(),
        poster_path: None,
        backdrop_path: None,
        imdb_id: None,
        year: None,
        last_magnet: None,
        last_sub: None,
        seconds: 0.0,
        duration_seconds: 0.0,
        updated_at: 0,
    });
    entry.tmdb_id = tmdb_id;
    if !meta.kind.is_empty() {
        entry.kind = meta.kind;
    }
    entry.season = season;
    entry.episode = episode;
    if !meta.title.is_empty() {
        entry.title = meta.title;
    }
    if meta.poster_path.is_some() {
        entry.poster_path = meta.poster_path;
    }
    if meta.backdrop_path.is_some() {
        entry.backdrop_path = meta.backdrop_path;
    }
    if meta.imdb_id.is_some() {
        entry.imdb_id = meta.imdb_id;
    }
    if meta.year.is_some() {
        entry.year = meta.year;
    }
    if meta.last_magnet.is_some() {
        entry.last_magnet = meta.last_magnet;
    }
    match meta.last_sub {
        Some(LastSubUpdate::Set(sub)) => entry.last_sub = Some(sub),
        Some(LastSubUpdate::Clear) => entry.last_sub = None,
        None => {}
    }
    entry.seconds = seconds.max(0.0);
    if duration_seconds > 0.0 {
        entry.duration_seconds = duration_seconds;
    }
    entry.updated_at = now_unix();
    store.entries.insert(key, entry);

    if let Err(e) = write_store_atomic(&path, &store) {
        tracing::warn!(target: "resume", error = %e, "movie_progress save");
    }
}

/// Lee la entrada exacta (peli o episodio). `None` si no existe.
pub fn load(tmdb_id: u64, season: Option<u16>, episode: Option<u16>) -> Option<MovieProgress> {
    let path = store_path()?;
    let store = load_store(&path);
    store
        .entries
        .get(&make_key(tmdb_id, season, episode))
        .cloned()
}

/// Lista todas las entradas ordenadas por `updated_at` DESC (más
/// reciente primero). Filtra las que caen dentro del rango
/// [2%, 95%] del runtime — fuera de ese rango la UX de "seguir
/// viendo" no aporta (o acaba de empezar y volverá a arrancar de
/// cero, o ya terminó y solo estorba).
pub fn list_all() -> Vec<MovieProgress> {
    let Some(path) = store_path() else {
        return Vec::new();
    };
    let store = load_store(&path);
    let mut items: Vec<MovieProgress> = store
        .entries
        .into_values()
        .filter(|e| {
            if e.duration_seconds <= 0.0 {
                // Sin duración conocida no podemos calcular %, la
                // dejamos entrar (el frontend la pintará sin barra).
                return e.seconds > 30.0;
            }
            let f = e.seconds / e.duration_seconds;
            f > 0.02 && f < 0.95
        })
        .collect();
    items.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
    items
}

/// Borra una entrada explícitamente (botón "quitar de seguir viendo"
/// en Home). No falla si no existe.
pub fn remove(tmdb_id: u64, season: Option<u16>, episode: Option<u16>) {
    let Some(path) = store_path() else { return };
    let mut store = load_store(&path);
    if store
        .entries
        .remove(&make_key(tmdb_id, season, episode))
        .is_some()
    {
        if store.entries.is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            let _ = write_store_atomic(&path, &store);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_key_for_movie_uses_tmdb_only() {
        assert_eq!(make_key(1234, None, None), "1234");
    }

    #[test]
    fn make_key_for_episode_uses_series_suffix() {
        assert_eq!(make_key(1234, Some(2), Some(5)), "1234:S2E5");
    }

    #[test]
    fn make_key_for_season_only_falls_back_to_tmdb_only() {
        // Sin episode explícito NO usamos sufijo — solo episodios
        // concretos merecen key propia, los packs se hidratan aparte.
        assert_eq!(make_key(1234, Some(2), None), "1234");
        assert_eq!(make_key(1234, None, Some(5)), "1234");
    }

    fn sample_entry(tmdb_id: u64, seconds: f64) -> MovieProgress {
        MovieProgress {
            tmdb_id,
            kind: "movie".into(),
            season: None,
            episode: None,
            title: format!("Movie {tmdb_id}"),
            poster_path: Some("/p.jpg".into()),
            backdrop_path: Some("/b.jpg".into()),
            imdb_id: Some(format!("tt{tmdb_id}")),
            year: Some(2020),
            last_magnet: Some("magnet:?xt=urn:btih:abc".into()),
            last_sub: Some(LastSub::Embedded {
                idx: 0,
                release: "Track 1".into(),
                language: "en".into(),
            }),
            seconds,
            duration_seconds: 7200.0,
            updated_at: 1000 + tmdb_id,
        }
    }

    #[test]
    fn store_json_roundtrip_preserves_fields() {
        let mut store = MovieProgressStore::default();
        store.entries.insert("1".into(), sample_entry(1, 1800.0));
        let json = serde_json::to_string(&store).unwrap();
        let back: MovieProgressStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries.len(), 1);
        let e = &back.entries["1"];
        assert_eq!(e.title, "Movie 1");
        assert_eq!(e.year, Some(2020));
        assert!(matches!(e.last_sub, Some(LastSub::Embedded { idx: 0, .. })));
    }

    #[test]
    fn last_sub_open_subs_serializes_with_tag_and_camel_case() {
        let sub = LastSub::OpenSubs {
            path: "/tmp/x.srt".into(),
            release: "Release".into(),
            language: "es".into(),
        };
        let json = serde_json::to_string(&sub).unwrap();
        assert!(json.contains("\"source\":\"openSubs\""));
        assert!(json.contains("\"path\":\"/tmp/x.srt\""));
        // Roundtrip.
        let back: LastSub = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, LastSub::OpenSubs { .. }));
    }

    #[test]
    fn last_sub_embedded_serializes_with_tag() {
        let sub = LastSub::Embedded {
            idx: 3,
            release: "Track 3".into(),
            language: "en".into(),
        };
        let json = serde_json::to_string(&sub).unwrap();
        assert!(json.contains("\"source\":\"embedded\""));
        assert!(json.contains("\"idx\":3"));
    }

    #[test]
    fn write_store_atomic_and_load_store_roundtrip() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("movie_progress.json");
        let mut store = MovieProgressStore::default();
        store.entries.insert("42".into(), sample_entry(42, 900.0));
        write_store_atomic(&path, &store).unwrap();
        let back = load_store(&path);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries["42"].seconds, 900.0);
    }

    #[test]
    fn load_store_returns_default_on_missing_file() {
        let td = tempfile::tempdir().unwrap();
        let store = load_store(&td.path().join("nope.json"));
        assert!(store.entries.is_empty());
    }

    #[test]
    fn load_store_returns_default_on_corrupt_json() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("bad.json");
        std::fs::write(&path, "not json").unwrap();
        let store = load_store(&path);
        assert!(store.entries.is_empty());
    }

    #[test]
    fn write_store_atomic_creates_parent_dirs() {
        let td = tempfile::tempdir().unwrap();
        let nested = td.path().join("a/b/c/progress.json");
        let store = MovieProgressStore::default();
        write_store_atomic(&nested, &store).unwrap();
        assert!(nested.exists());
    }
}
