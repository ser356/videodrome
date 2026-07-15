//! Preferencias persistentes del usuario que la GUI puede editar en la
//! vista de Ajustes. Vive en `~/.config/videodrome/preferences.json`.
//!
//! Deliberadamente ligero: solo lo que tiene sentido cambiar desde la
//! app (defaults de la vista Recs, idiomas de subs). Todo lo que es
//! configuración de despliegue (Torznab URL/APIKEY, credenciales)
//! sigue por env/Keychain — no queremos filtrar secretos a un JSON de
//! preferencias que también se sincroniza con dotfiles del user.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const PREFERENCES_FILE: &str = "preferences.json";

/// Preferencias del usuario. Cada campo lleva `#[serde(default)]` para
/// que añadir uno nuevo NO invalide los `preferences.json` existentes:
/// los campos ausentes se rellenan con el default individual del campo
/// (`default_min_rating` → 4.0, etc.), no con `Preferences::default()`
/// entera. Sin esto, el primer save después de un update borraba las
/// preferencias del usuario sin avisar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    /// Rating mínimo por defecto en la vista Recs (0.5 – 5.0).
    #[serde(default = "default_min_rating")]
    pub default_min_rating: f32,
    /// Número de recomendaciones por defecto. Alineado con la CLI
    /// (`videodrome recommend` usa 10) — la GUI puede subirlo si el user
    /// lo cambia en Ajustes.
    #[serde(default = "default_count")]
    pub default_count: usize,
    /// Idiomas de subtítulos separados por coma (ISO 639-1). Se pasa a
    /// OpenSubtitles como parámetro `languages`.
    #[serde(default = "default_subtitle_languages")]
    pub subtitle_languages: String,
    /// Días que se conserva la caché de streams antes de purgarse
    /// automáticamente al arrancar la GUI. Cada entrada guarda el
    /// mtime de un fichero `.last_used` dentro de `<hash>/` que se
    /// toca al iniciar/terminar el stream; si excede el TTL se borra
    /// el directorio entero. Rango efectivo 1–365; 0 se trata como 1.
    #[serde(default = "default_stream_cache_ttl_days")]
    pub stream_cache_ttl_days: u32,
}

fn default_min_rating() -> f32 {
    4.0
}
fn default_count() -> usize {
    10
}
fn default_subtitle_languages() -> String {
    crate::subtitles::DEFAULT_LANGUAGES.to_string()
}
fn default_stream_cache_ttl_days() -> u32 {
    7
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            default_min_rating: default_min_rating(),
            default_count: default_count(),
            subtitle_languages: default_subtitle_languages(),
            stream_cache_ttl_days: default_stream_cache_ttl_days(),
        }
    }
}

fn preferences_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("videodrome");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(PREFERENCES_FILE))
}

pub fn load() -> Preferences {
    let Ok(path) = preferences_path() else {
        return Preferences::default();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return Preferences::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

pub fn save(prefs: &Preferences) -> Result<()> {
    let path = preferences_path()?;
    let json = serde_json::to_string_pretty(prefs).context("Error al serializar preferencias")?;
    std::fs::write(path, json).context("Error al escribir preferences.json")?;
    Ok(())
}
