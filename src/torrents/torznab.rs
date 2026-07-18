//! Provider Torznab (Jackett / Prowlarr). Opt-in: solo se activa si están
//! definidas las variables `TORZNAB_URL` y `TORZNAB_APIKEY`.
//!
//! Ejemplo de URL para Jackett aggregate: `http://localhost:9117/api/v2.0/indexers/all/results/torznab/api`.
//! Docs Torznab: <https://torznab.github.io/spec-1.3-draft/>

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;

use super::{
    build_magnet, infohash_from_magnet, quality_from_title, MovieQuery, Torrent, TorrentProvider,
};

pub struct Torznab {
    url: String,
    apikey: String,
}

impl Torznab {
    pub fn new(url: String, apikey: String) -> Self {
        Self { url, apikey }
    }
}

// ── Estructuras XML mínimas ─────────────────────────────────────────────────
//
// Torznab devuelve un RSS con items enriquecidos con <torznab:attr>. Con
// quick-xml + serde lo parseamos con la forma mínima que necesitamos.

#[derive(Debug, Deserialize)]
struct Rss {
    channel: Channel,
}

#[derive(Debug, Deserialize, Default)]
struct Channel {
    #[serde(rename = "item", default)]
    items: Vec<Item>,
}

#[derive(Debug, Deserialize, Default)]
struct Item {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default, rename = "guid")]
    _guid: Option<String>,
    /// Atributos <torznab:attr name="..." value="..."/>. quick-xml los
    /// deserializa aquí porque el prefijo `torznab:` se ignora por defecto.
    #[serde(default, rename = "attr")]
    attrs: Vec<Attr>,
}

#[derive(Debug, Deserialize)]
struct Attr {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@value")]
    value: String,
}

impl Item {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .map(|a| a.value.as_str())
    }
}

#[async_trait]
impl TorrentProvider for Torznab {
    fn name(&self) -> &'static str {
        "torznab"
    }

    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>> {
        // Fase 3c — matching por ID cuando el indexer lo soporta.
        // Torznab espera el ID sin el prefijo "tt".
        //
        // Estrategia:
        //   * Series: `t=tvsearch&imdbid=<id>&season=S[&ep=E]` — la
        //     vía canónica en Torznab para episodios. Si el indexer
        //     no soporta tvsearch caps, cae a `t=search&q="title SxxEyy"`.
        //   * Película con imdb: `t=movie&imdbid=<id>`, fallback a
        //     `t=search&q=title`.
        //   * Sin imdb: `t=search&q=title` (o con "SxxEyy" si serie).
        //
        // Política unificada con Knaben/Apibay: el año NUNCA va en la
        // query — los grupos etiquetan el año del estreno original y
        // no el USA que TMDB reporta, así "Funny Games 2008" devuelve
        // basura.
        let is_series = matches!(q.kind, crate::tmdb::MediaKind::Series);
        let body = if is_series {
            // Query textual de fallback: "Title SxxEyy" o "Title Sxx".
            let text_query = build_series_query(&q.title, q.season, q.episode);
            let imdb = q.imdb_id.as_deref().map(|s| s.trim_start_matches("tt"));
            match self
                .fetch_tv(http, &text_query, imdb, q.season, q.episode)
                .await
            {
                Ok(body) => body,
                Err(_tv_err) => self.fetch(http, "search", &text_query, None).await?,
            }
        } else if let Some(id) = q.imdb_id.as_deref() {
            match self
                .fetch(
                    http,
                    "movie",
                    q.title.trim(),
                    Some(id.trim_start_matches("tt")),
                )
                .await
            {
                Ok(body) => body,
                Err(_movie_err) => {
                    // Fallback silencioso: no propagamos el error de
                    // capability. El caller de `search_all` recibirá
                    // resultados del `t=search` (o Err de ese segundo
                    // intento, que sí es genuino) — la telemetría de
                    // ProviderStatus captura ambas cosas.
                    self.fetch(http, "search", q.title.trim(), None).await?
                }
            }
        } else {
            self.fetch(http, "search", q.title.trim(), None).await?
        };

        let rss: Rss = quick_xml::de::from_str(&body).context("Error al parsear XML de Torznab")?;

        let mut out = Vec::new();
        for item in rss.channel.items {
            // El magnet puede venir en <link>, en el atributo `magneturl`, o
            // ser construible desde `infohash`.
            let magnet = if item.link.starts_with("magnet:") {
                Some(item.link.clone())
            } else if let Some(m) = item.attr("magneturl") {
                Some(m.to_string())
            } else if let Some(hash) = item.attr("infohash") {
                Some(build_magnet(hash, &item.title))
            } else {
                None
            };

            let Some(magnet) = magnet else { continue };
            let infohash = infohash_from_magnet(&magnet);
            if infohash.is_empty() {
                continue;
            }

            let seeders: u32 = item
                .attr("seeders")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let leechers: u32 = item
                .attr("peers")
                .or_else(|| item.attr("leechers"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let size_bytes = item
                .size
                .or_else(|| item.attr("size").and_then(|s| s.parse().ok()))
                .unwrap_or(0);
            let quality = quality_from_title(&item.title);

            out.push(Torrent {
                title: item.title,
                magnet,
                size_bytes,
                seeders,
                leechers,
                quality,
                source: "torznab".to_string(),
                match_kind: crate::torrents::MatchKind::default(),
                file_hint: None,
                infohash,
            });
        }

        Ok(out)
    }
}

impl Torznab {
    /// Pega a un endpoint Torznab con los params dados. `t_param` es
    /// `"movie"` o `"search"`; `imdbid` (sin `tt`) se añade cuando
    /// aplica. Devuelve el body XML crudo.
    async fn fetch(
        &self,
        http: &reqwest::Client,
        t_param: &str,
        query: &str,
        imdbid: Option<&str>,
    ) -> Result<String> {
        let mut url = format!(
            "{}?t={}&apikey={}&q={}",
            self.url.trim_end_matches('?'),
            t_param,
            urlencoding::encode(&self.apikey),
            urlencoding::encode(query),
        );
        if let Some(id) = imdbid {
            url.push_str("&imdbid=");
            url.push_str(id);
        }

        http.get(&url)
            .send()
            .await
            .context("Error de red hacia Torznab")?
            .error_for_status()
            .context("Torznab devolvi\u{f3} error HTTP")?
            .text()
            .await
            .context("Error al leer respuesta de Torznab")
    }

    /// Pega al endpoint `t=tvsearch` de Torznab con IMDb + season/ep
    /// cuando aplique. Es la ruta canónica para series y la de mayor
    /// precisión con Jackett/Prowlarr. Sin `ep`, devuelve todos los
    /// releases de la temporada (episodios y season packs).
    async fn fetch_tv(
        &self,
        http: &reqwest::Client,
        query: &str,
        imdbid: Option<&str>,
        season: Option<u16>,
        episode: Option<u16>,
    ) -> Result<String> {
        let mut url = format!(
            "{}?t=tvsearch&apikey={}&q={}",
            self.url.trim_end_matches('?'),
            urlencoding::encode(&self.apikey),
            urlencoding::encode(query),
        );
        if let Some(id) = imdbid {
            url.push_str("&imdbid=");
            url.push_str(id);
        }
        if let Some(s) = season {
            url.push_str(&format!("&season={}", s));
        }
        if let Some(e) = episode {
            url.push_str(&format!("&ep={}", e));
        }

        http.get(&url)
            .send()
            .await
            .context("Error de red hacia Torznab")?
            .error_for_status()
            .context("Torznab devolvi\u{f3} error HTTP")?
            .text()
            .await
            .context("Error al leer respuesta de Torznab")
    }
}

/// Construye el query textual de fallback para series:
/// `"Title S02E03"`, `"Title S02"` o `"Title"` según qué venga.
fn build_series_query(title: &str, season: Option<u16>, episode: Option<u16>) -> String {
    match (season, episode) {
        (Some(s), Some(e)) => format!("{} S{:02}E{:02}", title.trim(), s, e),
        (Some(s), None) => format!("{} S{:02}", title.trim(), s),
        _ => title.trim().to_string(),
    }
}
