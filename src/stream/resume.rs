//! Persistencia de posición de reproducción (resume) por torrent y
//! por fichero. Wire format v2 con retrocompatibilidad v1. Extraído
//! de `stream.rs` en el refactor (commit paso 2). Sin cambios de
//! comportamiento.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::cache::{cache_dir, now_unix};

pub(super) const RESUME_FILE: &str = "resume.json";

/// Umbral de "peli terminada". Si el player reporta posición pasado
/// este porcentaje del runtime, borramos el `resume.json` para que la
/// próxima apertura no ofrezca reanudar los créditos.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
const COMPLETION_THRESHOLD: f64 = 0.95;

/// Estado de resume persistido en `<data_dir>/resume.json`.
///
/// Dos fuentes lo escriben:
///
///   * El player HTML llama a `save_position(seconds, duration)` cada
///     ~15s mientras reproduce. Es la fuente PREFERIDA: viene del
///     `<video>.currentTime` (posición exacta) y funciona en modo
///     direct y en búsquedas sin TMDB (no necesita `runtime_minutes`
///     para convertir bytes a segundos).
///
///   * El Drop de `StreamHandle` escribe `fraction` (byte-based) como
///     fallback para el path VLC, que no puede reportar posición
///     porque el frontend no sabe qué tiempo lleva el spawn de VLC.
///     Es la aproximación vieja: `max_seek_bytes / file_len`, con la
///     precisión que te da suponer bitrate constante.
///
/// Frontend consume: si `seconds` está presente lo usa directo; si no,
/// cae al camino viejo (`fraction × runtime_minutes × 60`).
///
/// Las escrituras se HACEN merge-style (leer, mutar, escribir) para
/// que un save del player no borre el `fraction` del Drop previo y
/// viceversa.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resume {
    /// Fracción byte-based en [0.0, 1.0]. `0.0` si no se ha escrito
    /// (raro; Drop siempre la actualiza). Fallback histórico.
    #[serde(default)]
    pub fraction: f32,
    /// Segundos absolutos reportados por el player HTML. `None`
    /// cuando la última sesión fue VLC (que no reporta) o cuando
    /// llegamos al Drop antes del primer `report_position`.
    #[serde(default)]
    pub seconds: Option<f64>,
    /// Duración total conocida al momento del último report.
    /// Necesaria para calcular "% completado" en la regla de
    /// borrado y para pintar la barra sin depender de TMDB.
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    pub updated_at: u64,
    /// Metadata de episodio si el resume es de una serie (§6 audit).
    /// Habilita "continuar viendo" y la lógica de "siguiente episodio"
    /// sin re-parsear el nombre del fichero cada vez.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<ResumeEpisode>,
}

/// Metadata mínima para identificar un episodio en el resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeEpisode {
    pub season: u16,
    pub episode: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<u64>,
}

/// Wire-format del `resume.json` en disco (§6 audit). Antes había un
/// único `Resume` plano por infohash: dos episodios del mismo pack
/// compartían infohash y el resume de E03 machacaba el de E02.
/// Ahora un mapa por `file_id` (como string, por compatibilidad JSON).
///
/// Formato legacy (v1) se sigue leyendo — ver `load_store`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ResumeStore {
    #[serde(default)]
    pub(super) files: HashMap<String, Resume>,
}

/// Discriminated read: primero intenta parsear la v2
/// (`{"files":{...}}`); si el fichero es legacy (`{"fraction":...}`
/// plano) cae al parser antiguo y lo migra a v2 in-memory bajo la
/// clave `"0"`. La migración se persiste en el siguiente `save_*`
/// (write-through) — no reescribimos aquí para mantener la ruta de
/// lectura pura.
///
/// Racional del audit §6: adoptar la entrada vieja para file_id=0 es
/// correcto para torrents mono-fichero (la única elección posible).
/// Para packs multi-fichero, la primera lectura devuelve el resume
/// bajo "0" — quizás no sea el fichero real que reproducía el user,
/// pero un mismatch puntual es mejor que perder la posición.
///
/// Distingue tres estados: fichero ausente (nueva entrada válida
/// vacía), store parseado, o corrupto (write parcial de una sesión
/// previa que murió a mitad — NO reescribir para preservar la
/// posibilidad de recuperación manual).
pub(super) enum ResumeParse {
    Absent,
    Store(ResumeStore),
    Corrupt,
}

pub(super) fn read_store(path: &Path) -> ResumeParse {
    let data = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return ResumeParse::Absent,
    };
    let value: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "resume",
                path = %path.display(),
                error = %e,
                "unparseable as JSON; preserving"
            );
            return ResumeParse::Corrupt;
        }
    };
    if value.get("files").is_some() {
        match serde_json::from_value::<ResumeStore>(value) {
            Ok(store) => ResumeParse::Store(store),
            Err(e) => {
                tracing::warn!(
                    target: "resume",
                    path = %path.display(),
                    error = %e,
                    "v2 parse failed; preserving"
                );
                ResumeParse::Corrupt
            }
        }
    } else {
        match serde_json::from_value::<Resume>(value) {
            Ok(legacy) => {
                let mut files = HashMap::new();
                files.insert("0".to_string(), legacy);
                ResumeParse::Store(ResumeStore { files })
            }
            Err(e) => {
                tracing::warn!(
                    target: "resume",
                    path = %path.display(),
                    error = %e,
                    "legacy parse failed; preserving"
                );
                ResumeParse::Corrupt
            }
        }
    }
}

/// Conveniencia: colapsa `Absent | Corrupt` a un store vacío. Solo
/// para lecturas puras (`load_resume*`) donde perder acceso al
/// corrupto es aceptable — el corrupto sigue en disco y el próximo
/// write lo respetará.
fn load_store(path: &Path) -> ResumeStore {
    match read_store(path) {
        ResumeParse::Store(s) => s,
        _ => ResumeStore::default(),
    }
}

/// Lee el `resume.json` de una entrada, para un `file_id` concreto.
/// Devuelve `None` si no hay entrada para ese file_id (o el fichero
/// no existe / está corrupto). Ver `load_store` para detalles del
/// wire format y la migración de v1 legacy.
///
/// Callers que no saben el file_id (dialog de resume ANTES del start)
/// pueden usar `load_resume_any`, que devuelve la entrada más
/// reciente del store.
#[allow(dead_code)]
pub fn load_resume(infohash: &str, file_id: usize) -> Option<Resume> {
    load_resume_in(&cache_dir().ok()?, infohash, file_id)
}

/// Devuelve la entrada de resume más reciente para el infohash, sin
/// especificar file_id. Útil para el dialog pre-start del player
/// (aún no se ha resuelto metadata → no hay file_id todavía). Si el
/// caller pasa `episode`, filtra a entradas cuya `episode` matchee
/// exactamente (S+E); útil para el flujo de serie donde el user ya
/// sabe qué episodio va a ver.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn load_resume_any(infohash: &str, episode: Option<(u16, u16)>) -> Option<Resume> {
    load_resume_any_in(&cache_dir().ok()?, infohash, episode)
}

/// Variante testeable: opera sobre un directorio base explícito
/// (`<base>/<infohash>/resume.json`) en vez de resolver `cache_dir()`.
#[allow(dead_code)]
fn load_resume_in(base: &Path, infohash: &str, file_id: usize) -> Option<Resume> {
    let path = base.join(infohash).join(RESUME_FILE);
    let store = load_store(&path);
    store.files.get(&file_id.to_string()).cloned()
}

fn load_resume_any_in(base: &Path, infohash: &str, episode: Option<(u16, u16)>) -> Option<Resume> {
    let path = base.join(infohash).join(RESUME_FILE);
    let store = load_store(&path);
    let mut candidates: Vec<&Resume> = if let Some((s, e)) = episode {
        store
            .files
            .values()
            .filter(|r| matches!(&r.episode, Some(ep) if ep.season == s && ep.episode == e))
            .collect()
    } else {
        store.files.values().collect()
    };
    candidates.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    candidates.first().map(|r| (*r).clone())
}

/// Persiste una posición reportada por el player HTML. Merge-style:
/// si ya existe un `resume.json` (con `fraction` puesto por el Drop
/// anterior), preservamos ese campo y solo actualizamos `seconds` +
/// `duration_seconds` + `updated_at`.
///
/// `file_id` selecciona la entrada dentro del store multi-file
/// (§6 audit) — dos episodios del mismo pack conviven sin pisarse.
/// `episode` guarda metadata de S/E cuando aplica (habilita
/// "continuar viendo" y "siguiente episodio" sin re-parsear
/// nombres).
///
/// Si la posición reportada supera `COMPLETION_THRESHOLD` (95%) del
/// runtime, borra SOLO esa entrada del store — otras entradas del
/// mismo torrent (otros episodios) sobreviven.
///
/// Errores silenciosos (log a stderr): el flujo del player no debe
/// romperse porque no podamos persistir una posición.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn save_position(
    infohash: &str,
    file_id: usize,
    seconds: f64,
    duration_seconds: f64,
    episode: Option<ResumeEpisode>,
) {
    let Ok(base) = cache_dir() else {
        return;
    };
    save_position_in(&base, infohash, file_id, seconds, duration_seconds, episode);
}

/// Variante testeable: idem `save_position` sobre un base dir
/// explícito. Los tests pueden crear un tempdir y llamar aquí sin
/// tocar la caché real del sistema (portable a macOS/Windows, donde
/// `dirs::cache_dir` no respeta `XDG_CACHE_HOME`).
fn save_position_in(
    base: &Path,
    infohash: &str,
    file_id: usize,
    seconds: f64,
    duration_seconds: f64,
    episode: Option<ResumeEpisode>,
) {
    let entry = base.join(infohash);
    // Si la entrada no existe (magnet nunca reproducido en persistente,
    // o purgada por el prune), no la creamos aquí — el StreamHandle
    // vivo la habría creado al arrancar.
    if !entry.exists() {
        return;
    }
    let path = entry.join(RESUME_FILE);

    // Read-modify-write con resiliencia a corrupción: si el fichero
    // existe pero no parsea (write parcial de una sesión previa),
    // NO lo sobreescribimos — preservar la posibilidad de recuperación
    // manual es preferible a machacar con default limpio.
    let mut store = match read_store(&path) {
        ResumeParse::Store(s) => s,
        ResumeParse::Absent => ResumeStore::default(),
        ResumeParse::Corrupt => return,
    };
    let key = file_id.to_string();

    // Regla de completado: si `seconds/duration > 0.95`, borra ESTA
    // entrada del store. Preserva otras entradas (otros episodios).
    // El check requiere una duración conocida > 0 — si el player nos
    // manda `duration_seconds = 0` (ffprobe falló, live stream), no
    // aplicamos la regla.
    if duration_seconds > 0.0 && seconds / duration_seconds >= COMPLETION_THRESHOLD {
        if store.files.remove(&key).is_some() {
            if store.files.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else if let Err(e) = write_store_atomic(&path, &store) {
                tracing::warn!(target: "resume", error = %e, "failed to persist store after completion");
            }
        }
        return;
    }

    let mut entry_r = store.files.remove(&key).unwrap_or_default();
    entry_r.seconds = Some(seconds.max(0.0));
    if duration_seconds > 0.0 {
        entry_r.duration_seconds = Some(duration_seconds);
    }
    if episode.is_some() {
        entry_r.episode = episode;
    }
    entry_r.updated_at = now_unix();
    store.files.insert(key, entry_r);

    if let Err(e) = write_store_atomic(&path, &store) {
        tracing::warn!(target: "resume", error = %e, "failed to persist position");
    }
}

/// Escribe el store atómicamente. Rename es atómico en POSIX y en
/// NTFS (Windows). No cross-device (tmp y destino en el mismo dir),
/// así que no falla por EXDEV. Evita que un crash o Cmd+Q a mitad de
/// escritura deje el fichero truncado (que la próxima lectura
/// interpretaría como corrupto y descartaría).
pub(super) fn write_store_atomic(path: &Path, store: &ResumeStore) -> std::io::Result<()> {
    let json = serde_json::to_string(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

// Tests de persistencia de resume. Operan sobre un tempdir por
// test vía las variantes `_in` de `save_position`/`load_resume`,
// así que son portables (macOS/Windows/Linux) y no tocan la
// caché real del sistema.
#[cfg(all(test, feature = "gui"))]
mod tests {
    use super::*;

    fn make_entry(base: &std::path::Path, hash: &str) {
        std::fs::create_dir_all(base.join(hash)).unwrap();
    }

    // Wrappers cortos: los tests históricos pasaban seconds+duration.
    // Con §6 audit añadimos file_id + episode. Estos helpers
    // encapsulan file_id=0, episode=None → los tests legacy se
    // leen igual y solo los nuevos usan la firma completa.
    fn save(base: &std::path::Path, hash: &str, seconds: f64, duration: f64) {
        save_position_in(base, hash, 0, seconds, duration, None);
    }
    fn load(base: &std::path::Path, hash: &str) -> Option<Resume> {
        load_resume_in(base, hash, 0)
    }

    #[test]
    fn save_position_writes_seconds_and_duration() {
        let td = tempfile::tempdir().unwrap();
        let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        make_entry(td.path(), hash);
        save(td.path(), hash, 123.4, 4500.0);
        let r = load(td.path(), hash).unwrap();
        assert_eq!(r.seconds, Some(123.4));
        assert_eq!(r.duration_seconds, Some(4500.0));
    }

    #[test]
    fn save_position_preserves_prior_fraction() {
        let td = tempfile::tempdir().unwrap();
        let hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        make_entry(td.path(), hash);
        // Simulamos un Drop previo legacy (v1) escribiendo el
        // shape plano. `load_store` lo migra bajo files["0"].
        let path = td.path().join(hash).join(RESUME_FILE);
        std::fs::write(
            &path,
            r#"{"fraction":0.42,"seconds":null,"duration_seconds":null,"updated_at":100}"#,
        )
        .unwrap();
        save(td.path(), hash, 60.0, 3600.0);
        let r = load(td.path(), hash).unwrap();
        assert!(
            (r.fraction - 0.42).abs() < 1e-6,
            "fraction sobrescrita: {r:?}"
        );
        assert_eq!(r.seconds, Some(60.0));
        assert_eq!(r.duration_seconds, Some(3600.0));
    }

    #[test]
    fn save_position_deletes_when_over_completion_threshold() {
        let td = tempfile::tempdir().unwrap();
        let hash = "cccccccccccccccccccccccccccccccccccccccc";
        make_entry(td.path(), hash);
        save(td.path(), hash, 100.0, 1000.0);
        assert!(load(td.path(), hash).is_some());
        save(td.path(), hash, 960.0, 1000.0);
        assert!(load(td.path(), hash).is_none());
    }

    #[test]
    fn save_position_noop_when_entry_dir_missing() {
        let td = tempfile::tempdir().unwrap();
        let hash = "dddddddddddddddddddddddddddddddddddddddd";
        save(td.path(), hash, 30.0, 60.0);
        assert!(load(td.path(), hash).is_none());
    }

    #[test]
    fn save_position_ignores_zero_duration() {
        let td = tempfile::tempdir().unwrap();
        let hash = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        make_entry(td.path(), hash);
        save(td.path(), hash, 1_000_000.0, 0.0);
        let r = load(td.path(), hash).unwrap();
        assert_eq!(r.seconds, Some(1_000_000.0));
        assert!(r.duration_seconds.is_none());
    }

    #[test]
    fn save_position_deletes_at_exactly_95_percent() {
        let td = tempfile::tempdir().unwrap();
        let hash = "ffffffffffffffffffffffffffffffffffffffff";
        make_entry(td.path(), hash);
        save(td.path(), hash, 50.0, 100.0);
        assert!(load(td.path(), hash).is_some());
        save(td.path(), hash, 95.0, 100.0);
        assert!(load(td.path(), hash).is_none());
    }

    #[test]
    fn save_position_preserves_corrupt_existing_file() {
        let td = tempfile::tempdir().unwrap();
        let hash = "1111111111111111111111111111111111111111";
        make_entry(td.path(), hash);
        let path = td.path().join(hash).join(RESUME_FILE);
        let corrupt = r#"{"fraction":0.42,"seconds":123.4"#;
        std::fs::write(&path, corrupt).unwrap();
        save(td.path(), hash, 999.0, 3600.0);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, corrupt);
    }

    #[test]
    fn save_position_writes_are_atomic() {
        let td = tempfile::tempdir().unwrap();
        let hash = "2222222222222222222222222222222222222222";
        make_entry(td.path(), hash);
        save(td.path(), hash, 42.0, 3600.0);
        let entry_dir = td.path().join(hash);
        let leftovers: Vec<_> = std::fs::read_dir(&entry_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s == "tmp")
            })
            .collect();
        assert!(leftovers.is_empty(), "quedaron `.tmp` sin renombrar");
    }

    // ── §6 audit: multi-file ─────────────────────────────────

    #[test]
    fn save_position_isolates_entries_per_file_id() {
        let td = tempfile::tempdir().unwrap();
        let hash = "3333333333333333333333333333333333333333";
        make_entry(td.path(), hash);
        save_position_in(td.path(), hash, 3, 100.0, 3600.0, None);
        save_position_in(td.path(), hash, 5, 200.0, 3600.0, None);
        let r3 = load_resume_in(td.path(), hash, 3).unwrap();
        let r5 = load_resume_in(td.path(), hash, 5).unwrap();
        assert_eq!(r3.seconds, Some(100.0));
        assert_eq!(r5.seconds, Some(200.0));
    }

    #[test]
    fn save_position_completion_removes_only_that_file() {
        // E03 se termina, E02 sigue vivo con su posición.
        let td = tempfile::tempdir().unwrap();
        let hash = "4444444444444444444444444444444444444444";
        make_entry(td.path(), hash);
        save_position_in(td.path(), hash, 2, 300.0, 3600.0, None);
        save_position_in(td.path(), hash, 3, 3500.0, 3600.0, None);
        // E03 (file 3) > 95% → borrado
        assert!(load_resume_in(td.path(), hash, 3).is_none());
        // E02 (file 2) intacto
        let r = load_resume_in(td.path(), hash, 2).unwrap();
        assert_eq!(r.seconds, Some(300.0));
    }

    #[test]
    fn save_position_stores_episode_metadata() {
        let td = tempfile::tempdir().unwrap();
        let hash = "5555555555555555555555555555555555555555";
        make_entry(td.path(), hash);
        let ep = ResumeEpisode {
            season: 2,
            episode: 3,
            tmdb_id: Some(31234),
        };
        save_position_in(td.path(), hash, 3, 100.0, 3600.0, Some(ep));
        let r = load_resume_in(td.path(), hash, 3).unwrap();
        let e = r.episode.expect("episode meta debía persistirse");
        assert_eq!(e.season, 2);
        assert_eq!(e.episode, 3);
        assert_eq!(e.tmdb_id, Some(31234));
    }

    #[test]
    fn load_resume_any_returns_most_recent_by_updated_at() {
        let td = tempfile::tempdir().unwrap();
        let hash = "6666666666666666666666666666666666666666";
        make_entry(td.path(), hash);
        save_position_in(td.path(), hash, 1, 10.0, 3600.0, None);
        std::thread::sleep(std::time::Duration::from_secs(1));
        save_position_in(td.path(), hash, 2, 20.0, 3600.0, None);
        let r = load_resume_any_in(td.path(), hash, None).unwrap();
        assert_eq!(r.seconds, Some(20.0));
    }

    #[test]
    fn load_resume_any_filters_by_episode() {
        let td = tempfile::tempdir().unwrap();
        let hash = "7777777777777777777777777777777777777777";
        make_entry(td.path(), hash);
        let ep_a = ResumeEpisode {
            season: 1,
            episode: 1,
            tmdb_id: None,
        };
        let ep_b = ResumeEpisode {
            season: 2,
            episode: 3,
            tmdb_id: None,
        };
        save_position_in(td.path(), hash, 0, 10.0, 3600.0, Some(ep_a));
        save_position_in(td.path(), hash, 5, 400.0, 3600.0, Some(ep_b));
        let r = load_resume_any_in(td.path(), hash, Some((2, 3))).unwrap();
        assert_eq!(r.seconds, Some(400.0));
        let none = load_resume_any_in(td.path(), hash, Some((9, 9)));
        assert!(none.is_none());
    }

    #[test]
    fn legacy_v1_file_migrates_to_files_zero_on_load() {
        // Un resume.json escrito por el binario antiguo (plano)
        // debe leerse como si estuviera bajo files["0"].
        let td = tempfile::tempdir().unwrap();
        let hash = "8888888888888888888888888888888888888888";
        make_entry(td.path(), hash);
        let path = td.path().join(hash).join(RESUME_FILE);
        std::fs::write(
            &path,
            r#"{"fraction":0.5,"seconds":600.0,"duration_seconds":3600.0,"updated_at":42}"#,
        )
        .unwrap();
        let r = load_resume_in(td.path(), hash, 0).unwrap();
        assert_eq!(r.seconds, Some(600.0));
        assert!((r.fraction - 0.5).abs() < 1e-6);
        // Otros file_ids no matchean.
        assert!(load_resume_in(td.path(), hash, 3).is_none());
    }
}
