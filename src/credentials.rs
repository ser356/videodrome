//! Almacén cross-platform de credenciales de usuario (refresh_token +
//! username) en `~/.config/letterboxd-cli/credentials.json`.
//!
//! Motivación: la app se distribuye a usuarios de macOS/Linux/Windows. El
//! Keychain solo existe en macOS, así que las credenciales del usuario se
//! guardan en un JSON con permisos `0600` (Unix) — funciona en todos los
//! sistemas por igual y no requiere entrar en el llavero del sistema.
//!
//! Se usa como fallback tras las variables de entorno y (en macOS) del
//! Keychain — así los desarrolladores que ya lo tienen todo en el Keychain
//! siguen funcionando sin cambios.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const FILE_NAME: &str = "credentials.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("letterboxd-cli");
    std::fs::create_dir_all(&dir).with_context(|| format!("No se pudo crear {}", dir.display()))?;
    Ok(dir.join(FILE_NAME))
}

/// Lee las credenciales del fichero. Devuelve `Credentials` vacío si el
/// fichero no existe o es ilegible — así el login flow arranca limpio.
pub fn load() -> Credentials {
    let Ok(p) = path() else {
        return Credentials::default();
    };
    let Ok(data) = std::fs::read_to_string(&p) else {
        return Credentials::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Guarda las credenciales al fichero con permisos 0600 (Unix). Solo el
/// usuario actual puede leerlo.
pub fn save(creds: &Credentials) -> Result<()> {
    let p = path()?;
    let json = serde_json::to_string_pretty(creds)?;
    std::fs::write(&p, json).with_context(|| format!("No se pudo escribir {}", p.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

/// Borra el fichero de credenciales (logout).
#[allow(dead_code)]
pub fn clear() -> Result<()> {
    let p = path()?;
    if p.exists() {
        std::fs::remove_file(&p).with_context(|| format!("No se pudo borrar {}", p.display()))?;
    }
    Ok(())
}
