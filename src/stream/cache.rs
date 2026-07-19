//! Caché persistente de streams: gestión del directorio
//! `<cache>/videodrome/streams/`, tamaños, prune por TTL, barrido de
//! tempdirs huérfanos. Helpers de tiempo compartidos con `resume`.
//!
//! Extraído de `stream.rs` en el refactor (commit paso 2). Sin
//! cambios de comportamiento.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

pub(super) const LAST_USED_FILE: &str = ".last_used";

/// Directorio raíz de la caché de streams:
/// `<dirs::cache_dir>/videodrome/streams/`. Se crea si no existe.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn cache_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .context("No se puede obtener el directorio de caché del sistema")?
        .join("videodrome")
        .join("streams");
    std::fs::create_dir_all(&dir).with_context(|| format!("No se pudo crear {}", dir.display()))?;
    Ok(dir)
}

/// Re-export delgado: la implementación real (con validación de
/// formato) vive en `torrents::parse_infohash`. Existía una copia
/// aquí antes; se unificó para que el cache persistente y el dedupe
/// de providers usen exactamente la misma normalización.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn parse_infohash(magnet: &str) -> Option<String> {
    crate::torrents::parse_infohash(magnet)
}

pub(super) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Actualiza el mtime del sentinel `.last_used` dentro de `dir`. Si no
/// existe lo crea. El prune usa este mtime como "última vez usado".
pub(super) fn touch_last_used(dir: &Path) -> std::io::Result<()> {
    let path = dir.join(LAST_USED_FILE);
    // `File::create` trunca a 0 bytes y actualiza mtime en el proceso.
    std::fs::File::create(&path).map(|_| ())
}

fn entry_last_used(dir: &Path) -> u64 {
    let sentinel = dir.join(LAST_USED_FILE);
    let meta = std::fs::metadata(&sentinel)
        .or_else(|_| std::fs::metadata(dir))
        .ok();
    meta.and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dir_size_bytes(dir: &Path) -> u64 {
    // Recorrido shallow: los torrents de librqbit ponen los ficheros
    // directamente en `dir/`, sin subcarpetas anidadas profundas más
    // allá de una posible carpeta del propio torrent. Un walk iterativo
    // simple sobra.
    //
    // Medimos BLOQUES ASIGNADOS (no `len()` = tamaño lógico). librqbit
    // prealoca cada fichero del torrent a su tamaño final con `set_len`,
    // que en APFS/ext4/btrfs crea ficheros SPARSE: un torrent de 7 GB
    // con el 3 % descargado figura como 7 GB lógicos pero solo ~200 MB
    // en disco. Usar `len()` inflaba la UI de Ajustes ~20× (247 GB
    // reportados vs 12 GB reales medidos con `du -sh`). En NTFS la
    // preasignación de librqbit NO es sparse por defecto, así que
    // `len()` es una aproximación aceptable — ver `file_disk_usage`.
    let mut total = 0u64;
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(iter) = std::fs::read_dir(&p) else {
            continue;
        };
        for entry in iter.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
            } else if let Ok(m) = entry.metadata() {
                total = total.saturating_add(file_disk_usage(&m));
            }
        }
    }
    total
}

/// Uso real en disco de un fichero. En Unix consulta `st_blocks` para
/// obtener bytes físicamente asignados (crítico con ficheros sparse
/// preallocated por librqbit). En Windows cae a `len()`: la
/// preasignación de librqbit no crea sparse por defecto en NTFS, así
/// que el tamaño lógico es una aproximación aceptable. Para exactitud
/// perfecta en Windows habría que llamar a `GetCompressedFileSizeW`
/// via `winapi` — coste no justificado hoy.
#[cfg(unix)]
fn file_disk_usage(md: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    // `st_blocks` SIEMPRE está en unidades de 512 B (POSIX),
    // independiente del block size del filesystem. macOS y Linux lo
    // exponen así; APFS interno usa 4 KiB pero el reporte respeta el
    // contrato POSIX.
    md.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn file_disk_usage(md: &std::fs::Metadata) -> u64 {
    md.len()
}

/// Tamaño total en bytes de la caché.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn total_size() -> u64 {
    let Ok(root) = cache_dir() else {
        return 0;
    };
    dir_size_bytes(&root)
}

/// Borra TODAS las entradas de la caché (equivalente a `rm -rf` del
/// directorio raíz, recreándolo vacío).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn clear_all() -> Result<()> {
    let root = cache_dir()?;
    // No borramos el root en sí: solo su contenido, así siguientes
    // llamadas a `cache_dir()` no fallan por permisos si el directorio
    // padre no es escribible.
    if let Ok(iter) = std::fs::read_dir(&root) {
        for entry in iter.flatten() {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(())
}

/// Purga entradas cuyo `.last_used` sea más viejo que `ttl_days`.
/// Devuelve los bytes liberados. Un TTL de 0 se trata como 1 día (para
/// evitar borrar entradas recién tocadas por un race con el drop del
/// StreamHandle).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn prune(ttl_days: u32) -> Result<u64> {
    let root = cache_dir()?;
    let ttl_secs = (ttl_days.max(1) as u64) * 24 * 3600;
    let cutoff = now_unix().saturating_sub(ttl_secs);
    let mut freed = 0u64;
    let Ok(iter) = std::fs::read_dir(&root) else {
        return Ok(0);
    };
    for entry in iter.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let path = entry.path();
        let last_used = entry_last_used(&path);
        if last_used == 0 || last_used >= cutoff {
            continue;
        }
        let size = dir_size_bytes(&path);
        if std::fs::remove_dir_all(&path).is_ok() {
            freed = freed.saturating_add(size);
        }
    }
    Ok(freed)
}

/// Barre `std::env::temp_dir()` en busca de tempdirs huérfanos con
/// nuestros prefijos (`videodrome-hls-*`, `videodrome-stream-*`) y
/// los borra. Se llama al arranque de la app (main.rs y gui.rs::run).
///
/// Motivo (Fase F del audit Windows): en NTFS no se puede borrar un
/// fichero mientras otro handle lo tiene abierto sin
/// `FILE_SHARE_DELETE`. Cuando el `TempDir::drop` corre mientras
/// ffmpeg / axum tienen aún un `.ts` abierto, el unlink falla en
/// silencio y queda basura en `%TEMP%`. En macOS/Linux el unlink
/// procede aunque haya handles abiertos, así que el problema no
/// aparece — pero el barrido cubre también crashes / SIGKILLs en
/// cualquier SO. Barato y seguro: solo borramos directorios con
/// nuestro prefijo, así que no podemos tocar nada del user.
///
/// No propaga errores: silencioso y best-effort. Devuelve el número
/// de directorios borrados (informativo para logs).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn prune_orphan_tempdirs() -> usize {
    const PREFIXES: &[&str] = &["videodrome-hls-", "videodrome-stream-"];
    let temp = std::env::temp_dir();
    let Ok(iter) = std::fs::read_dir(&temp) else {
        return 0;
    };
    let mut count = 0;
    for entry in iter.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !PREFIXES.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        // best-effort: si otro proceso vivo tiene handles abiertos
        // en NTFS puede fallar; en la siguiente ejecución tocará.
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_infohash_reexports_from_torrents() {
        // El helper de stream.rs debe delegar en torrents::parse_infohash
        // (misma normalización → lowercase, misma validación).
        let hash = parse_infohash("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567");
        assert_eq!(hash.unwrap().len(), 40);
    }
}
