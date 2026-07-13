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

/// Busca una credencial primero en el Keychain (macOS) y, si no está, en las
/// variables de entorno / `.env`.
fn resolve(env_key: &str, keychain_account: &str) -> Option<String> {
    keychain::get(keychain_account).or_else(|| std::env::var(env_key).ok())
}

impl Config {
    /// Carga la configuración para uso normal de la app: cada credencial se
    /// busca primero en el Keychain de macOS y, si no está ahí, en `.env`.
    pub fn from_env() -> Result<Self> {
        load_dotenv();

        let client_id = resolve("LETTERBOXD_CLIENT_ID", keychain::CLIENT_ID)
            .context("LETTERBOXD_CLIENT_ID no está definida (ni en el Keychain ni en .env)")?;
        let client_secret =
            resolve("LETTERBOXD_CLIENT_SECRET", keychain::CLIENT_SECRET).unwrap_or_default();
        let refresh_token = resolve("LETTERBOXD_REFRESH_TOKEN", keychain::REFRESH_TOKEN)
            .context("LETTERBOXD_REFRESH_TOKEN no está definida (ni en el Keychain ni en .env)")?;
        let username =
            std::env::var("LETTERBOXD_USERNAME").context("LETTERBOXD_USERNAME no está definida")?;
        let tmdb_bearer_token = resolve("TMDB_BEARER_TOKEN", keychain::TMDB_BEARER_TOKEN)
            .context("TMDB_BEARER_TOKEN no está definida (ni en el Keychain ni en .env)")?;

        Ok(Self {
            client_id,
            client_secret,
            refresh_token,
            username,
            tmdb_bearer_token,
        })
    }

    /// Carga la configuración solo desde `.env`/variables de entorno,
    /// ignorando el Keychain. La usa `keychain import` para saber qué guardar.
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
