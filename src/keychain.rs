//! Acceso a credenciales guardadas en el Keychain de macOS. En cualquier otro
//! sistema operativo, `get` siempre devuelve `None` y `set`/`delete` fallan
//! con un mensaje claro.
//!
//! Convención en el Keychain: cada credencial es un item genérico con
//! `service` = nombre de la credencial (p.ej. `videodromeent-id`) y
//! `account` = nombre de la app (`videodrome`). Así aparecen agrupadas
//! en Acceso a Llaveros con nombres legibles.

pub const CLIENT_ID: &str = "videodromeent-id";
pub const CLIENT_SECRET: &str = "videodromeent-secret";
pub const REFRESH_TOKEN: &str = "letterboxd-refresh-token";
pub const TMDB_BEARER_TOKEN: &str = "letterboxd-tmdb-bearer-token";
pub const USERNAME: &str = "letterboxd-username";

pub use imp::{delete, get, set};

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::Result;
    use keyring::{Entry, Error as KeyringError};

    const ACCOUNT: &str = "videodrome";

    /// Lee una credencial del Keychain. `None` si no existe **o si el
    /// Keychain no está accesible** (sesión bloqueada, user denegó el
    /// prompt, daemon caído). Ambos casos colapsan al mismo return
    /// value para el caller (fallback a env / credentials.json), pero
    /// el segundo se loguea a `warn` para que quede rastro en el
    /// debug.log — sin esto era imposible diagnosticar por qué el
    /// user "está deslogueado" cuando en realidad denegó el prompt
    /// del keychain.
    pub fn get(service: &str) -> Option<String> {
        match Entry::new(service, ACCOUNT) {
            Ok(entry) => match entry.get_password() {
                Ok(pw) => Some(pw),
                Err(KeyringError::NoEntry) => None,
                Err(e) => {
                    tracing::warn!(
                        target: "keychain",
                        service,
                        error = %e,
                        "get_password failed (acceso denegado / keychain locked)"
                    );
                    None
                }
            },
            Err(e) => {
                tracing::warn!(
                    target: "keychain",
                    service,
                    error = %e,
                    "Entry::new failed"
                );
                None
            }
        }
    }

    pub fn set(service: &str, value: &str) -> Result<()> {
        Entry::new(service, ACCOUNT)?.set_password(value)?;
        Ok(())
    }

    pub fn delete(service: &str) -> Result<()> {
        match Entry::new(service, ACCOUNT)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use anyhow::Result;

    pub fn get(_service: &str) -> Option<String> {
        None
    }

    pub fn set(_service: &str, _value: &str) -> Result<()> {
        anyhow::bail!("El Keychain solo está disponible en macOS")
    }

    pub fn delete(_service: &str) -> Result<()> {
        anyhow::bail!("El Keychain solo está disponible en macOS")
    }
}
