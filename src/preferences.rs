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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    /// Rating mínimo por defecto en la vista Recs (0.5 – 5.0).
    pub default_min_rating: f32,
    /// Número de recomendaciones por defecto.
    pub default_count: usize,
    /// Idiomas de subtítulos separados por coma (ISO 639-1). Se pasa a
    /// OpenSubtitles como parámetro `languages`.
    pub subtitle_languages: String,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            default_min_rating: 4.0,
            default_count: 20,
            subtitle_languages: crate::subtitles::DEFAULT_LANGUAGES.to_string(),
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
    let json = serde_json::to_string_pretty(prefs)
        .context("Error al serializar preferencias")?;
    std::fs::write(path, json).context("Error al escribir preferences.json")?;
    Ok(())
}
