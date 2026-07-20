//! Parser estructurado de nombres de release scene/P2P.
//!
//! Reemplaza (a partir de Fase 2 del audit de búsqueda) las cuatro
//! heurísticas paralelas que operaban sobre strings crudos:
//!   * `is_tv_release`      → `parsed.season / parsed.episode`
//!   * `release_matches_year` → `parsed.year`
//!   * `quality_from_title`  → `parsed.resolution / parsed.source`
//!   * `filter_by_token_overlap` → `parsed.title` vs conjunto de
//!     títulos válidos (variantes de TMDB en Fase 3)
//!
//! Filosofía: **parsear una vez, decidir contra la estructura**.
//! Los falsos positivos de las heurísticas antiguas (homónimos
//! peli/serie, años dentro del título tipo "2001: A Space Odyssey")
//! desaparecen porque el parser distingue tokens según su POSICIÓN
//! y su tipo, no por búsquedas globales de substrings.
//!
//! No usamos crate externo (`torrent-name-parser` sería la opción
//! natural, pero para este alcance basta con una pasada tokenizada
//! y un puñado de constantes). Si en el futuro necesitamos cubrir
//! más edge cases (anime con brackets, series con títulos raros),
//! se puede portar sin cambiar la API pública (`ParsedRelease`).

/// Resultado del parsing de un nombre de release. Todos los campos
/// son opcionales excepto `title` — si el parser no encuentra
/// resolución/año/etc., se dejan en `None` y el caller decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRelease {
    /// Título "limpio" reconstruido a partir de los tokens ANTERIORES
    /// al primer tag técnico (año, resolución, source, sxxeyy).
    /// Espacios como separadores; sin puntos/guiones scene. NO se
    /// normaliza a lowercase — el caller aplica `normalize_title`
    /// para comparar.
    pub title: String,
    /// Año de release (typically 1900-2099). El parser coge el
    /// PRIMER año dentro del rango que aparezca — para releases con
    /// dos años (`Blade.Runner.2049.2017`), el año del título
    /// (`2049`) suele quedar dentro del `title` y `year` recoge el
    /// segundo (`2017` = año de estreno). No siempre acierta pero
    /// para el 90% de casos basta; el matching final tolera ±1.
    pub year: Option<u16>,
    pub season: Option<u16>,
    pub episode: Option<u16>,
    /// Resolución detectada: `"2160p"`, `"1080p"`, `"720p"`, `"480p"`,
    /// `"4K"`. Espejo de la `quality_from_title` antigua para no
    /// romper el score de calidad en `mod.rs`.
    pub resolution: Option<String>,
    /// Fuente del release: `"BluRay"`, `"WEB-DL"`, `"WEBRip"`,
    /// `"HDRip"`, `"DVDRip"`, `"HDTV"`, `"REMUX"`.
    pub source: Option<String>,
    /// Codec: `"x264"`, `"x265"`, `"H.264"`, `"H.265"`, `"HEVC"`,
    /// `"AV1"`, `"XviD"`.
    pub codec: Option<String>,
}

impl ParsedRelease {
    /// `true` si el release parece un episodio o pack de serie:
    ///   * `SxxEyy` explícito ⇒ `season && episode`
    ///   * `Season N` / `Sxx` sin episodio ⇒ solo `season`
    ///
    /// Reemplaza el grueso de `is_tv_release` en el nuevo pipeline.
    /// Los marcadores tipo "Complete Series" / "Mini-Series" siguen
    /// aplicándose vía la heurística antigua como red extra (ver
    /// `search_all` en `mod.rs`).
    pub fn is_tv(&self) -> bool {
        self.season.is_some() || self.episode.is_some()
    }
}

// ── Constantes de tokens conocidos ──────────────────────────────────────────
//
// Case-insensitive. Los tokens se comparan en lowercase después de
// tokenizar. Ordenados por especificidad (los más largos primero
// dentro de cada categoría) para no matchear un prefijo por accidente.

const RESOLUTIONS: &[(&str, &str)] = &[
    ("2160p", "2160p"),
    ("1440p", "1440p"),
    ("1080p", "1080p"),
    ("720p", "720p"),
    ("480p", "480p"),
    ("4k", "2160p"),
    ("uhd", "2160p"),
    ("fullhd", "1080p"),
    ("fhd", "1080p"),
];

const SOURCES: &[(&str, &str)] = &[
    // Multi-token — se detectan combinados aparte (WEB-DL, etc.),
    // aquí solo los "canónicos" que scene usa como un token.
    ("bluray", "BluRay"),
    ("blu-ray", "BluRay"),
    ("bdrip", "BDRip"),
    ("brrip", "BRRip"),
    ("webrip", "WEBRip"),
    ("web-dl", "WEB-DL"),
    ("webdl", "WEB-DL"),
    ("web", "WEB"),
    ("hdrip", "HDRip"),
    ("dvdrip", "DVDRip"),
    ("hdtv", "HDTV"),
    ("remux", "REMUX"),
    ("uhdrip", "UHDRip"),
    ("dvd", "DVD"),
];

const CODECS: &[(&str, &str)] = &[
    ("x265", "x265"),
    ("x264", "x264"),
    ("h265", "H.265"),
    ("h264", "H.264"),
    ("hevc", "HEVC"),
    ("av1", "AV1"),
    ("xvid", "XviD"),
    ("divx", "DivX"),
    ("vp9", "VP9"),
];

// ── API ────────────────────────────────────────────────────────────────────

/// Parsea un nombre de release. Nunca falla — si no reconoce nada,
/// devuelve un `ParsedRelease` con `title = raw` (limpio) y el resto
/// en `None`. Los callers pueden decidir vía `parsed.is_tv()` y
/// comparaciones sobre `parsed.year / parsed.title`.
pub fn parse(raw: &str) -> ParsedRelease {
    let tokens = tokenize(raw);

    // Pasada 1: catalogar tokens por tipo. Se registran TODAS las
    // ocurrencias — la decisión final (qué año es el "release year",
    // dónde cortar el título) se toma en la pasada 2 con más
    // información.
    #[derive(Default)]
    struct Scan {
        year_positions: Vec<(usize, u16)>,
        first_resolution: Option<(usize, String)>,
        first_source: Option<(usize, String)>,
        first_codec: Option<(usize, String)>,
        first_season_ep: Option<(usize, u16, Option<u16>)>,
        /// Episode-only token (`E01`, `EP01`, `Ep 01`) que aparece
        /// SIN season prefix. Común en releases asiáticos con
        /// separación tipo `Series.Name.S01.EP01.1080p` — el parser
        /// original solo capturaba `SxxEyy` en un token único y
        /// dejaba estos como pack de temporada por error.
        first_episode_only: Option<(usize, u16)>,
    }
    let mut scan = Scan::default();

    for (i, tok) in tokens.iter().enumerate() {
        let lower = tok.to_ascii_lowercase();

        if let Some((s, e)) = parse_sxxeyy(&lower) {
            if scan.first_season_ep.is_none() {
                scan.first_season_ep = Some((i, s, Some(e)));
            }
            continue;
        }
        if let Some(s) = parse_sxx(&lower) {
            if scan.first_season_ep.is_none() {
                scan.first_season_ep = Some((i, s, None));
            }
            continue;
        }
        if let Some(e) = parse_episode_only(&lower) {
            if scan.first_episode_only.is_none() {
                scan.first_episode_only = Some((i, e));
            }
            continue;
        }
        if let Some(y) = parse_year(&lower) {
            scan.year_positions.push((i, y));
            continue;
        }
        if scan.first_resolution.is_none() {
            if let Some(v) = match_tag(&lower, RESOLUTIONS) {
                scan.first_resolution = Some((i, v.to_string()));
                continue;
            }
        }
        if scan.first_source.is_none() {
            if let Some(v) = match_tag(&lower, SOURCES) {
                scan.first_source = Some((i, v.to_string()));
                continue;
            }
        }
        if scan.first_codec.is_none() {
            if let Some(v) = match_tag(&lower, CODECS) {
                scan.first_codec = Some((i, v.to_string()));
                continue;
            }
        }
    }

    // Pasada 2: determinar el "release year" y el corte del título.
    //
    // Los tags técnicos (resolución/source/codec/sxxeyy) siempre van
    // DESPUÉS del título. Su posición mínima es el techo natural
    // del corte. Para el año hay dos casos:
    //   * Un solo año → es el año técnico, corta el título.
    //   * Varios años → el ÚLTIMO que caiga ANTES del primer tag
    //     técnico es el "release year"; los anteriores forman parte
    //     del título (p. ej. "2001 A Space Odyssey 1968" → 1968).
    let tag_cut = [
        scan.first_resolution.as_ref().map(|(i, _)| *i),
        scan.first_source.as_ref().map(|(i, _)| *i),
        scan.first_codec.as_ref().map(|(i, _)| *i),
        scan.first_season_ep.as_ref().map(|(i, _, _)| *i),
        scan.first_episode_only.as_ref().map(|(i, _)| *i),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(tokens.len());

    // Release year = último año antes del primer tag técnico. Si no
    // hay tag técnico, el último año a secas. Si no hay años, None.
    // (`rfind` recorre desde el final y corta en el primer match →
    // O(k) en el peor caso, no O(n).)
    let release_year = scan
        .year_positions
        .iter()
        .rfind(|(pos, _)| *pos < tag_cut)
        .or_else(|| scan.year_positions.last())
        .map(|(_, y)| *y);
    let year_cut = scan
        .year_positions
        .iter()
        .rfind(|(pos, _)| *pos < tag_cut)
        .map(|(pos, _)| *pos)
        .unwrap_or(tokens.len());

    let cut = tag_cut.min(year_cut);

    let title: String = tokens[..cut]
        .iter()
        .filter(|t| !t.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");

    let (season, episode) = match scan.first_season_ep {
        Some((_, s, e)) => (Some(s), e),
        None => (None, None),
    };
    // Reconciliar `Sxx` + `EP01` (o `E01` suelto) en (Some(s), Some(e))
    // — cubre releases asiáticos comunes (`Series.Name.S01.EP01.1080p`).
    // Solo cuando ya teníamos season pero no episode, y hay un
    // episode-only en la parte técnica del release.
    let episode = match (episode, scan.first_episode_only) {
        (None, Some((_, e))) if season.is_some() => Some(e),
        (other, _) => other,
    };

    ParsedRelease {
        title,
        year: release_year,
        season,
        episode,
        resolution: scan.first_resolution.map(|(_, v)| v),
        source: scan.first_source.map(|(_, v)| v),
        codec: scan.first_codec.map(|(_, v)| v),
    }
}

/// Normaliza un título para comparar dos strings equivalentes:
/// lowercase, colapsa cualquier carácter no alfanumérico a espacio
/// simple, y trim. `normalize_title("The.Lord.of.the.Rings") ==
/// normalize_title("The Lord of the Rings")`.
///
/// **NO** quita stopwords ni el prefijo `the` — comparación estricta
/// (dos formas del mismo título deben coincidir literalmente). Para
/// matching laxo (variantes de TMDB), el caller genera todas las
/// variantes y prueba cada una tras `normalize_title`.
pub fn normalize_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

// ── Helpers internos ────────────────────────────────────────────────────────

/// Tokeniza un release name partiendo por separadores scene comunes
/// (`.` `_` espacio) y brackets (`[` `]` `(` `)`). Los tokens vacíos
/// se descartan aquí para que el parser no tenga que preocuparse de
/// ellos.
///
/// **NO** partimos por `-` intencionadamente: `WEB-DL` es un solo
/// tag de source y `x265-CyTSuNee` lleva el codec + grupo pegados.
/// `match_tag` sabe mirar tanto el token completo como el prefijo
/// antes del primer `-` para cubrir ambos casos.
fn tokenize(s: &str) -> Vec<String> {
    s.split(['.', '_', ' ', '[', ']', '(', ')'])
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// Compara `tok` (ya en lowercase) contra las entradas del mapa.
/// Prueba el token completo primero (para `web-dl`, `blu-ray`) y
/// luego el prefijo antes del primer `-` (para `x264-GROUP`). Así
/// captura las dos formas comunes sin perder el grupo en los codecs.
fn match_tag(tok: &str, map: &[(&'static str, &'static str)]) -> Option<&'static str> {
    for (needle, out) in map {
        if tok == *needle {
            return Some(out);
        }
    }
    if let Some(head) = tok.split('-').next() {
        if head != tok {
            for (needle, out) in map {
                if head == *needle {
                    return Some(out);
                }
            }
        }
    }
    None
}

fn parse_year(tok: &str) -> Option<u16> {
    if tok.len() != 4 || !tok.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let y: u16 = tok.parse().ok()?;
    (1900..=2099).contains(&y).then_some(y)
}

/// `s01e02`, `s1e2`, `s01e02e03` (solo temporada+primer episodio).
fn parse_sxxeyy(tok: &str) -> Option<(u16, u16)> {
    let bytes = tok.as_bytes();
    if bytes.first() != Some(&b's') {
        return None;
    }
    let mut i = 1;
    let s_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == s_start || i == bytes.len() || bytes[i] != b'e' {
        return None;
    }
    let season: u16 = tok[s_start..i].parse().ok()?;
    i += 1;
    let e_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == e_start {
        return None;
    }
    let episode: u16 = tok[e_start..i].parse().ok()?;
    Some((season, episode))
}

/// `s01` / `s1` sin episodio (temporada completa como token suelto).
fn parse_sxx(tok: &str) -> Option<u16> {
    let bytes = tok.as_bytes();
    if bytes.first() != Some(&b's') {
        return None;
    }
    if bytes.len() < 2 || !bytes[1..].iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    tok[1..].parse().ok()
}

/// `e01` / `ep01` / `ep1` — marcador de episodio suelto que aparece
/// SIN el prefijo de temporada en el mismo token. Común en releases
/// asiáticos con separación por puntos: `Series.Name.S01.EP01.WEB`.
/// Rechazamos tokens ambiguos: `en` (idioma English) empieza por `e`
/// pero no tiene dígitos, `episode` sin número → None. Aceptamos
/// hasta 4 dígitos (packs de anime raros con 200+ episodios).
fn parse_episode_only(tok: &str) -> Option<u16> {
    let bytes = tok.as_bytes();
    if bytes.first() != Some(&b'e') {
        return None;
    }
    // Salta `e` o `ep`.
    let mut i = 1;
    if bytes.get(i) == Some(&b'p') {
        i += 1;
    }
    if i == bytes.len() {
        return None;
    }
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start || i != bytes.len() {
        return None;
    }
    tok[digits_start..].parse().ok()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_movie_release() {
        let p = parse("Blade.Runner.2049.2017.2160p.UHD.BluRay.x265-CyTSuNee");
        assert_eq!(p.title, "Blade Runner 2049");
        assert_eq!(p.year, Some(2017));
        assert_eq!(p.resolution, Some("2160p".to_string()));
        assert_eq!(p.source, Some("BluRay".to_string()));
        assert_eq!(p.codec, Some("x265".to_string()));
        assert!(!p.is_tv());
    }

    #[test]
    fn parses_scene_tv_episode() {
        let p = parse("The.Office.US.S03E12.720p.HDTV.x264-LOL");
        assert_eq!(p.season, Some(3));
        assert_eq!(p.episode, Some(12));
        assert_eq!(p.resolution, Some("720p".to_string()));
        assert_eq!(p.source, Some("HDTV".to_string()));
        assert!(p.is_tv());
        // El título se corta ANTES de S03E12.
        assert_eq!(p.title, "The Office US");
    }

    #[test]
    fn does_not_confuse_movie_with_season_in_title() {
        // "Season of the Witch" no lleva SxxEyy → is_tv=false.
        let p = parse("Season.of.the.Witch.2011.1080p.BluRay.x264");
        assert!(!p.is_tv());
        assert_eq!(p.title, "Season of the Witch");
        assert_eq!(p.year, Some(2011));
    }

    #[test]
    fn does_not_confuse_2001_a_space_odyssey() {
        // Dos años: "2001" (título) y "1968" (release). El parser
        // toma el ÚLTIMO año antes del primer tag técnico como año
        // de release y deja el año anterior dentro del título.
        let p = parse("2001.A.Space.Odyssey.1968.1080p.BluRay.x264");
        assert_eq!(p.year, Some(1968));
        assert_eq!(p.title, "2001 A Space Odyssey");
    }

    #[test]
    fn parses_web_dl_and_hevc_variants() {
        let p = parse("Some.Movie.2020.1080p.WEB-DL.H264-GROUP");
        assert_eq!(p.year, Some(2020));
        assert_eq!(p.source, Some("WEB-DL".to_string()));
        assert_eq!(p.codec, Some("H.264".to_string()));
        assert!(!p.is_tv());
    }

    #[test]
    fn parses_hdrip_movie() {
        let p = parse("Funny.Games.2007.HDRip.XviD-FooBar");
        assert_eq!(p.title, "Funny Games");
        assert_eq!(p.year, Some(2007));
        assert_eq!(p.source, Some("HDRip".to_string()));
        assert_eq!(p.codec, Some("XviD".to_string()));
    }

    #[test]
    fn parses_release_with_spaces_and_brackets() {
        let p = parse("[Group] Some Movie (2019) [1080p] [BluRay] [x264]");
        assert_eq!(p.year, Some(2019));
        assert_eq!(p.resolution, Some("1080p".to_string()));
        assert_eq!(p.source, Some("BluRay".to_string()));
        assert_eq!(p.codec, Some("x264".to_string()));
        // "[Group]" queda como primer token del título (parser no
        // detecta grupos-por-brackets aún). Aceptable — el matcher
        // por variantes normaliza y lo tolera.
        assert!(p.title.contains("Some Movie"));
    }

    #[test]
    fn normalize_title_collapses_separators() {
        assert_eq!(
            normalize_title("The.Lord.of.the.Rings"),
            normalize_title("The Lord of the Rings")
        );
        assert_eq!(normalize_title("Amélie (2001)"), "amélie 2001");
    }

    #[test]
    fn normalize_title_handles_cyrillic() {
        // Cirílico → conserva las letras (alphanumeric == true).
        assert_eq!(normalize_title("Брат 1997"), "брат 1997");
    }

    #[test]
    fn tv_scene_uppercase_still_detected() {
        let p = parse("BREAKING.BAD.S05E14.1080p.WEB-DL");
        assert_eq!(p.season, Some(5));
        assert_eq!(p.episode, Some(14));
        assert!(p.is_tv());
    }

    #[test]
    fn season_only_pack() {
        // `S01` suelto (sin episodio) suele indicar pack de temporada.
        let p = parse("Chernobyl.S01.1080p.HMAX.WEB-DL");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, None);
        assert!(p.is_tv());
    }

    #[test]
    fn separated_season_and_episode_tokens() {
        // Release asiático típico: `S01 EP01` en tokens separados.
        // Antes del audit alt-titles-series esto salía como
        // season pack (E01 se caía como token técnico ignorado).
        let p = parse("Series.Name.S01.EP01.1080p.WEB-DL");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(1));
        assert!(p.is_tv());
        assert_eq!(p.title, "Series Name");
    }

    #[test]
    fn separated_e_only_marker() {
        // `E01` sin `p` prefix también cuenta cuando hay Sxx antes.
        let p = parse("Show.Name.S02.E05.720p.HDTV.x264");
        assert_eq!(p.season, Some(2));
        assert_eq!(p.episode, Some(5));
    }

    #[test]
    fn ep_marker_without_season_ignored() {
        // Sin `Sxx` en el release, un `EP01` solo no promueve a
        // episode-of-unknown-season — nos quedamos con episode=None
        // para no falsear un match contra query.season.
        let p = parse("Some.Show.EP01.1080p.WEB");
        assert_eq!(p.episode, None);
    }

    #[test]
    fn en_language_tag_not_confused_with_episode() {
        // `en` sin dígitos no debe capturarse como episode.
        assert_eq!(parse_episode_only("en"), None);
        assert_eq!(parse_episode_only("english"), None);
        assert_eq!(parse_episode_only("ep"), None);
    }

    #[test]
    fn missing_tags_still_returns_something() {
        // Release "sucio" sin resolución/source: el parser no
        // encuentra corte y devuelve todo como título.
        let p = parse("Some Random Movie Name");
        assert_eq!(p.title, "Some Random Movie Name");
        assert_eq!(p.year, None);
        assert!(!p.is_tv());
    }

    // ── Edge cases ─────────────────────────────────────────────────

    #[test]
    fn empty_input_does_not_panic() {
        let p = parse("");
        assert_eq!(p.title, "");
        assert_eq!(p.year, None);
        assert!(!p.is_tv());
    }

    #[test]
    fn parse_year_boundaries() {
        assert_eq!(parse_year("1900"), Some(1900));
        assert_eq!(parse_year("2099"), Some(2099));
        assert_eq!(parse_year("1899"), None);
        assert_eq!(parse_year("2100"), None);
        assert_eq!(parse_year("999"), None); // <4 dígitos
        assert_eq!(parse_year("20250"), None); // >4 dígitos
        assert_eq!(parse_year("abcd"), None);
        assert_eq!(parse_year("201a"), None);
    }

    #[test]
    fn parse_sxxeyy_variants() {
        assert_eq!(parse_sxxeyy("s01e02"), Some((1, 2)));
        assert_eq!(parse_sxxeyy("s1e2"), Some((1, 2)));
        // Multi-episodio: `s01e02e03` — el parser lee dígitos hasta
        // el primer no-dígito, así que devuelve `Some((1, 2))` y
        // descarta el resto. Aceptable — el episodio "primario" es
        // el que interesa para matching.
        assert_eq!(parse_sxxeyy("s01e02e03"), Some((1, 2)));
        // Malformados.
        assert_eq!(parse_sxxeyy("se01"), None);
        assert_eq!(parse_sxxeyy("s01"), None);
        assert_eq!(parse_sxxeyy("s01e"), None);
        assert_eq!(parse_sxxeyy("e01"), None);
        assert_eq!(parse_sxxeyy(""), None);
    }

    #[test]
    fn parse_sxx_variants() {
        assert_eq!(parse_sxx("s01"), Some(1));
        assert_eq!(parse_sxx("s1"), Some(1));
        assert_eq!(parse_sxx("s99"), Some(99));
        assert_eq!(parse_sxx("s"), None);
        assert_eq!(parse_sxx("sx"), None);
        assert_eq!(parse_sxx("s01a"), None);
    }

    #[test]
    fn parse_episode_only_variants() {
        assert_eq!(parse_episode_only("e01"), Some(1));
        assert_eq!(parse_episode_only("ep01"), Some(1));
        assert_eq!(parse_episode_only("ep1"), Some(1));
        assert_eq!(parse_episode_only("ep200"), Some(200));
        assert_eq!(parse_episode_only("e"), None);
        assert_eq!(parse_episode_only("ep"), None);
        assert_eq!(parse_episode_only("epx"), None);
        assert_eq!(parse_episode_only("e01a"), None);
    }

    #[test]
    fn match_tag_hits_group_prefix() {
        // `x264-GROUP` debe extraer `x264` como codec via prefijo.
        assert_eq!(match_tag("x264-cytsun", CODECS), Some("x264"));
        assert_eq!(match_tag("x265-yify", CODECS), Some("x265"));
        assert_eq!(match_tag("h264-lol", CODECS), Some("H.264"));
        // Sin `-` no cuenta si el token difiere.
        assert_eq!(match_tag("xvid2", CODECS), None);
    }

    #[test]
    fn resolution_aliases_normalize() {
        let p1 = parse("Movie.2019.4K.BluRay.x265");
        assert_eq!(p1.resolution, Some("2160p".to_string()));
        let p2 = parse("Movie.2019.UHD.WEB-DL");
        assert_eq!(p2.resolution, Some("2160p".to_string()));
        let p3 = parse("Movie.2020.FullHD.BluRay");
        assert_eq!(p3.resolution, Some("1080p".to_string()));
        let p4 = parse("Movie.2020.FHD.BluRay");
        assert_eq!(p4.resolution, Some("1080p".to_string()));
    }

    #[test]
    fn source_variants_normalize() {
        for (raw, expected) in [
            ("Movie.2020.BDRip.x264", "BDRip"),
            ("Movie.2020.BRRip.x264", "BRRip"),
            ("Movie.2020.WEBRip.x264", "WEBRip"),
            ("Movie.2020.WEB-DL.x264", "WEB-DL"),
            ("Movie.2020.WEBDL.x264", "WEB-DL"),
            ("Movie.2020.UHDRip.x265", "UHDRip"),
            ("Movie.2020.REMUX.x265", "REMUX"),
            ("Movie.2020.Blu-Ray.x264", "BluRay"),
        ] {
            let p = parse(raw);
            assert_eq!(p.source.as_deref(), Some(expected), "raw: {raw}");
        }
    }

    #[test]
    fn codec_variants_normalize() {
        for (raw, expected) in [
            ("Movie.2020.1080p.HEVC", "HEVC"),
            ("Movie.2020.1080p.AV1", "AV1"),
            ("Movie.2020.1080p.VP9", "VP9"),
            ("Movie.2020.1080p.DivX", "DivX"),
            ("Movie.2000.1080p.H265", "H.265"),
        ] {
            let p = parse(raw);
            assert_eq!(p.codec.as_deref(), Some(expected), "raw: {raw}");
        }
    }

    #[test]
    fn movie_starting_with_year_1917() {
        // "1917" (2019) — año dentro del título Y año de release.
        let p = parse("1917.2019.1080p.BluRay.x264");
        assert_eq!(p.year, Some(2019));
        assert_eq!(p.title, "1917");
    }

    #[test]
    fn is_tv_true_when_only_season() {
        let p = ParsedRelease {
            title: "X".into(),
            year: None,
            season: Some(1),
            episode: None,
            resolution: None,
            source: None,
            codec: None,
        };
        assert!(p.is_tv());
    }

    #[test]
    fn is_tv_true_when_only_episode() {
        let p = ParsedRelease {
            title: "X".into(),
            year: None,
            season: None,
            episode: Some(5),
            resolution: None,
            source: None,
            codec: None,
        };
        assert!(p.is_tv());
    }

    #[test]
    fn normalize_title_trims_and_collapses() {
        assert_eq!(normalize_title("  Foo   Bar  "), "foo bar");
        assert_eq!(normalize_title("!!!  foo  !!!"), "foo");
        assert_eq!(normalize_title(""), "");
        assert_eq!(normalize_title("---"), "");
    }

    #[test]
    fn normalize_title_preserves_cjk() {
        // Chino simplificado + japonés + coreano — todos alfanuméricos.
        assert_eq!(normalize_title("流浪地球 2019"), "流浪地球 2019");
        assert_eq!(normalize_title("千と千尋の神隠し"), "千と千尋の神隠し");
        assert_eq!(normalize_title("기생충 2019"), "기생충 2019");
    }

    #[test]
    fn tokenize_splits_scene_separators() {
        assert_eq!(
            tokenize("Some.Movie_Name-Group [1080p]"),
            vec!["Some", "Movie", "Name-Group", "1080p"]
        );
    }

    #[test]
    fn tokenize_ignores_empty() {
        // Múltiples puntos consecutivos no generan tokens vacíos.
        assert_eq!(tokenize("Foo...Bar"), vec!["Foo", "Bar"]);
    }

    #[test]
    fn ambiguous_year_in_title_only() {
        // "Blade Runner 2049" sin año extra — el "2049" debe ir al
        // título (>2099 out of range de todos modos, pero la lógica
        // no lo sabe todavía). Para 2049 exact que sí está en rango,
        // como es el ÚNICO año antes de tags, se toma como año de
        // release. Es el trade-off documentado en `parse`.
        let p = parse("Blade.Runner.2049.1080p.WEB-DL");
        // Con la lógica actual: 2049 es el único año antes del tag
        // 1080p → year = 2049, title = "Blade Runner".
        assert_eq!(p.year, Some(2049));
        assert_eq!(p.title, "Blade Runner");
    }

    #[test]
    fn scene_group_after_codec_ignored() {
        // El sufijo `-CyTSuNee` del grupo scene NO debe ensuciar
        // ningún campo — se queda como prefijo del último token
        // técnico.
        let p = parse("Movie.2020.1080p.BluRay.x264-YIFY");
        assert_eq!(p.codec, Some("x264".to_string()));
        // El título se corta antes de "2020".
        assert_eq!(p.title, "Movie");
    }
}
