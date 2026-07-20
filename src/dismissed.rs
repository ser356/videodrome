//! Descartes ("no sugerir") persistidos por el usuario desde la GUI.
//!
//! Vive en `~/.config/videodrome/dismissed.json`. Guardamos title +
//! poster_path junto al TMDB id para poder pintar el panel de "Restaurar"
//! en Ajustes sin tener que refetchar TMDB por cada entrada descartada.
//!
//! Solo se usa desde el backend GUI (`#[cfg(feature = "gui")]`); el CLI
//! y la TUI no filtran por esto — mantienen el comportamiento clásico.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

const DISMISSED_FILE: &str = "dismissed.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DismissedEntry {
    pub id: u64,
    pub title: String,
    pub poster_path: Option<String>,
    /// Epoch UNIX en segundos.
    pub dismissed_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Dismissed {
    #[serde(default)]
    pub entries: Vec<DismissedEntry>,
}

impl Dismissed {
    pub fn ids(&self) -> HashSet<u64> {
        self.entries.iter().map(|e| e.id).collect()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// Añade una entrada si no existía; no-op si ya estaba.
    pub fn insert(&mut self, entry: DismissedEntry) {
        if !self.contains(entry.id) {
            self.entries.push(entry);
        }
    }

    /// Elimina por id. Devuelve `true` si estaba presente.
    pub fn remove(&mut self, id: u64) -> bool {
        let len = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != len
    }
}

fn dismissed_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("videodrome");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(DISMISSED_FILE))
}

pub fn load() -> Dismissed {
    let Ok(path) = dismissed_path() else {
        return Dismissed::default();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return Dismissed::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

pub fn save(d: &Dismissed) -> Result<()> {
    let path = dismissed_path()?;
    let json = serde_json::to_string_pretty(d).context("Error al serializar dismissed.json")?;
    std::fs::write(path, json).context("Error al escribir dismissed.json")?;
    Ok(())
}

/// Vacía el store por completo. Alimenta el botón "Vaciar" de Ajustes
/// (símetrico al de `watched::clear`).
pub fn clear() -> Result<()> {
    save(&Dismissed::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64) -> DismissedEntry {
        DismissedEntry {
            id,
            title: format!("Movie {id}"),
            poster_path: Some(format!("/p{id}.jpg")),
            dismissed_at: 1000 + id,
        }
    }

    #[test]
    fn insert_adds_new_entry() {
        let mut d = Dismissed::default();
        d.insert(entry(1));
        assert_eq!(d.entries.len(), 1);
        assert!(d.contains(1));
    }

    #[test]
    fn insert_is_idempotent() {
        let mut d = Dismissed::default();
        d.insert(entry(1));
        d.insert(entry(1));
        assert_eq!(d.entries.len(), 1);
    }

    #[test]
    fn insert_preserves_first_entry_metadata() {
        // Re-insertar con el mismo id NO sobrescribe (por diseño —
        // el user descartó UNA vez; si vuelve a hacerlo, no importa).
        let mut d = Dismissed::default();
        d.insert(entry(1));
        let mut other = entry(1);
        other.title = "Different".into();
        d.insert(other);
        assert_eq!(d.entries[0].title, "Movie 1");
    }

    #[test]
    fn remove_returns_true_when_present() {
        let mut d = Dismissed::default();
        d.insert(entry(1));
        d.insert(entry(2));
        assert!(d.remove(1));
        assert_eq!(d.entries.len(), 1);
        assert!(!d.contains(1));
        assert!(d.contains(2));
    }

    #[test]
    fn remove_returns_false_when_absent() {
        let mut d = Dismissed::default();
        d.insert(entry(1));
        assert!(!d.remove(99));
        assert_eq!(d.entries.len(), 1);
    }

    #[test]
    fn ids_returns_set() {
        let mut d = Dismissed::default();
        d.insert(entry(1));
        d.insert(entry(2));
        d.insert(entry(3));
        let ids = d.ids();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(ids.contains(&3));
    }

    #[test]
    fn json_roundtrip_preserves_entries() {
        let mut d = Dismissed::default();
        d.insert(entry(42));
        let json = serde_json::to_string(&d).unwrap();
        let back: Dismissed = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].id, 42);
        assert_eq!(back.entries[0].title, "Movie 42");
    }

    #[test]
    fn json_deserializes_empty_object_as_default() {
        let d: Dismissed = serde_json::from_str("{}").unwrap();
        assert!(d.entries.is_empty());
    }
}
