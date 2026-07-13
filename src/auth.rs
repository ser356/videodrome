use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;

const TOKEN_CACHE_FILE: &str = "token.json";
const LETTERBOXD_AUTH_URL: &str = "https://api.letterboxd.com/api/v0/auth/token";

#[derive(Debug, Serialize, Deserialize)]
struct CachedToken {
    access_token: String,
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

fn cache_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("letterboxd-cli");
    std::fs::create_dir_all(&dir).context("No se puede crear el directorio de caché")?;
    Ok(dir.join(TOKEN_CACHE_FILE))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("El tiempo no puede ir hacia atrás")
        .as_secs()
}

fn load_cached_token() -> Option<String> {
    let path = cache_path().ok()?;
    let data = std::fs::read_to_string(path).ok()?;
    let cached: CachedToken = serde_json::from_str(&data).ok()?;
    // Margen de 60 s para no usar un token a punto de caducar
    if now_unix() + 60 < cached.expires_at {
        Some(cached.access_token)
    } else {
        None
    }
}

fn save_token(token: &str, expires_in: u64) -> Result<()> {
    let cached = CachedToken {
        access_token: token.to_string(),
        expires_at: now_unix() + expires_in,
    };
    let path = cache_path()?;
    let json = serde_json::to_string(&cached)?;
    std::fs::write(path, json).context("No se puede guardar el token en caché")?;
    Ok(())
}

pub async fn get_access_token(client: &reqwest::Client, config: &Config) -> Result<String> {
    if let Some(token) = load_cached_token() {
        return Ok(token);
    }

    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", &config.refresh_token),
        ("client_id", &config.client_id),
        ("client_secret", &config.client_secret),
    ];

    let response = client
        .post(LETTERBOXD_AUTH_URL)
        .form(&params)
        .send()
        .await
        .context("Error al llamar al endpoint de autenticación")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Error de autenticación ({status}): {body}");
    }

    let token_resp: TokenResponse = response
        .json()
        .await
        .context("Error al parsear la respuesta del token")?;

    save_token(&token_resp.access_token, token_resp.expires_in)?;

    Ok(token_resp.access_token)
}
