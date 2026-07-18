//! Provider YTS (yts.mx). API JSON pública, sin auth. Solo cine.
//! Docs: <https://yts.mx/api>

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::{build_magnet, MovieQuery, Torrent, TorrentProvider};

/// Lista de hosts YTS a probar en orden. La API JSON es idéntica en
/// todos — mismo path, mismos parámetros, mismo schema — así que el
/// primer host que responda 200 gana. Motivo: `yts.mx` está bloqueado
/// por DNS en muchos ISP europeos (Movistar/Vodafone en ES),
/// devolviendo un `error de red` que hunde el provider entero en el
/// `ProviderStatus`. Con la lista intentamos degradadamente hasta
/// encontrar uno accesible desde la red del user.
///
/// Orden:
///   1. `movies-api.accel.li` — backend OFICIAL nuevo. La propia API
///      de yts.am anuncia el traslado en el campo `status_message`
///      ("Base URL moving to https://movies-api.accel.li/api/v2/").
///      Es CDN puro, sin frontend HTML → no lo bloquean los DNS
///      filters que persiguen el dominio `yts.*`. Debería funcionar
///      desde cualquier ISP.
///   2. `yts.am` — espejo estable con el mismo backend expuesto en
///      HTML + API. Fallback si accel.li tuviera problemas.
///   3. `yts.mx` / `yts.rs` — canónico + oficial. Suelen estar
///      bloqueados por ISP pero se dejan por si el user está fuera
///      de una jurisdicción con censura.
///   4. `yts.hn` — mirror comunitario. A veces está caído (HTTP 500).
///
/// NOTA: NO añadas hosts random tipo `yts.movie` — algunos son
/// squatters con contenido totalmente distinto (páginas de apuestas
/// indonesias servían HTML en vez de JSON, comprobado 2026-07). Antes
/// de meter un mirror nuevo, verifica con curl que el JSON del
/// endpoint `/api/v2/list_movies.json` sea genuino de YTS
/// (`status: "ok"`, `movies[].imdb_code`).
const YTS_HOSTS: &[&str] = &[
    "https://movies-api.accel.li",
    "https://yts.am",
    "https://yts.mx",
    "https://yts.rs",
    "https://yts.hn",
];

/// Timeout corto POR HOST — no queremos gastar los 8s de budget del
/// provider (definido en `super::PROVIDER_TIMEOUT`) en un solo mirror
/// muerto. Con 4 hosts × 2s salen 8s peor caso, encajando justo con
/// el timeout global antes de que `run_provider` corte.
const YTS_HOST_TIMEOUT: Duration = Duration::from_millis(2000);

/// User-Agent tipo navegador para las peticiones a YTS. Los mirrors
/// suelen sentarse detrás de Cloudflare y devuelven `403` / `503` a
/// cualquier UA no-browser (incluido `videodrome/x.y.z`). Este UA de
/// Firefox actual pasa el challenge estándar y evita el falso
/// positivo de "HTTP status server error" cuando la API está viva.
const YTS_BROWSER_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0";

pub struct Yts;

#[derive(Debug, Deserialize)]
struct YtsResponse {
    data: YtsData,
}

#[derive(Debug, Deserialize, Default)]
struct YtsData {
    #[serde(default)]
    movies: Option<Vec<YtsMovie>>,
}

#[derive(Debug, Deserialize)]
struct YtsMovie {
    title_long: String,
    #[serde(default)]
    year: Option<u16>,
    #[serde(default)]
    imdb_code: String,
    #[serde(default)]
    torrents: Vec<YtsTorrent>,
}

#[derive(Debug, Deserialize)]
struct YtsTorrent {
    hash: String,
    #[serde(default)]
    quality: String,
    #[serde(default)]
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    seeds: u32,
    #[serde(default)]
    peers: u32,
    #[serde(default)]
    size_bytes: u64,
}

#[async_trait]
impl TorrentProvider for Yts {
    fn name(&self) -> &'static str {
        "yts"
    }

    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>> {
        // Preferimos IMDb ID (mucho más preciso). Si no, caemos al título.
        let query_term = q.imdb_id.clone().unwrap_or_else(|| q.title.clone());
        let path = format!(
            "/api/v2/list_movies.json?query_term={}&limit=5&sort_by=seeds&order_by=desc",
            urlencoding::encode(&query_term)
        );

        let resp = fetch_from_any_host(http, &path).await?;

        // YTS puede devolver varias películas con títulos parecidos
        // ("Alien", "Aliens", "Alien 3"…). Antes se pusheaban torrents
        // de todas ellas antes de decidir cuál era la buscada, así que
        // sin --year el resultado mezclaba pelis distintas. Ahora
        // seleccionamos una sola película y solo devolvemos sus
        // torrents:
        //   - Si viene `imdb_id`, la que coincida por IMDb.
        //   - Si no, la primera cuyo título normalizado coincida
        //     exactamente con el buscado (después del filtro de año).
        let target = norm_title(&q.title);
        let movies = resp.data.movies.unwrap_or_default();
        let picked = movies.into_iter().find(|m| {
            if let (Some(want), Some(got)) = (q.year, m.year) {
                if (want as i32 - got as i32).abs() > 1 {
                    return false;
                }
            }
            if let Some(imdb) = q.imdb_id.as_deref() {
                m.imdb_code == imdb
            } else {
                norm_title(&m.title_long) == target
            }
        });

        let Some(m) = picked else {
            return Ok(Vec::new());
        };

        let mut out = Vec::with_capacity(m.torrents.len());
        for t in m.torrents {
            let display = format!("{} [{}] {}", m.title_long, t.quality, t.kind);
            let magnet = build_magnet(&t.hash, &display);
            out.push(Torrent {
                title: display,
                magnet,
                size_bytes: t.size_bytes,
                seeders: t.seeds,
                leechers: t.peers,
                quality: Some(t.quality),
                source: "yts".to_string(),
                match_kind: crate::torrents::MatchKind::default(),
                infohash: t.hash.to_ascii_uppercase(),
            });
        }

        Ok(out)
    }
}

/// Normaliza un título para comparar YTS con la query del user:
/// lowercase, quita el año trailing (`(1979)`), y colapsa todo lo no
/// alfanumérico a espacios simples. `norm_title("Alien (1979)") ==
/// "alien"` y `norm_title("Aliens") == "aliens"`.
fn norm_title(s: &str) -> String {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter(|w| !(w.len() == 4 && w.chars().all(|c| c.is_ascii_digit())))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Prueba cada host de `YTS_HOSTS` en orden hasta que uno responda
/// 200 con JSON válido, y devuelve el `YtsResponse` parseado.
/// Si todos fallan, propaga el último error (el `ProviderStatus`
/// lo mostrará como `yts ✗ <motivo>`). Extraído de `search` en
/// una fn aparte por el clippy `never_loop` — el bucle exterior
/// del original solo iteraba una vez (siempre `break 'hosts` o
/// `return Err`), y clippy lo detecta como código muerto.
async fn fetch_from_any_host(http: &reqwest::Client, path: &str) -> Result<YtsResponse> {
    let mut last_err: Option<anyhow::Error> = None;
    for host in YTS_HOSTS {
        let url = format!("{host}{path}");
        let attempt = tokio::time::timeout(YTS_HOST_TIMEOUT, async {
            http.get(&url)
                .header(reqwest::header::USER_AGENT, YTS_BROWSER_UA)
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await
                .with_context(|| format!("Error de red hacia {host}"))?
                .error_for_status()
                .with_context(|| format!("{host} devolvi\u{f3} error HTTP"))?
                .json::<YtsResponse>()
                .await
                .with_context(|| format!("Error al parsear respuesta de {host}"))
        })
        .await;
        match attempt {
            Ok(Ok(parsed)) => return Ok(parsed),
            Ok(Err(e)) => last_err = Some(e),
            Err(_elapsed) => {
                last_err = Some(anyhow::anyhow!("{host}: timeout (>2s)"));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("YTS: sin hosts alcanzables")))
}
