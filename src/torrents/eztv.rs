//! Provider EZTV. API JSON pública, sin auth. Solo series.
//! Docs: <https://eztv.re/api/>
//!
//! Endpoint:
//!
//!   GET https://<host>/api/get-torrents?imdb_id=<digits>&limit=100&page=N
//!
//! Devuelve JSON con `torrents[] { hash, filename, title, season, episode,
//! magnet_url, seeds, peers, size_bytes, ... }`. Todos los campos
//! season/episode vienen YA parseados por EZTV (no hay que adivinar
//! desde el filename), lo que hace este provider particularmente
//! preciso para packs que otros indexers etiquetan mal.
//!
//! Es el análogo de YTS para series: direccionable por IMDb, ruido
//! bajo, sin auth. Para películas devuelve vacío en silencio.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::{build_magnet, MovieQuery, Torrent, TorrentProvider};

/// Lista de hosts EZTV a probar en orden. Todos exponen la misma API
/// JSON — mismo path, mismos parámetros, mismo schema — así que el
/// primer host que responda 200 con `torrents[]` gana. Motivo: el
/// dominio canónico `eztv.re` está BLOQUEADO POR DNS en muchos ISP
/// europeos (Movistar/Vodafone/Orange en ES, TIM en IT), devolviendo
/// "Could not resolve host" que hunde el provider entero al `ok=false`
/// del ProviderStatus.
///
/// Orden:
///   1. `eztvx.to` — dominio "oficial" del catálogo desde el
///      re-launch de 2024. Estable, sin filtros DNS habituales.
///   2. `eztv.wf` — mirror comunitario con el mismo backend.
///      Fallback si `.to` tuviera problemas puntuales.
///   3. `eztv.re` — canónico histórico. Bloqueado en muchos ISP
///      europeos pero lo dejamos por si el user está fuera de una
///      jurisdicción con censura.
///
/// NOTA: NO añadas `eztv.ag`, `eztv.it` u otros dominios encontrados
/// por búsqueda — son squatters con contenido distinto (o vacío)
/// desde 2023. Antes de meter un mirror nuevo, verifica con curl
/// que `/api/get-torrents?imdb_id=5491994&limit=1` devuelve JSON con
/// `torrents_count > 0`.
const EZTV_HOSTS: &[&str] = &["https://eztvx.to", "https://eztv.wf", "https://eztv.re"];

/// Timeout corto POR HOST — no queremos gastar los 8s de budget del
/// provider (definido en `super::PROVIDER_TIMEOUT`) en un solo mirror
/// muerto. Con 3 hosts × 2.5s salen ~7.5s peor caso, encajando justo
/// con el timeout global antes de que `run_provider` corte.
const EZTV_HOST_TIMEOUT: Duration = Duration::from_millis(2500);

/// User-Agent tipo navegador. Los mirrors de EZTV pueden sentarse
/// tras Cloudflare/DDoS-Guard y devolver 403/503 a UAs no-browser
/// (incluido `videodrome/x.y.z`). Este UA de Firefox reciente pasa
/// el challenge estándar y evita el falso positivo de "HTTP status
/// server error" cuando la API está viva.
const EZTV_BROWSER_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0";

pub struct Eztv;

#[derive(Debug, Deserialize)]
struct EztvResponse {
    #[serde(default)]
    torrents: Vec<EztvItem>,
}

#[derive(Debug, Deserialize)]
struct EztvItem {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    magnet_url: String,
    /// EZTV los expone como strings ("1", "01") — parseamos ambos.
    #[serde(default)]
    season: Option<serde_json::Value>,
    #[serde(default)]
    episode: Option<serde_json::Value>,
    #[serde(default)]
    seeds: u32,
    #[serde(default)]
    peers: u32,
    #[serde(default)]
    size_bytes: SizeBytes,
}

/// EZTV a veces devuelve `size_bytes` como string ("734003200") en
/// vez de número. Aceptamos ambos.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum SizeBytes {
    Int(u64),
    Str(String),
    #[default]
    Missing,
}

impl SizeBytes {
    fn into_u64(self) -> u64 {
        match self {
            SizeBytes::Int(n) => n,
            SizeBytes::Str(s) => s.parse().unwrap_or(0),
            SizeBytes::Missing => 0,
        }
    }
}

fn parse_num(v: &Option<serde_json::Value>) -> Option<u16> {
    let v = v.as_ref()?;
    if let Some(n) = v.as_u64() {
        return u16::try_from(n).ok();
    }
    if let Some(s) = v.as_str() {
        return s.parse().ok();
    }
    None
}

#[async_trait]
impl TorrentProvider for Eztv {
    fn name(&self) -> &'static str {
        "eztv"
    }

    async fn search(&self, http: &reqwest::Client, q: &MovieQuery) -> Result<Vec<Torrent>> {
        // Solo series. Cualquier otra kind devuelve vacío en silencio
        // — no queremos ensuciar el ProviderStatus con un fallo que
        // es simplemente "fuera de scope".
        if !matches!(q.kind, crate::tmdb::MediaKind::Series) {
            return Ok(Vec::new());
        }
        // EZTV solo direcciona por IMDb (y sin el prefijo "tt"). Sin
        // ID no hay forma de pegarle — mejor devolver vacío que
        // hacer una petición sin foco.
        let Some(imdb) = q.imdb_id.as_deref() else {
            return Ok(Vec::new());
        };
        let imdb_num = imdb.trim_start_matches("tt");
        if imdb_num.is_empty() || !imdb_num.chars().all(|c| c.is_ascii_digit()) {
            return Ok(Vec::new());
        }

        let path = format!("/api/get-torrents?imdb_id={imdb_num}&limit=100&page=1");
        let parsed = fetch_from_any_host(http, &path).await?;

        let mut out = Vec::with_capacity(parsed.torrents.len());
        for it in parsed.torrents {
            // Filtro por season/episode pedidos: si el user pidió
            // S02E03, no devolvemos S01E04. Si pidió S02 (pack),
            // aceptamos episodios de S02 + el propio pack (episode
            // ausente o 0). Si no pidió nada, todo pasa.
            let s = parse_num(&it.season);
            let e = parse_num(&it.episode);
            if let Some(qs) = q.season {
                match s {
                    Some(ss) if ss == qs => {}
                    _ => continue,
                }
                if let Some(qe) = q.episode {
                    match e {
                        Some(ee) if ee == qe => {}
                        // Episode 0 en EZTV suele ser season pack.
                        Some(0) | None => {}
                        _ => continue,
                    }
                }
            }

            let magnet = if it.magnet_url.starts_with("magnet:") {
                it.magnet_url
            } else if !it.hash.is_empty() {
                build_magnet(&it.hash, &it.title)
            } else {
                continue;
            };
            let hash_up = it.hash.to_ascii_uppercase();
            if hash_up.is_empty() {
                continue;
            }

            out.push(Torrent {
                title: it.title.clone(),
                magnet,
                size_bytes: it.size_bytes.into_u64(),
                seeders: it.seeds,
                leechers: it.peers,
                // EZTV no expone quality como campo — se infiere del
                // título con `quality_from_title` (720p/1080p/2160p).
                quality: super::quality_from_title(&it.title),
                source: "eztv".to_string(),
                match_kind: crate::torrents::MatchKind::default(),
                // EZTV podría exponer fileIdx en algún caso pero su
                // API no lo devuelve fiablemente — dejamos None.
                file_hint: None,
                infohash: hash_up,
            });
        }

        Ok(out)
    }
}

/// Prueba cada host de `EZTV_HOSTS` en orden hasta que uno responda
/// 200 con JSON parseable. Errores de red / HTTP / parse en un host
/// no propagan al caller — se registran en stderr y se sigue con el
/// siguiente. Solo cuando TODOS fallan devolvemos el último error.
///
/// Timeout POR HOST vía `EZTV_HOST_TIMEOUT` para no gastar todo el
/// budget del provider en un mirror muerto.
async fn fetch_from_any_host(http: &reqwest::Client, path: &str) -> Result<EztvResponse> {
    let mut last_err: Option<anyhow::Error> = None;
    for host in EZTV_HOSTS {
        let url = format!("{host}{path}");
        let fut = http
            .get(&url)
            .header(reqwest::header::USER_AGENT, EZTV_BROWSER_UA)
            .send();
        let resp = match tokio::time::timeout(EZTV_HOST_TIMEOUT, fut).await {
            Err(_) => {
                last_err = Some(anyhow::anyhow!("timeout {host}"));
                tracing::warn!(target: "eztv", host, "timeout");
                continue;
            }
            Ok(Err(e)) => {
                // DNS block / connection reset / TLS. Silencio a
                // stderr y probamos el siguiente.
                tracing::warn!(target: "eztv", host, error = %e, "red");
                last_err = Some(anyhow::Error::from(e));
                continue;
            }
            Ok(Ok(r)) => r,
        };
        let status = resp.status();
        if !status.is_success() {
            tracing::warn!(target: "eztv", host, %status, "HTTP error");
            last_err = Some(anyhow::anyhow!("{host} HTTP {status}"));
            continue;
        }
        match resp.json::<EztvResponse>().await {
            Ok(parsed) => return Ok(parsed),
            Err(e) => {
                // JSON malformado (típico: Cloudflare devuelve HTML
                // de challenge con 200). Probar el siguiente host.
                tracing::warn!(target: "eztv", host, error = %e, "parse");
                last_err = Some(anyhow::Error::from(e));
                continue;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Sin hosts EZTV disponibles")))
        .context("Todos los mirrors de EZTV fallaron")
}
