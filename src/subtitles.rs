//! Cliente de OpenSubtitles (REST API v1) para buscar y descargar
//! subtítulos que se le pasan a VLC como `--sub-file=…` al arrancar el
//! stream.
//!
//! Doc: <https://opensubtitles.stoplight.io/docs/opensubtitles-api>
//!
//! Necesita un API key gratuito (con quota: 5 req/s, ~200 descargas/día
//! anónimas). En builds distribuidos va bakeada en el binario; para
//! builds locales / desarrolladores se puede definir la env var
//! `OPENSUBTITLES_API_KEY` en runtime.
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
const USER_AGENT: &str = concat!("videodrome v", env!("CARGO_PKG_VERSION"));

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
    /// Marcado por OpenSubtitles como "from trusted uploader" — subs
    /// verificados por moderadores. Suben en el ranking.
    #[serde(default)]
    pub from_trusted: bool,
    /// Nombre del fichero (`foo.srt`).
    pub file_name: Option<String>,
    /// True cuando este sub proviene de una búsqueda por `moviehash`
    /// (sincronización perfecta con el fichero de vídeo exacto). Lo
    /// rellena el caller (`gui::search_subtitles`) tras marcar los
    /// resultados de la vía hash, ANTES de fusionarlos con los de la
    /// vía `imdb_id`/`query`. Se usa para dedup estable (los
    /// hash-matches se conservan primero) y — opcionalmente — para
    /// pintar un badge en el frontend.
    #[serde(default)]
    pub hash_match: bool,
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

/// Calcula el "OpenSubtitles hash" de un fichero: identificador único
/// del contenido usado por OpenSubtitles para encontrar subs con
/// sincronización perfecta.
///
/// Algoritmo (spec oficial):
///   hash = filesize
///   hash += sum de los primeros 64 KB, leídos como u64 LE, wrapping
///   hash += sum de los últimos 64 KB, leídos como u64 LE, wrapping
///
/// Devuelve 16 hex chars lowercase (u64 → hex zero-padded).
///
/// Cuando OpenSubtitles tiene subs indexados con hash matching, los
/// devuelve como perfect match — sincronización garantizada. Es EL
/// método que usan VLC, mpv y Stremio.
///
/// El caller debe pasar `first_64k` y `last_64k` ya leídos: en el
/// contexto de videodrome son los primeros y últimos 64 KB del fichero
/// dentro del torrent librqbit. Ambos buffers DEBEN ser exactamente
/// 65536 bytes; si `file_len < 131072` (menos de 128 KB) no tiene
/// sentido calcular hash — pass `None` al caller.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn compute_moviehash(file_len: u64, first_64k: &[u8], last_64k: &[u8]) -> Option<String> {
    const CHUNK: usize = 65536;
    if first_64k.len() != CHUNK || last_64k.len() != CHUNK {
        return None;
    }
    let mut hash: u64 = file_len;
    for chunk in first_64k.chunks_exact(8) {
        let v = u64::from_le_bytes(chunk.try_into().ok()?);
        hash = hash.wrapping_add(v);
    }
    for chunk in last_64k.chunks_exact(8) {
        let v = u64::from_le_bytes(chunk.try_into().ok()?);
        hash = hash.wrapping_add(v);
    }
    Some(format!("{hash:016x}"))
}

/// Busca subtítulos para una película o episodio de serie. Al menos
/// uno de `imdb_id`, `moviehash` o `query` debe estar informado.
///
/// * `moviehash` — hash OpenSubtitles del fichero (16 hex chars). Es
///   el filtro más preciso posible: identifica el fichero exacto
///   (bytes) y OpenSubtitles devuelve solo subs cuyo hash indexado
///   coincide → sincronización perfecta con el release. Cuando lo
///   tenemos, es unívoco y no necesitamos imdb_id ni query.
/// * `imdb_id` — IMDb ID (`ttXXXXXXX` o solo el número). Filtra a la
///   película exacta. Para series episódicas, este imdb_id debe ser
///   el de la SERIE (parent), no el del episodio — se combina con
///   `season`/`episode` para direccionar el episodio concreto.
/// * `query` — texto libre. Cuando es el `release name` del torrent,
///   OpenSubtitles rankea los subs por parecido al release → primer
///   resultado ≈ edición correcta.
/// * `season` + `episode` — para subs de un episodio de serie.
///   Cuando `imdb_id` está presente, se envían como triplete
///   (`parent_imdb_id + season_number + episode_number`) — son un
///   único selector lógico, no filtros AND independientes. Sin
///   `imdb_id` se usa como refuerzo del `query`.
/// * `languages` — coma-separado, ej. `"es,en,fr"`. Vacío = todos.
///
/// Prioridad: `moviehash` > `parent_imdb + S + E` > `imdb_id` > `query`.
/// Solo el primero presente se envía como filtro principal (evita
/// AND estricto que descartaría subs válidos).
pub async fn search(
    http: &Client,
    moviehash: Option<&str>,
    imdb_id: Option<&str>,
    query: Option<&str>,
    season: Option<u16>,
    episode: Option<u16>,
    languages: &str,
) -> Result<Vec<Subtitle>> {
    let key = api_key().context("No hay OPENSUBTITLES_API_KEY (ni bakeada ni en env)")?;

    // Construimos la query manualmente en el mismo orden que la doc para
    // que la cache-key del server no varíe entre llamadas equivalentes.
    //
    // IMPORTANTE: enviamos SOLO UNO de los filtros PRINCIPALES en
    // orden de precisión. OpenSubtitles hace AND estricto entre
    // varios: si combinamos imdb_id + query, filtraría los subs cuyo
    // `release` NO matchee la query, aunque el imdb_id sea correcto.
    //
    // Excepción documentada: `parent_imdb_id + season_number +
    // episode_number` SÍ van juntos porque son un único selector
    // lógico ("este episodio de esta serie"), no filtros
    // independientes. Es la ruta canónica para subs de episodios.
    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(h) = moviehash.filter(|s| !s.is_empty()) {
        params.push(("moviehash", h.to_lowercase()));
    } else if let (Some(id), Some(s), Some(e)) = (imdb_id, season, episode) {
        let n = id.trim_start_matches("tt");
        params.push(("parent_imdb_id", n.to_string()));
        params.push(("season_number", s.to_string()));
        params.push(("episode_number", e.to_string()));
    } else if let Some(id) = imdb_id {
        let n = id.trim_start_matches("tt");
        params.push(("imdb_id", n.to_string()));
    } else if let Some(q) = query {
        // Sin imdb pero con S/E: enriquecemos el query textual con
        // "SxxEyy" para que el ranker de OpenSubtitles prefiera subs
        // del episodio pedido. Sin S/E se queda como estaba.
        let q_full = match (season, episode) {
            (Some(s), Some(e)) => format!("{} S{:02}E{:02}", q, s, e),
            _ => q.to_string(),
        };
        params.push(("query", q_full));
    }
    if !languages.is_empty() {
        params.push(("languages", languages.to_string()));
    }
    // Filtros anti-basura a nivel API:
    //
    //   * `machine_translated=exclude` → fuera los subs auto-traducidos
    //     con Google Translate. Calidad terrible, casi ilegibles.
    //
    // NO excluimos `ai_translated`: los subs traducidos con LLM son
    // razonablemente buenos y muchas veces son la ÚNICA opción en
    // idiomas menores (español para cine europeo obscuro, etc.).
    // Preferimos mostrarlos que quedarnos sin subs.
    //
    // Deliberadamente NO usamos `type=movie` — probamos y filtraba a
    // cero para pelis antiguas / ediciones Criterion mal categorizadas
    // en el catálogo. El caso "episodio de serie con mismo título" es
    // raro y prefiero que se cuele un episodio a que una peli
    // aparezca sin subs disponibles.
    params.push(("machine_translated", "exclude".to_string()));
    // Ordena por descargas: los subs más usados van primero.
    params.push(("order_by", "download_count".to_string()));
    params.push(("order_direction", "desc".to_string()));

    // Paginación. OpenSubtitles REST v1 devuelve 50 resultados por
    // página; sin este bucle nos quedábamos siempre en 50 (aunque
    // hubiera 300+ subs indexados para la peli).
    //
    // La primera petición se manda sin `page` explícito (equivalente a
    // page=1) y consulta `total_pages` de la respuesta. Si hay más,
    // seguimos pidiendo hasta agotar o alcanzar el tope.
    //
    // Tope duro (`MAX_PAGES`) para no acercarse al rate-limit del API
    // (5 req/s anónimo) ni disparar la latencia total: con 4 páginas
    // cubrimos ~200 subs, más que suficiente incluso para pelis muy
    // populares (una vez ordenadas por trusted+downloads los primeros
    // 200 son mucho mejores que la cola). La paginación NO consume
    // quota de descarga (esa solo la gasta POST /download).
    const MAX_PAGES: u32 = 4;

    let mut all_items: Vec<SearchItem> = Vec::new();
    let mut page: u32 = 1;
    loop {
        // Clonamos los params base y añadimos `page=n` solo para p>1;
        // así la primera request queda idéntica a la de siempre (y
        // OpenSubtitles la cachea con la misma cache-key).
        let mut page_params = params.clone();
        if page > 1 {
            page_params.push(("page", page.to_string()));
        }

        let resp = http
            .get(format!("{API_BASE}/subtitles"))
            .header("Api-Key", &key)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json")
            .query(&page_params)
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

        // `total_pages=0` en respuestas vacías o si el servidor no lo
        // envía; con `max(1)` evitamos loops infinitos y con el clamp
        // por MAX_PAGES respetamos el tope duro.
        let total_pages = json.total_pages.clamp(1, MAX_PAGES);
        all_items.extend(json.data);

        if page >= total_pages {
            break;
        }
        page += 1;
    }

    let mut subs: Vec<Subtitle> = all_items.into_iter().filter_map(parse_item).collect();

    // Ordenación final estable en dos pasos (sort_by_key es stable):
    //
    //   1. Trusted primero DENTRO de cada idioma — subs verificados
    //      por moderador van arriba. Sin esto, un sub trusted con 100
    //      descargas quedaba detrás de uno no-trusted con 200 y
    //      cualquier bug de sync arruinaba la peli.
    //   2. Orden por idioma según la lista explícita `languages`
    //      (primero `es`, luego `en`, ...).
    //
    // Como `sort_by_key` es estable, aplicamos primero el criterio
    // interno (trusted) y encima el criterio externo (idioma) para
    // que el resultado final esté ordenado por idioma y, dentro,
    // por trusted → downloads (el orden por descargas ya viene del
    // servidor con `download_count desc`).
    rank_subtitles(&mut subs, languages);

    Ok(subs)
}

/// Ordena `subs` in-place según la política de scoring de candidatos:
///
///  1. `from_trusted` primero dentro de cada idioma (sort estable).
///  2. Idioma preferido antes, según el orden de `languages`
///     ("es,en,fr" → español antes que inglés, inglés antes que francés;
///     idiomas no listados van al final).
///
/// Se extrae del cuerpo de `search()` para ser testeable sin red.
fn rank_subtitles(subs: &mut [Subtitle], languages: &str) {
    subs.sort_by_key(|s| !s.from_trusted);
    if !languages.is_empty() {
        let order: Vec<&str> = languages.split(',').map(str::trim).collect();
        subs.sort_by_key(|s| {
            order
                .iter()
                .position(|l| l.eq_ignore_ascii_case(&s.language))
                .unwrap_or(usize::MAX)
        });
    }
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

/// Convierte contenido de un `.srt` a WebVTT en memoria. WebVTT es lo
/// que consume `<track>` en HTML5 (WKWebView y WebView2 rechazan
/// `.srt` directamente).
///
/// La diferencia es mínima:
///   1. Header `WEBVTT` + línea en blanco.
///   2. Timestamps: `HH:MM:SS,mmm` → `HH:MM:SS.mmm` (coma → punto).
///
/// Todo lo demás (numeración de cues, saltos entre cues, tags de
/// texto) es compatible. Se acepta input con BOM UTF-8 y con
/// codificación latin-1 heurística (algunos SRT antiguos vienen en
/// windows-1252 y romperían al parsear como UTF-8 estricto).
#[cfg(feature = "gui")]
pub fn srt_to_vtt(srt_bytes: &[u8]) -> String {
    // Decodifica en dos capas ortogonales:
    //
    //   1. **Encoding real de los bytes**:
    //      * UTF-8 estricto primero — si valida, es lo que hay.
    //      * Si UTF-8 estricto falla: contamos cuántos bytes son
    //        inválidos. Si son POCOS (< 5% del archivo), es UTF-8
    //        con corrupción menor (típico: 1-2 bytes basura en un
    //        archivo de 50KB) → usamos `from_utf8_lossy` que
    //        preserva todo lo válido y solo pinta U+FFFD (`�`) en
    //        los bytes basura. Si son MUCHOS (>= 5%), el archivo no
    //        es UTF-8: pasamos a chardetng + encoding_rs para
    //        detectar Latin-1, cirílicos, chino, etc.
    //      * El bug anterior era caer a chardetng ante CUALQUIER
    //        error UTF-8, aunque fuera un byte suelto: chardetng
    //        elegía CP1252 como best guess y decodificaba TODO el
    //        archivo como Latin-1, generando mojibake (`é` UTF-8
    //        `C3 A9` pintado como `Ã` + `©`) en cada acento.
    //
    //   2. **Fix de doble-encoding** (`try_fix_mojibake`): cubre el
    //      caso ortogonal — archivos que son UTF-8 válidos pero cuyo
    //      CONTENIDO es texto que alguien malinterpretó como Latin-1
    //      y re-guardó como UTF-8 (`á` → `Ã¡`, `¿` → `Â¿`).
    //
    // El VTT resultante SIEMPRE sale UTF-8 puro, que es lo único
    // que la spec WebVTT (y por tanto `<track>`) acepta.
    let decoded: String = match std::str::from_utf8(srt_bytes) {
        Ok(s) => s.to_string(),
        Err(e) => {
            // ¿Cuántos bytes son inválidos vs total?
            let total = srt_bytes.len();
            let invalid_ratio = estimate_invalid_utf8_ratio(srt_bytes, e);
            if invalid_ratio < 0.05 {
                tracing::debug!(
                    target: "subs",
                    invalid_pct = format!("{:.2}", invalid_ratio * 100.0),
                    total,
                    "UTF-8 con bytes inválidos → lossy decode"
                );
                String::from_utf8_lossy(srt_bytes).into_owned()
            } else {
                tracing::debug!(
                    target: "subs",
                    invalid_pct = format!("{:.2}", invalid_ratio * 100.0),
                    "UTF-8 con bytes inválidos → chardetng"
                );
                let mut detector = chardetng::EncodingDetector::new();
                detector.feed(srt_bytes, true);
                let encoding = detector.guess(None, true);
                let (cow, actual, _had_errors) = encoding.decode(srt_bytes);
                tracing::debug!(target: "subs", encoding = actual.name(), "chardetng eligió");
                cow.into_owned()
            }
        }
    };
    let text = try_fix_mojibake(&decoded);
    // Strip BOM.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    // Normaliza newlines CRLF → LF para poder trabajar por líneas.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    // Regex-free timestamp swap: buscamos patrones `dd:dd:dd,ddd` y
    // cambiamos la coma por punto. Evita añadir dep de regex.
    let converted = swap_srt_timestamps(&normalized);
    let mut out = String::with_capacity(converted.len() + 16);
    out.push_str("WEBVTT\n\n");
    out.push_str(&converted);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Estima el porcentaje de bytes inválidos UTF-8 en un buffer.
/// Recorremos el buffer intentando decodear como UTF-8; cada vez que
/// tropezamos con un byte inválido, incrementamos el contador y
/// saltamos ese byte (no reintentamos secuencias multi-byte
/// completas). Aproximación suficiente para distinguir "1 byte
/// basura en 50KB de UTF-8 válido" de "todo el archivo es Latin-1".
#[cfg(feature = "gui")]
fn estimate_invalid_utf8_ratio(bytes: &[u8], _first_err: std::str::Utf8Error) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut invalid = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        match std::str::from_utf8(&bytes[cursor..]) {
            Ok(_) => break,
            Err(e) => {
                cursor += e.valid_up_to();
                let step = e.error_len().unwrap_or(bytes.len() - cursor);
                invalid += step;
                cursor += step;
                if e.error_len().is_none() {
                    // Trailing incomplete sequence — no seguimos.
                    break;
                }
            }
        }
    }
    invalid as f64 / bytes.len() as f64
}

/// "cuán malo pinta esto" — cuantos más matches, más probable que sea
/// texto doble-encodeado.
#[cfg(feature = "gui")]
#[allow(clippy::invisible_characters)]
fn mojibake_score(s: &str) -> usize {
    // Los patterns incluyen soft hyphen U+00AD y otros chars invisibles
    // porque son parte real del bitstream mojibake que estamos detectando
    // (0xC3 0xAD = UTF-8 "í" leído byte a byte como Latin-1 = "Ã" + SHY).
    // No es un typo — es lo que buscamos literalmente.
    // Pares Latin1(UTF-8) que salen al doble-encodear caracteres
    // acentuados típicos: á é í ó ú ñ ü ¿ ¡ « » – "smart quotes".
    // Incluimos también `Ã` y `Â` aislados (con el char siguiente en
    // rango Latin-1 supplement) porque cubren casos raros de acentos
    // menos comunes.
    const PATTERNS: &[&str] = &[
        "Ã¡",
        "Ã©",
        "Ã­",
        "Ã³",
        "Ãº",
        "Ã±",
        "Ã¼",
        "Ã‘",
        "Ã\u{81}",
        "Ã‰",
        "Ã\u{8d}",
        "Ã\u{93}",
        "Ãš",
        "Â¿",
        "Â¡",
        "Â«",
        "Â»",
        "Â°",
        "Â·",
        "â\u{80}\u{9c}",
        "â\u{80}\u{9d}",
        "â\u{80}\u{93}",
        "â\u{80}\u{94}",
        "â\u{80}\u{a6}",
    ];
    PATTERNS.iter().map(|p| s.matches(p).count()).sum()
}

/// Intenta el fix mojibake y devuelve la mejor versión (original o
/// fixed) según cuál tenga MENOS secuencias mojibake residuales. Si
/// el fix reduce estrictamente el score → mejor. Si empata o empeora
/// → dejamos el original (evita romper subs genuinamente en portugués
/// o nórdico que usan `Ã` legítimamente).
#[cfg(feature = "gui")]
fn try_fix_mojibake(s: &str) -> String {
    let original_score = mojibake_score(s);
    if original_score == 0 {
        return s.to_string();
    }
    let fixed = fix_mojibake(s);
    if mojibake_score(&fixed) < original_score {
        fixed
    } else {
        s.to_string()
    }
}

/// Revierte el doble-encoding aplicando reemplazos targeted por cada
/// secuencia mojibake conocida. NO intenta decode UTF-8 del string
/// entero (approach anterior): si el archivo mezclaba mojibake con
/// chars UTF-8 limpios (p.ej. "señor Ã©l" con `ñ` bien y `Ã©`
/// mojibake), el byte del `ñ` (0xF1) rompía el decode y el fix
/// entero se abandonaba → el sub salía mojibake en pantalla.
///
/// Los pares están ordenados por longitud descendente para evitar
/// que un reemplazo corto (`"Ã"` p.ej.) rompa uno largo (`"Ã©"`).
/// En la práctica no tenemos casos ambiguos aquí porque todos los
/// pairs tienen prefix único.
#[cfg(feature = "gui")]
#[allow(clippy::invisible_characters)]
fn fix_mojibake(s: &str) -> String {
    // (mojibake, char correcto). Cubre acentos españoles minúscula
    // + mayúscula, signos ¿¡«»°·, smart quotes, en/em dash y ellipsis.
    const REPLACEMENTS: &[(&str, &str)] = &[
        // Smart quotes y dashes (3 bytes mojibake → 3 bytes UTF-8).
        ("â\u{80}\u{9c}", "\u{201c}"), // "  (left double quote)
        ("â\u{80}\u{9d}", "\u{201d}"), // "  (right double quote)
        ("â\u{80}\u{98}", "\u{2018}"), // '
        ("â\u{80}\u{99}", "\u{2019}"), // '
        ("â\u{80}\u{93}", "\u{2013}"), // –  (en dash)
        ("â\u{80}\u{94}", "\u{2014}"), // —  (em dash)
        ("â\u{80}\u{a6}", "\u{2026}"), // …
        // Acentos minúscula.
        ("Ã¡", "á"),
        ("Ã©", "é"),
        ("Ã­", "í"),
        ("Ã³", "ó"),
        ("Ãº", "ú"),
        ("Ã±", "ñ"),
        ("Ã¼", "ü"),
        // Acentos mayúscula (los bytes UTF-8 de Á É Í Ó Ú Ñ Ü empiezan
        // por C3 y su segundo byte cae en el rango de chars imprimibles
        // Latin-1, por eso salen como `Ã` + char alto).
        ("Ã\u{81}", "Á"),
        ("Ã‰", "É"),
        ("Ã\u{8d}", "Í"),
        ("Ã\u{93}", "Ó"),
        ("Ãš", "Ú"),
        ("Ã‘", "Ñ"),
        ("Ãœ", "Ü"),
        // Signos y símbolos comunes.
        ("Â¿", "¿"),
        ("Â¡", "¡"),
        ("Â«", "«"),
        ("Â»", "»"),
        ("Â°", "°"),
        ("Â·", "·"),
        ("Â´", "´"),
    ];
    let mut out = s.to_string();
    for (from, to) in REPLACEMENTS {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

/// Cambia `HH:MM:SS,mmm --> HH:MM:SS,mmm` por `HH:MM:SS.mmm --> HH:MM:SS.mmm`.
/// Se aplica solo a comas que estén rodeadas por dígitos (para no tocar
/// comas en el texto de los subs).
///
/// Itera por CHARS UNICODE, no por bytes. La versión anterior hacía
/// `out.push(byte as char)` — eso descompone los chars UTF-8
/// multi-byte en sus bytes individuales (el char `¿` = UTF-8 bytes
/// `C2 BF` se rompía en dos chars `Â` + `¿`). Cualquier acento en un
/// sub se corrompía silenciosamente porque el string SEGUÍA siendo
/// UTF-8 válido, solo que con chars distintos.
#[cfg(feature = "gui")]
fn swap_srt_timestamps(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for i in 0..chars.len() {
        let c = chars[i];
        if c == ','
            && i > 0
            && chars[i - 1].is_ascii_digit()
            && i + 1 < chars.len()
            && chars[i + 1].is_ascii_digit()
        {
            out.push('.');
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_item(it: SearchItem) -> Option<Subtitle> {
    let a = it.attributes;
    // Safety net contra MT (Google Translate) aunque el filtro del API
    // los deje pasar (algunas entradas antiguas no tienen el flag).
    // NO filtramos AI-translated: son a menudo la única opción en
    // idiomas menores y su calidad es aceptable.
    if a.machine_translated.unwrap_or(false) {
        return None;
    }
    let file = a.files.into_iter().next()?;
    Some(Subtitle {
        file_id: file.file_id,
        language: a.language.unwrap_or_default(),
        release: a.release.unwrap_or_default(),
        downloads: a.download_count.unwrap_or(0),
        rating: a.ratings.unwrap_or(0.0),
        hearing_impaired: a.hearing_impaired.unwrap_or(false),
        from_trusted: a.from_trusted.unwrap_or(false),
        file_name: file.file_name,
        // El flag lo pone el caller (gui.rs) tras la búsqueda por
        // hash; aquí nace siempre false.
        hash_match: false,
    })
}

// ---------- shapes de la API ----------

#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<SearchItem>,
    /// Total de páginas disponibles para la búsqueda actual. La API
    /// pagina a 50 resultados por página; sin leer esto nos quedamos
    /// siempre en la primera. Puede venir ausente en respuestas
    /// vacías → default 0, que el loop de paginación trata como "1".
    #[serde(default)]
    total_pages: u32,
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
    /// Sub verificado por moderador OpenSubtitles. Se usa para
    /// priorizar en el sort (trusted primero dentro de cada idioma).
    #[serde(default)]
    from_trusted: Option<bool>,
    /// Marcado en el catálogo como traducción automática (Google
    /// Translate). Se descarta en `parse_item` como safety net por
    /// si el filtro `machine_translated=exclude` del API falla.
    #[serde(default)]
    machine_translated: Option<bool>,
    /// Traducción hecha con LLM. Lo tolerámos: son a menudo la única
    /// opción en idiomas menores. Se deserializa por completitud pero
    /// no filtramos por él.
    #[serde(default)]
    #[allow(dead_code)]
    ai_translated: Option<bool>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sub(language: &str, from_trusted: bool, downloads: u64) -> Subtitle {
        Subtitle {
            file_id: 0,
            language: language.to_string(),
            release: String::new(),
            downloads,
            rating: 0.0,
            hearing_impaired: false,
            from_trusted,
            file_name: None,
            hash_match: false,
        }
    }

    // ── compute_moviehash ─────────────────────────────────────────────────

    #[test]
    fn moviehash_all_zero_buffers_give_filesize() {
        // Hash = file_len cuando los 128 KB son ceros (contribución cero).
        let zeros = vec![0u8; 65536];
        let result = compute_moviehash(0x0000_0000_0002_0000, &zeros, &zeros);
        assert_eq!(result.as_deref(), Some("0000000000020000"));
    }

    #[test]
    fn moviehash_known_vector() {
        // file_len=0, primeros 64 KB = [1,0,0,0,0,0,0,0] repetido.
        // Cada chunk LE u64 = 1. 65536/8 = 8192 chunks.
        // hash = 0 + 8192 + 8192 = 16384 = 0x4000.
        let mut buf = vec![0u8; 65536];
        for i in (0..65536).step_by(8) {
            buf[i] = 1;
        }
        let result = compute_moviehash(0, &buf, &buf);
        assert_eq!(result.as_deref(), Some("0000000000004000"));
    }

    #[test]
    fn moviehash_rejects_wrong_buffer_size() {
        let short = vec![0u8; 100];
        let ok = vec![0u8; 65536];
        assert!(compute_moviehash(0, &short, &ok).is_none());
        assert!(compute_moviehash(0, &ok, &short).is_none());
        assert!(compute_moviehash(0, &short, &short).is_none());
    }

    #[test]
    fn moviehash_wraps_on_overflow() {
        // Verifica que la suma wrapping funciona: un buffer con todos 0xFF
        // tiene cada chunk u64 = u64::MAX. 8192 chunks de u64::MAX sumarán
        // en wrapping a 8192 * u64::MAX mod 2^64.
        let buf = vec![0xFFu8; 65536];
        let result = compute_moviehash(0, &buf, &buf);
        assert!(result.is_some(), "moviehash debe funcionar con overflow");
        let hex = result.expect("some");
        assert_eq!(hex.len(), 16, "resultado siempre 16 hex chars");
    }

    // ── sanitize_filename ─────────────────────────────────────────────────

    #[test]
    fn sanitize_replaces_forbidden_chars() {
        assert_eq!(sanitize_filename("sub/track.srt"), "sub_track.srt");
        assert_eq!(sanitize_filename("C:\\path\\to.srt"), "C__path_to.srt");
        assert_eq!(sanitize_filename("file:name?.srt"), "file_name_.srt");
        assert_eq!(sanitize_filename("a<b>c.srt"), "a_b_c.srt");
        assert_eq!(sanitize_filename("pipe|sep.srt"), "pipe_sep.srt");
        assert_eq!(sanitize_filename("q\"uoted.srt"), "q_uoted.srt");
    }

    #[test]
    fn sanitize_preserves_safe_filename() {
        let name = "Movie.2007.1080p.BluRay-CLASSiC.srt";
        assert_eq!(sanitize_filename(name), name);
    }

    // ── rank_subtitles — camino de scoring ────────────────────────────────

    #[test]
    fn sort_trusted_before_untrusted_same_language() {
        // Dentro del mismo idioma, el sub verificado por moderador
        // debe aparecer primero.
        let mut subs = vec![make_sub("es", false, 5000), make_sub("es", true, 100)];
        rank_subtitles(&mut subs, "es");
        assert!(subs[0].from_trusted, "trusted debe ir primero");
        assert!(!subs[1].from_trusted);
    }

    #[test]
    fn sort_respects_language_preference_order() {
        // Con preferencia "es,en", el español va primero aunque tenga
        // menos descargas (simulamos lo que llega del API ya ordenado
        // por downloads desc).
        let mut subs = vec![
            make_sub("en", false, 10_000),
            make_sub("fr", false, 8_000),
            make_sub("es", false, 3_000),
        ];
        rank_subtitles(&mut subs, "es,en");
        assert_eq!(subs[0].language, "es");
        assert_eq!(subs[1].language, "en");
        assert_eq!(subs[2].language, "fr");
    }

    #[test]
    fn sort_unlisted_language_goes_to_end() {
        // Idioma no listado en la preferencia → al final.
        let mut subs = vec![make_sub("ja", false, 999), make_sub("es", false, 1)];
        rank_subtitles(&mut subs, "es,en");
        assert_eq!(subs[0].language, "es");
        assert_eq!(subs[1].language, "ja");
    }

    #[test]
    fn sort_trusted_preserved_within_language_after_language_sort() {
        // Tras ordenar por idioma, el orden trusted↑ dentro de cada
        // grupo debe conservarse (sort_by_key es stable).
        let mut subs = vec![
            make_sub("es", false, 200),
            make_sub("en", true, 50),
            make_sub("es", true, 1),
            make_sub("en", false, 100),
        ];
        rank_subtitles(&mut subs, "es,en");
        assert_eq!(subs[0].language, "es");
        assert!(subs[0].from_trusted, "es trusted primero en su grupo");
        assert_eq!(subs[2].language, "en");
        assert!(subs[2].from_trusted, "en trusted primero en su grupo");
    }

    #[test]
    fn sort_empty_vec_does_not_panic() {
        let mut subs: Vec<Subtitle> = vec![];
        rank_subtitles(&mut subs, "es,en");
        assert!(subs.is_empty());
    }

    // ── srt_to_vtt (feature = "gui") ─────────────────────────────────────

    #[cfg(feature = "gui")]
    #[test]
    fn srt_to_vtt_basic_timestamp_conversion() {
        let srt = b"1\n00:00:01,500 --> 00:00:03,750\nHello world\n\n";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(
            vtt.contains("00:00:01.500 --> 00:00:03.750"),
            "coma debe convertirse a punto en timestamps"
        );
        assert!(vtt.contains("Hello world"));
    }

    #[cfg(feature = "gui")]
    #[test]
    fn srt_to_vtt_preserves_utf8_accents() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nÁlvaro pregunta: ¿cómo estás?\n\n";
        let vtt = srt_to_vtt(srt.as_bytes());
        assert!(vtt.contains("Álvaro"), "Á debe preservarse");
        assert!(vtt.contains("¿cómo estás?"), "accentos deben preservarse");
    }

    #[cfg(feature = "gui")]
    #[test]
    fn srt_to_vtt_strips_bom() {
        let mut srt = vec![0xEFu8, 0xBB, 0xBF];
        srt.extend_from_slice(b"1\n00:00:01,000 --> 00:00:02,000\nText\n\n");
        let vtt = srt_to_vtt(&srt);
        assert!(
            vtt.starts_with("WEBVTT\n\n"),
            "BOM eliminado: no debe haber carácter antes de WEBVTT"
        );
    }

    #[cfg(feature = "gui")]
    #[test]
    fn swap_timestamps_preserves_commas_in_text() {
        let s = "00:00:01,500 --> 00:00:03,750\nHello, world!\n";
        let out = swap_srt_timestamps(s);
        assert!(
            out.contains("01.500") && out.contains("03.750"),
            "commas en timestamps convertidas"
        );
        assert!(out.contains("Hello, world!"), "coma en texto preservada");
    }

    #[cfg(feature = "gui")]
    #[test]
    fn fix_mojibake_corrects_double_encoded_spanish() {
        assert_eq!(fix_mojibake("Ã¡"), "á");
        assert_eq!(fix_mojibake("Ã©"), "é");
        assert_eq!(fix_mojibake("Â¿"), "¿");
        let s = "Ã¡Ã©Ã³Ãº";
        assert_eq!(fix_mojibake(s), "áéóú");
    }

    #[cfg(feature = "gui")]
    #[test]
    fn try_fix_mojibake_leaves_clean_text_alone() {
        let clean = "El señor preguntó: ¿dónde está el gato?";
        assert_eq!(try_fix_mojibake(clean), clean);
    }
}
