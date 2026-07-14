//! Cliente de OpenSubtitles (REST API v1) para buscar y descargar
//! subtítulos que se le pasan a VLC como `--sub-file=…` al arrancar el
//! stream.
//!
//! Doc: <https://opensubtitles.stoplight.io/docs/opensubtitles-api>
//!
//! Necesita un API key gratuito (con quota: 5 req/s, ~200 descargas/día
//! anónimas). Se bakea en el binario al compilar con
//! `LB_APP_OS_API_KEY=xxx cargo install --path .`, o se puede definir la
//! env var `OPENSUBTITLES_API_KEY` en runtime.
//!
//! El match "edición correcta" (BluRay ↔ BluRay, WEB-DL ↔ WEB-DL...) se
//! consigue pasando el título del torrent como `query`: OpenSubtitles
//! rankea por similitud del `release name`, así que la primera entrada
//! suele ser exactamente la misma edición cuando la hay.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

const API_BASE: &str = "https://api.opensubtitles.com/api/v1";

/// API key hardcoded en el source (source build para amigos). Se rota
/// aquí si algún día se abusa.
const BAKED_OS_API_KEY: Option<&str> = Some("BGtS90uaAB0s7LndtE3kqmusBpcLv4ir");

/// User-Agent requerido por OpenSubtitles (si no lo mandas te banean).
const USER_AGENT: &str = concat!("letterboxd-cli v", env!("CARGO_PKG_VERSION"));

/// Idiomas por defecto que se piden a OpenSubtitles.
pub const DEFAULT_LANGUAGES: &str = "es,en,fr,de,it";

/// Subtítulo devuelto por la búsqueda.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Subtitle {
    /// ID interno de OpenSubtitles del `file` (no del release). Es lo que
    /// se manda a `POST /download`.
    pub file_id: u64,
    /// ISO 639-1 (`"es"`, `"en"`...).
    pub language: String,
    /// Nombre del release al que este sub está sincronizado
    /// (ej. `"Funny.Games.2007.1080p.BluRay.x264-CLASSiC"`).
    pub release: String,
    /// Cuántas veces se ha descargado — es el proxy de calidad más útil.
    pub downloads: u64,
    /// Rating de la comunidad de OpenSubtitles.
    pub rating: f32,
    /// Sub con transcripción para sordos (SDH).
    pub hearing_impaired: bool,
    /// Nombre del fichero (`foo.srt`).
    pub file_name: Option<String>,
}

/// Devuelve la API key resuelta (env var > baked-in).
pub fn api_key() -> Option<String> {
    std::env::var("OPENSUBTITLES_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| BAKED_OS_API_KEY.map(|s| s.to_string()))
}

/// True si tenemos alguna API key configurada. La TUI la usa para saber
/// si mostrar el atajo "x — subtítulos" o no.
pub fn is_available() -> bool {
    api_key().is_some()
}

/// Busca subtítulos para una película. Al menos uno de `imdb_id` o
/// `query` debe estar informado.
///
/// * `imdb_id` — IMDb ID (`ttXXXXXXX` o solo el número). Filtra a la
///   película exacta.
/// * `query` — texto libre. Cuando es el `release name` del torrent,
///   OpenSubtitles rankea los subs por parecido al release → primer
///   resultado ≈ edición correcta.
/// * `languages` — coma-separado, ej. `"es,en,fr"`. Vacío = todos.
pub async fn search(
    http: &Client,
    imdb_id: Option<&str>,
    query: Option<&str>,
    languages: &str,
) -> Result<Vec<Subtitle>> {
    let key = api_key().context("No hay OPENSUBTITLES_API_KEY (ni bakeada ni en env)")?;

    // Construimos la query manualmente en el mismo orden que la doc para
    // que la cache-key del server no varíe entre llamadas equivalentes.
    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(id) = imdb_id {
        let n = id.trim_start_matches("tt");
        params.push(("imdb_id", n.to_string()));
    }
    if let Some(q) = query {
        params.push(("query", q.to_string()));
    }
    if !languages.is_empty() {
        params.push(("languages", languages.to_string()));
    }
    // Ordena por descargas: los subs más usados van primero.
    params.push(("order_by", "download_count".to_string()));
    params.push(("order_direction", "desc".to_string()));

    let resp = http
        .get(format!("{API_BASE}/subtitles"))
        .header("Api-Key", &key)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .query(&params)
        .send()
        .await
        .context("Error de red hablando con OpenSubtitles")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenSubtitles /subtitles devolvió {status}: {body}");
    }

    let json: SearchResponse = resp
        .json()
        .await
        .context("Respuesta de OpenSubtitles no parseable como JSON")?;

    Ok(json.data.into_iter().filter_map(parse_item).collect())
}

/// Pide a OpenSubtitles el link de descarga temporal (`POST /download`) y
/// baja el `.srt` a `dest_dir`. Devuelve la ruta local del fichero.
///
/// La descarga consume una unidad de la quota diaria del API key (~200/día
/// anónima). El link es de un solo uso y expira rápido, así que hacemos
/// el GET inmediatamente después del POST.
pub async fn download(http: &Client, sub: &Subtitle, dest_dir: &Path) -> Result<PathBuf> {
    let key = api_key().context("No hay OPENSUBTITLES_API_KEY (ni bakeada ni en env)")?;

    let dl: DownloadResponse = http
        .post(format!("{API_BASE}/download"))
        .header("Api-Key", &key)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .json(&serde_json::json!({ "file_id": sub.file_id }))
        .send()
        .await
        .context("Error pidiendo el link de descarga a OpenSubtitles")?
        .error_for_status()
        .context("OpenSubtitles /download devolvió error HTTP")?
        .json()
        .await
        .context("Respuesta de /download no parseable")?;

    let bytes = http
        .get(&dl.link)
        .send()
        .await
        .context("Error descargando el .srt")?
        .error_for_status()?
        .bytes()
        .await?;

    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("No se pudo crear {}", dest_dir.display()))?;

    // Prefiere el file_name real del sub (mantiene idioma en el nombre);
    // fallback: el que devuelve la API; último recurso: `subs-<id>.srt`.
    let name = sub
        .file_name
        .clone()
        .or(dl.file_name)
        .unwrap_or_else(|| format!("subs-{}.srt", sub.file_id));
    let path = dest_dir.join(sanitize_filename(&name));
    std::fs::write(&path, &bytes)
        .with_context(|| format!("No se pudo escribir {}", path.display()))?;

    Ok(path)
}

/// Reemplaza caracteres problemáticos en el filename (los subs vienen a
/// veces con `/`, `:`, etc. que rompen el path).
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

fn parse_item(it: SearchItem) -> Option<Subtitle> {
    let a = it.attributes;
    let file = a.files.into_iter().next()?;
    Some(Subtitle {
        file_id: file.file_id,
        language: a.language.unwrap_or_default(),
        release: a.release.unwrap_or_default(),
        downloads: a.download_count.unwrap_or(0),
        rating: a.ratings.unwrap_or(0.0),
        hearing_impaired: a.hearing_impaired.unwrap_or(false),
        file_name: file.file_name,
    })
}

// ---------- shapes de la API ----------

#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    attributes: Attrs,
}

#[derive(Deserialize)]
struct Attrs {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    download_count: Option<u64>,
    #[serde(default)]
    ratings: Option<f32>,
    #[serde(default)]
    hearing_impaired: Option<bool>,
    #[serde(default)]
    files: Vec<FileRef>,
}

#[derive(Deserialize)]
struct FileRef {
    file_id: u64,
    #[serde(default)]
    file_name: Option<String>,
}

#[derive(Deserialize)]
struct DownloadResponse {
    link: String,
    #[serde(default)]
    file_name: Option<String>,
}
