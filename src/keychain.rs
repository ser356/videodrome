//! Acceso a credenciales guardadas en el Keychain de macOS. En cualquier otro
//! sistema operativo, `get` siempre devuelve `None` (fallback silencioso a
//! `.env`) y `set`/`delete` fallan con un mensaje claro.

pub const CLIENT_ID: &str = "letterboxd_client_id";
pub const CLIENT_SECRET: &str = "letterboxd_client_secret";
pub const REFRESH_TOKEN: &str = "letterboxd_refresh_token";
pub const TMDB_BEARER_TOKEN: &str = "tmdb_bearer_token";

pub use imp::{delete, get, set};

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::Result;
    use keyring::{Entry, Error as KeyringError};

    const SERVICE: &str = "letterboxd-cli";

    /// Lee una credencial del Keychain. `None` si no existe o si el Keychain
    /// no está accesible (por ejemplo, sesión sin desbloquear).
    pub fn get(account: &str) -> Option<String> {
        Entry::new(SERVICE, account).ok()?.get_password().ok()
    }

    pub fn set(account: &str, value: &str) -> Result<()> {
        Entry::new(SERVICE, account)?.set_password(value)?;
        Ok(())
    }

    pub fn delete(account: &str) -> Result<()> {
        match Entry::new(SERVICE, account)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use anyhow::Result;

    pub fn get(_account: &str) -> Option<String> {
        None
    }

    pub fn set(_account: &str, _value: &str) -> Result<()> {
        anyhow::bail!("El Keychain solo está disponible en macOS")
    }

    pub fn delete(_account: &str) -> Result<()> {
        anyhow::bail!("El Keychain solo está disponible en macOS")
    }
}
