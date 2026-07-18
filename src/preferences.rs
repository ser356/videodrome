//! Preferencias persistentes del usuario que la GUI puede editar en la
//! vista de Ajustes. Vive en `~/.config/videodrome/preferences.json`.
//!
//! Deliberadamente ligero: solo lo que tiene sentido cambiar desde la
//! app (defaults de la vista Recs, idiomas de subs). Todo lo que es
//! configuración de despliegue (Torznab URL/APIKEY, credenciales)
//! sigue por env/Keychain — no queremos filtrar secretos a un JSON de
//! preferencias que también se sincroniza con dotfiles del user.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const PREFERENCES_FILE: &str = "preferences.json";

/// Preferencias del usuario. Cada campo lleva `#[serde(default)]` para
/// que añadir uno nuevo NO invalide los `preferences.json` existentes:
/// los campos ausentes se rellenan con el default individual del campo
/// (`default_min_rating` → 4.0, etc.), no con `Preferences::default()`
/// entera. Sin esto, el primer save después de un update borraba las
/// preferencias del usuario sin avisar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    /// Rating mínimo por defecto en la vista Recs (0.5 – 5.0).
    #[serde(default = "default_min_rating")]
    pub default_min_rating: f32,
    /// Idiomas de subtítulos separados por coma (ISO 639-1). Se pasa a
    /// OpenSubtitles como parámetro `languages`.
    #[serde(default = "default_subtitle_languages")]
    pub subtitle_languages: String,
    /// Días que se conserva la caché de streams antes de purgarse
    /// automáticamente al arrancar la GUI. Cada entrada guarda el
    /// mtime de un fichero `.last_used` dentro de `<hash>/` que se
    /// toca al iniciar/terminar el stream; si excede el TTL se borra
    /// el directorio entero. Rango efectivo 1–365; 0 se trata como 1.
    #[serde(default = "default_stream_cache_ttl_days")]
    pub stream_cache_ttl_days: u32,
    /// Opacidad del "liquid glass" (0..=100). 0 = superficies muy
    /// transparentes (look por defecto), 100 = superficies casi
    /// sólidas para máxima legibilidad. El frontend interpreta este
    /// valor como el peso alfa de un fondo oscuro que se aplica
    /// encima del gradiente de `.glass`/`.glass-strong`/`.popover`.
    #[serde(default = "default_glass_opacity")]
    pub glass_opacity: u8,
    /// Reproductor que la GUI usa por defecto al pulsar Enter/play sobre un torrent.
    /// `Html` usa el player embebido (view `Player.tsx`, requiere
    /// ffmpeg en PATH); `Vlc` mantiene la ruta legacy que abre VLC
    /// como proceso externo. El clic derecho sobre un torrent siempre
    /// ofrece "Abrir en VLC" como escape hatch independientemente de
    /// esta preferencia.
    #[serde(default = "default_player")]
    pub default_player: PlayerKind,
    /// Idioma de la UI (ISO 639-1: `"en"`, `"es"`, `"fr"`, `"de"`,
    /// `"it"`, `"pt"`). `None` = auto-detección en frontend vía
    /// `navigator.language` la primera vez que arranca la app, tras
    /// lo cual se persiste el valor detectado. Los diccionarios
    /// viven en `ui/src/lib/i18n/<code>.ts`.
    ///
    /// Se usa además como PRIMER idioma de preferencia al buscar
    /// subtítulos (§ audit i18n): un user con la app en ES ve subs
    /// españoles arriba aunque su `subtitle_languages` histórica
    /// empezara por "en,es".
    #[serde(default)]
    pub ui_language: Option<String>,
    /// Estrategia de calidad para el pipeline HLS (audit §2/§7).
    ///
    /// * `Auto` (default): intenta `-c:v copy` cuando el probe
    ///   detecta un códec compatible con el cliente y un índice de
    ///   keyframes legible con GOPs razonables (≤10s). Si algo falla,
    ///   cae a transcode. Cero pérdida cuando es posible; pérdida
    ///   controlada (CRF 18) cuando no.
    /// * `Copy`: fuerza `-c:v copy` siempre; si el índice o el
    ///   códec no lo permite, el arranque falla y el user tiene que
    ///   volver a Auto/Transcode. Modo debug/experto.
    /// * `Transcode`: fuerza transcode CRF 18 siempre (el
    ///   comportamiento pre-§2 con quality bump del §5). Útil como
    ///   red de seguridad si copy tiene bugs en algún archivo.
    #[serde(default = "default_quality_mode")]
    pub quality_mode: QualityMode,
    /// Presupuesto de disco (GB) para la caché de segmentos `.ts`
    /// del pipeline HLS. Cuando el tempdir de un stream supera este
    /// tamaño, la evicción LRU borra los segmentos MÁS ALEJADOS del
    /// playhead actual (ambas direcciones, priorizando los de atrás
    /// muy lejanos). Necesario en modo `Copy` porque los segmentos
    /// tienen bitrate del original (una peli UHD de 60 GB deja 60 GB
    /// si se ve entera). En `Transcode` el efecto es más leve pero
    /// la eviction sigue aplicando.
    ///
    /// Un segmento evictado se re-materializa bajo demanda como
    /// cualquier otro; solo se pierde la propiedad "seek atrás
    /// gratis" más allá del presupuesto. Rango efectivo 1–200; 0
    /// se trata como 8 (default).
    #[serde(default = "default_hls_disk_budget_gb")]
    pub hls_disk_budget_gb: u32,
}

/// Estrategia de calidad para el pipeline HLS. Ver
/// [`Preferences::quality_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityMode {
    Auto,
    Copy,
    Transcode,
}

/// Reproductor que la GUI usa por defecto. Serializado como string
/// en el JSON de preferencias para que sea legible a ojo (`"html"` /
/// `"vlc"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerKind {
    Html,
    Vlc,
}

fn default_min_rating() -> f32 {
    4.0
}
fn default_subtitle_languages() -> String {
    crate::subtitles::DEFAULT_LANGUAGES.to_string()
}
fn default_stream_cache_ttl_days() -> u32 {
    7
}
fn default_glass_opacity() -> u8 {
    0
}

fn default_player() -> PlayerKind {
    PlayerKind::Html
}

fn default_quality_mode() -> QualityMode {
    QualityMode::Auto
}

fn default_hls_disk_budget_gb() -> u32 {
    8
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            default_min_rating: default_min_rating(),
            subtitle_languages: default_subtitle_languages(),
            stream_cache_ttl_days: default_stream_cache_ttl_days(),
            glass_opacity: default_glass_opacity(),
            default_player: default_player(),
            ui_language: None,
            quality_mode: default_quality_mode(),
            hls_disk_budget_gb: default_hls_disk_budget_gb(),
        }
    }
}

fn preferences_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("No se puede obtener el directorio de configuración")?
        .join("videodrome");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(PREFERENCES_FILE))
}

pub fn load() -> Preferences {
    let Ok(path) = preferences_path() else {
        return Preferences::default();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return Preferences::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

pub fn save(prefs: &Preferences) -> Result<()> {
    let path = preferences_path()?;
    let json = serde_json::to_string_pretty(prefs).context("Error al serializar preferencias")?;
    std::fs::write(path, json).context("Error al escribir preferences.json")?;
    Ok(())
}
