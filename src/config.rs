use anyhow::{Context, Result};

use crate::keychain;

#[derive(Debug, Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    pub username: String,
    pub tmdb_bearer_token: String,
}

fn load_dotenv() {
    // Buscar .env en ~/.config/letterboxd-cli/ (funciona en macOS y Linux)
    if let Some(home) = dirs::home_dir() {
        let env_path = home.join(".config").join("letterboxd-cli").join(".env");
        if env_path.exists() {
            dotenvy::from_path(&env_path).ok();
        }
    }
    // Fallback: .env en el directorio de trabajo actual (desarrollo)
    dotenvy::dotenv().ok();
}

/// Carga las variables desde `.env` (global y local). Exporta esta función
/// para que `keychain import` pueda leer del entorno tras poblarlo.
pub fn load_env_files() {
    load_dotenv();
}

/// Devuelve el bearer de TMDB usando la misma política que `Config::from_env`
/// pero sin exigir el resto de credenciales de Letterboxd. Útil para
/// subcomandos que solo necesitan TMDB (por ejemplo `torrents --imdb`).
pub fn tmdb_bearer() -> Option<String> {
    resolve("TMDB_BEARER_TOKEN", keychain::TMDB_BEARER_TOKEN)
}

/// Busca una credencial sensible.
///
/// * En macOS: primero variables de entorno / `.env`, y como fallback el
///   Keychain. Esto evita el diálogo de aprobación del Keychain cada vez
///   que se ejecuta el CLI si ya hay un `.env` cacheado (típicamente creado
///   por `letterboxd-cli keychain export`). El Keychain sigue siendo la
///   fuente de verdad original.
/// * En otros sistemas: solo variables de entorno / `.env`.
#[cfg(target_os = "macos")]
fn resolve(env_key: &str, keychain_service: &str) -> Option<String> {
    std::env::var(env_key)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| keychain::get(keychain_service))
}

#[cfg(not(target_os = "macos"))]
fn resolve(env_key: &str, _keychain_service: &str) -> Option<String> {
    std::env::var(env_key).ok()
}

#[cfg(target_os = "macos")]
fn missing(env_key: &str, keychain_service: &str) -> String {
    format!(
        "{env_key} no está definida (ni en .env ni en el Keychain como `{keychain_service}`). \
         Ejecuta `letterboxd-cli keychain import` para importarla desde .env, \
         o `letterboxd-cli keychain export` para volcar el Keychain a .env."
    )
}

#[cfg(not(target_os = "macos"))]
fn missing(env_key: &str, _keychain_service: &str) -> String {
    format!("{env_key} no está definida")
}

impl Config {
    /// Carga la configuración para uso normal de la app. En macOS las
    /// credenciales sensibles se leen **solo** del Keychain; en otros
    /// sistemas se leen de `.env`/entorno.
    pub fn from_env() -> Result<Self> {
        load_dotenv();

        let client_id = resolve("LETTERBOXD_CLIENT_ID", keychain::CLIENT_ID)
            .with_context(|| missing("LETTERBOXD_CLIENT_ID", keychain::CLIENT_ID))?;
        let client_secret =
            resolve("LETTERBOXD_CLIENT_SECRET", keychain::CLIENT_SECRET).unwrap_or_default();
        let refresh_token = resolve("LETTERBOXD_REFRESH_TOKEN", keychain::REFRESH_TOKEN)
            .with_context(|| missing("LETTERBOXD_REFRESH_TOKEN", keychain::REFRESH_TOKEN))?;
        let username = resolve("LETTERBOXD_USERNAME", keychain::USERNAME)
            .with_context(|| missing("LETTERBOXD_USERNAME", keychain::USERNAME))?;
        let tmdb_bearer_token = resolve("TMDB_BEARER_TOKEN", keychain::TMDB_BEARER_TOKEN)
            .with_context(|| missing("TMDB_BEARER_TOKEN", keychain::TMDB_BEARER_TOKEN))?;

        Ok(Self {
            client_id,
            client_secret,
            refresh_token,
            username,
            tmdb_bearer_token,
        })
    }

    /// Carga la configuración solo desde `.env`/variables de entorno,
    /// ignorando el Keychain. Ya no se usa (la importación al Keychain es
    /// tolerante y no requiere que todas las variables estén presentes),
    /// pero se mantiene por retrocompatibilidad de la API interna.
    #[allow(dead_code)]
    pub fn from_env_file_only() -> Result<Self> {
        load_dotenv();

        let client_id = std::env::var("LETTERBOXD_CLIENT_ID")
            .context("LETTERBOXD_CLIENT_ID no está definida")?;
        let client_secret = std::env::var("LETTERBOXD_CLIENT_SECRET").unwrap_or_default();
        let refresh_token = std::env::var("LETTERBOXD_REFRESH_TOKEN")
            .context("LETTERBOXD_REFRESH_TOKEN no está definida")?;
        let username =
            std::env::var("LETTERBOXD_USERNAME").context("LETTERBOXD_USERNAME no está definida")?;
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
