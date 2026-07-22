use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::credentials;

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
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Respuesta del login con usuario/contraseña. La app la usa para
/// persistir el `refresh_token` en `credentials.json`.
#[derive(Debug, Clone)]
pub struct LoginResult {
    pub refresh_token: String,
    #[allow(dead_code)]
    pub access_token: String,
    #[allow(dead_code)]
    pub expires_in: u64,
}

fn cache_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("videodrome");
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

/// Borra el token de acceso cacheado en disco. Se llama desde `logout`
/// para que el próximo `get_access_token` no devuelva un token todavía
/// válido de la sesión anterior.
#[allow(dead_code)]
pub fn clear_cached_token() -> Result<()> {
    let path = cache_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("No se pudo borrar {}", path.display()))?;
    }
    Ok(())
}

fn save_token(token: &str, expires_in: u64) -> Result<()> {
    let cached = CachedToken {
        access_token: token.to_string(),
        expires_at: now_unix() + expires_in,
    };
    let path = cache_path()?;
    let json = serde_json::to_string(&cached)?;

    // Escritura atómica: write en `.tmp` + rename. Sin esto un crash a
    // mitad de write deja el fichero corrupto y `serde_json::from_str`
    // falla al arrancar la siguiente sesión → login zombie.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).context("No se puede escribir el token temporal")?;

    // 0o600 antes del rename para que la ventana de "fichero legible por
    // otros users del sistema" no exista nunca. `set_permissions` es
    // best-effort — si falla continuamos y el rename sigue adelante
    // (peor caso: se lee con 644 durante microsegundos).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }

    std::fs::rename(&tmp, &path).context("No se puede renombrar el token en caché")?;
    Ok(())
}

pub async fn get_access_token(client: &reqwest::Client, config: &Config) -> Result<String> {
    // Si el user hizo logout el refresh_token en config es None; en ese
    // caso NO queremos devolver un token viejo cacheado — la sesión debe
    // considerarse cerrada.
    if config.refresh_token.is_none() {
        anyhow::bail!("No hay refresh_token — el usuario debe hacer login primero");
    }

    if let Some(token) = load_cached_token() {
        return Ok(token);
    }

    let refresh_token = config
        .refresh_token
        .as_deref()
        .context("No hay refresh_token — el usuario debe hacer login primero")?;

    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
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

    // Letterboxd puede rotar el refresh_token en cada refresh. Si viene
    // uno nuevo lo persistimos en credentials.json — si no, el user
    // acaba expulsado silenciosamente cuando el viejo caduque.
    if let Some(new_refresh) = token_resp.refresh_token.as_deref() {
        if new_refresh != refresh_token && !new_refresh.is_empty() {
            let _ = credentials::update_refresh_token(new_refresh);
        }
    }

    Ok(token_resp.access_token)
}

/// Login con usuario y contraseña usando `grant_type=password` (soportado
/// por la API de Letterboxd). Devuelve el refresh_token para persistirlo
/// en `credentials.json`.
pub async fn login_with_password(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    username: &str,
    password: &str,
) -> Result<LoginResult> {
    let params = [
        ("grant_type", "password"),
        ("username", username),
        ("password", password),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ];

    let response = client
        .post(LETTERBOXD_AUTH_URL)
        .form(&params)
        .send()
        .await
        .context("Error al llamar al endpoint de login")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Letterboxd suele devolver un JSON `{"error":"invalid_grant","error_description":"..."}`
        // — intentamos extraer el motivo para mostrarlo en la TUI.
        if let Ok(err) = serde_json::from_str::<serde_json::Value>(&body) {
            let msg = err
                .get("error_description")
                .and_then(|v| v.as_str())
                .or_else(|| err.get("error").and_then(|v| v.as_str()))
                .unwrap_or("error desconocido");
            anyhow::bail!("Login rechazado ({status}): {msg}");
        }
        anyhow::bail!("Login rechazado ({status}): {body}");
    }

    let token_resp: TokenResponse = response
        .json()
        .await
        .context("Error al parsear la respuesta de login")?;

    let refresh = token_resp
        .refresh_token
        .context("La respuesta de login no incluyó refresh_token")?;

    // Aprovechamos el access_token para no tener que renovarlo justo
    // después del login.
    save_token(&token_resp.access_token, token_resp.expires_in).ok();

    Ok(LoginResult {
        refresh_token: refresh,
        access_token: token_resp.access_token,
        expires_in: token_resp.expires_in,
    })
}
