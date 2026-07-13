use anyhow::{Context, Result};

use crate::credentials;
use crate::keychain;

/// Credenciales de "app" baked-in en el binario al compilar. Se inyectan
/// vía `cargo install` con:
///
/// ```sh
/// LB_APP_CLIENT_ID=xxx \
/// LB_APP_CLIENT_SECRET=yyy \
/// LB_APP_TMDB_BEARER=zzz \
/// cargo install --path .
/// ```
///
/// Si no están definidas al compilar, valen `None` y hay que resolverlas
/// en runtime desde env/Keychain — útil en desarrollo.
const BAKED_CLIENT_ID: Option<&str> = option_env!("LB_APP_CLIENT_ID");
const BAKED_CLIENT_SECRET: Option<&str> = option_env!("LB_APP_CLIENT_SECRET");
const BAKED_TMDB_BEARER: Option<&str> = option_env!("LB_APP_TMDB_BEARER");

#[derive(Debug, Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    /// Refresh token del usuario. `None` cuando el usuario aún no ha hecho
    /// login desde la TUI — la vista de recomendaciones desencadena el
    /// flujo de login y rellena este campo antes de seguir.
    pub refresh_token: Option<String>,
    pub username: String,
    pub tmdb_bearer_token: String,
}

fn load_dotenv() {
    if let Some(home) = dirs::home_dir() {
        let env_path = home.join(".config").join("letterboxd-cli").join(".env");
        if env_path.exists() {
            dotenvy::from_path(&env_path).ok();
        }
    }
    dotenvy::dotenv().ok();
}

/// Carga las variables desde `.env` (global y local).
pub fn load_env_files() {
    load_dotenv();
}

/// Devuelve el bearer de TMDB usando la política habitual + baked-in.
#[allow(dead_code)]
pub fn tmdb_bearer() -> Option<String> {
    resolve("TMDB_BEARER_TOKEN", keychain::TMDB_BEARER_TOKEN)
        .or_else(|| BAKED_TMDB_BEARER.map(|s| s.to_string()))
}

/// Búsqueda de credenciales sensibles.
///
/// * macOS: primero variables de entorno / `.env`, y como fallback el
///   Keychain. Evita el diálogo de aprobación cada vez si hay `.env`
///   cacheado.
/// * Otros: solo variables de entorno / `.env`.
#[cfg(target_os = "macos")]
fn resolve(env_key: &str, keychain_service: &str) -> Option<String> {
    std::env::var(env_key)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| keychain::get(keychain_service))
}

#[cfg(not(target_os = "macos"))]
fn resolve(env_key: &str, _keychain_service: &str) -> Option<String> {
    std::env::var(env_key).ok().filter(|s| !s.is_empty())
}

impl Config {
    /// Carga la configuración con esta prioridad para cada campo:
    ///
    /// 1. Variables de entorno / `.env` (override en desarrollo).
    /// 2. Keychain de macOS (solo macOS, si estaba populado).
    /// 3. Baked-in en el binario (para builds distribuidos a usuarios).
    /// 4. `credentials.json` en `~/.config/letterboxd-cli/` (guardado tras
    ///    el login en la TUI). Solo para `refresh_token` y `username`.
    ///
    /// Los campos de app (client_id/secret, tmdb) deben estar por 1, 2 o
    /// 3 — si faltan, `from_env` devuelve error.
    ///
    /// `refresh_token` puede ser `None`; en ese caso la TUI mostrará
    /// login antes de las recomendaciones.
    pub fn from_env() -> Result<Self> {
        load_dotenv();
        let creds = credentials::load();

        let client_id = resolve("LETTERBOXD_CLIENT_ID", keychain::CLIENT_ID)
            .or_else(|| BAKED_CLIENT_ID.map(|s| s.to_string()))
            .context(
                "LETTERBOXD_CLIENT_ID no está definida. Recompila con \
                 `LB_APP_CLIENT_ID=xxx cargo install --path .`, o define \
                 LETTERBOXD_CLIENT_ID en el entorno.",
            )?;
        let client_secret = resolve("LETTERBOXD_CLIENT_SECRET", keychain::CLIENT_SECRET)
            .or_else(|| BAKED_CLIENT_SECRET.map(|s| s.to_string()))
            .unwrap_or_default();

        let refresh_token = resolve("LETTERBOXD_REFRESH_TOKEN", keychain::REFRESH_TOKEN)
            .or_else(|| creds.refresh_token.clone());

        let username = resolve("LETTERBOXD_USERNAME", keychain::USERNAME)
            .or_else(|| creds.username.clone())
            .unwrap_or_default();
        let username = std::env::var("LETTERBOXD_DISPLAY_USERNAME")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or(username);

        let tmdb_bearer_token = resolve("TMDB_BEARER_TOKEN", keychain::TMDB_BEARER_TOKEN)
            .or_else(|| BAKED_TMDB_BEARER.map(|s| s.to_string()))
            .context(
                "TMDB_BEARER_TOKEN no está definida. Recompila con \
                 `LB_APP_TMDB_BEARER=xxx cargo install --path .`, o define \
                 TMDB_BEARER_TOKEN en el entorno.",
            )?;

        Ok(Self {
            client_id,
            client_secret,
            refresh_token,
            username,
            tmdb_bearer_token,
        })
    }

    /// Carga la configuración solo desde `.env`/variables de entorno.
    #[allow(dead_code)]
    pub fn from_env_file_only() -> Result<Self> {
        load_dotenv();
        let client_id = std::env::var("LETTERBOXD_CLIENT_ID")
            .context("LETTERBOXD_CLIENT_ID no está definida")?;
        let client_secret = std::env::var("LETTERBOXD_CLIENT_SECRET").unwrap_or_default();
        let refresh_token = std::env::var("LETTERBOXD_REFRESH_TOKEN").ok();
        let username = std::env::var("LETTERBOXD_USERNAME").unwrap_or_default();
        let tmdb_bearer_token =
            std::env::var("TMDB_BEARER_TOKEN").context("TMDB_BEARER_TOKEN no está definida")?;
        Ok(Self {
            client_id,
            client_secret,
            refresh_token,
            username,
            tmdb_bearer_token,
        })
    }
}
