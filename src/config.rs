use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    pub username: String,
    pub tmdb_bearer_token: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // Buscar .env en ~/.config/letterboxd-cli/ (funciona en macOS y Linux)
        if let Some(home) = dirs::home_dir() {
            let env_path = home.join(".config").join("letterboxd-cli").join(".env");
            if env_path.exists() {
                dotenvy::from_path(&env_path).ok();
            }
        }
        // Fallback: .env en el directorio de trabajo actual (desarrollo)
        dotenvy::dotenv().ok();

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
