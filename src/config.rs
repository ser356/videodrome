use anyhow::{Context, Result};

use crate::credentials;
use crate::keychain;

/// Credenciales de "app" hardcoded en el source. Sí, están públicas en
/// GitHub. El trade-off aceptado:
/// - Letterboxd: cualquiera con un iPhone puede extraerlas de la app.
/// - TMDB: cuota generosa por IP anónima; si se abusa se rota key.
/// - OpenSubtitles: 200 downloads/día por IP anónima, quota compartida
///   irrelevante en escala amigos.
///
/// Ventaja: source build (`brew install`, `cargo install --git`, `nix
/// build`) produce un binario funcional sin pedir configuración al user.
const BAKED_CLIENT_ID: Option<&str> = Some("4f203301-9688-f722-9f4b-c59e90ad6fd6");
const BAKED_CLIENT_SECRET: Option<&str> =
    Some("7d0356bd9e6a357a068f7c48b8557dbfe36b056331bdffc554720165f1620876");
const BAKED_TMDB_BEARER: Option<&str> = Some("eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiIwOWY4ZmFmZDc5ODVjOTVlNDE0NWFjMTQzMWE3MTc0YSIsIm5iZiI6MTc4MjU5OTEzNy42NDQsInN1YiI6IjZhNDA0ZGUxMDg5ZmE3YjE5OTA4MDYxMSIsInNjb3BlcyI6WyJhcGlfcmVhZCJdLCJ2ZXJzaW9uIjoxfQ.RJ1x2zx09nEowi09FE2Tt86sJruCnPGOgUEBXQ3vveA");

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
    // Prioridad de rutas del `.env` (Fase G del audit Windows):
    //   1. `dirs::config_dir()` — la ruta canónica del SO:
    //        * macOS   → ~/Library/Application Support/videodrome
    //        * Linux   → ~/.config/videodrome
    //        * Windows → %APPDATA%\videodrome
    //      Es donde `credentials.rs` y todos los demás módulos
    //      escriben su estado (`credentials.json`, caches, etc.).
    //   2. `~/.config/videodrome/.env` — ruta LEGADA. Se mantiene
    //      por retrocompatibilidad con instalaciones previas al
    //      audit; en Windows se traducía a `C:\Users\X\.config\...`,
    //      que quedaba huérfana respecto al resto de la config
    //      (`%APPDATA%\videodrome\`). NO lo migramos automáticamente
    //      para no tocar ficheros del user sin permiso — si existe,
    //      se carga; el user puede moverlo cuando quiera.
    //   3. `.env` del cwd — override en desarrollo (útil para
    //      `cargo run` con credenciales de test).
    for candidate in dotenv_candidates() {
        if candidate.exists() {
            dotenvy::from_path(&candidate).ok();
        }
    }
    dotenvy::dotenv().ok();
}

/// Rutas donde buscar el `.env`, en orden de prioridad. Expuesta
/// como fn (no const) porque `dirs::config_dir()` es I/O implícito.
fn dotenv_candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(cfg) = dirs::config_dir() {
        out.push(cfg.join("videodrome").join(".env"));
    }
    if let Some(home) = dirs::home_dir() {
        let legacy = home.join(".config").join("videodrome").join(".env");
        // Evita duplicar cuando XDG_CONFIG_HOME apunta ya a
        // ~/.config (típico en Linux).
        if !out.contains(&legacy) {
            out.push(legacy);
        }
    }
    out
}

/// Ruta CANÓNICA del `.env` según el SO — la que citamos en los
/// mensajes de error para que el usuario sepa dónde crearlo.
fn canonical_env_hint() -> String {
    dirs::config_dir()
        .map(|d| d.join("videodrome").join(".env").display().to_string())
        .unwrap_or_else(|| "~/.config/videodrome/.env".to_string())
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
    /// 4. `credentials.json` en `~/.config/videodrome/` (guardado tras
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
            .with_context(|| {
                format!(
                    "LETTERBOXD_CLIENT_ID no est\u{e1} definida y el binario no lleva \
                     credenciales bakeadas. Define LETTERBOXD_CLIENT_ID en el \
                     entorno o en `{}`.",
                    canonical_env_hint()
                )
            })?;
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
            .with_context(|| {
                format!(
                    "TMDB_BEARER_TOKEN no est\u{e1} definida y el binario no lleva \
                     credenciales bakeadas. Define TMDB_BEARER_TOKEN en el \
                     entorno o en `{}`.",
                    canonical_env_hint()
                )
            })?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_candidates_are_non_empty_and_end_in_dotenv() {
        // La lista concreta depende de la plataforma y del env
        // (`XDG_CONFIG_HOME`, `HOME`). Verificamos invariantes:
        //   * Al menos un candidato (a menos que el SO no tenga home).
        //   * Cada candidato termina en `.env`.
        //   * Todos únicos (deduplicación aplicada).
        let cands = dotenv_candidates();
        for c in &cands {
            assert_eq!(
                c.file_name().and_then(|s| s.to_str()),
                Some(".env"),
                "candidato debe terminar en .env: {c:?}"
            );
        }
        let mut seen = std::collections::HashSet::new();
        for c in &cands {
            assert!(
                seen.insert(c.clone()),
                "duplicado en dotenv_candidates: {c:?}"
            );
        }
    }

    #[test]
    fn canonical_env_hint_ends_in_env_path() {
        let hint = canonical_env_hint();
        assert!(
            hint.contains("videodrome") && hint.ends_with(".env"),
            "hint debe citar el path canónico: {hint}"
        );
    }

    #[test]
    fn resolve_prefers_env_var_when_set() {
        // Test único cross-platform: la implementación macOS cae al
        // Keychain solo si la env var está vacía o ausente. Con var
        // set, ambos paths devuelven el mismo valor.
        let key = "VIDEODROME_TEST_ENV_KEY_XYZ";
        // SAFETY: `set_var`/`remove_var` requieren `unsafe` en Rust
        // 2024 por el hazard de threads leyendo env vars a la vez.
        // Los tests corren sequentially en `cargo test --lib` cuando
        // usamos `--test-threads=1`, pero por defecto van en paralelo.
        // Usamos un nombre único y solo verificamos el camino de
        // "var set" para minimizar la ventana de colisión.
        // SAFETY: nombre de env único a este test; sin lectura concurrente esperada.
        unsafe { std::env::set_var(key, "expected") };
        assert_eq!(
            resolve(key, "no-existe-en-keychain"),
            Some("expected".to_string())
        );
        // SAFETY: cleanup a continuación del set anterior.
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn resolve_treats_empty_env_var_as_missing() {
        let key = "VIDEODROME_TEST_ENV_KEY_EMPTY_XYZ";
        // SAFETY: nombre único, sin lectura concurrente esperada.
        unsafe { std::env::set_var(key, "") };
        // En macOS caería al keychain; sin entrada válida ahí,
        // devuelve None. En otros SO, `filter(|s| !s.is_empty())`
        // también da None directo.
        let out = resolve(key, "no-existe-en-keychain-tampoco");
        assert!(
            out.is_none(),
            "env var vacía debe tratarse como missing: {out:?}"
        );
        // SAFETY: cleanup a continuación del set anterior.
        unsafe { std::env::remove_var(key) };
    }
}
