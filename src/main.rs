// En Windows, la build con GUI se compila como `windows` subsystem para
// que al hacer doble click en el atajo del Start Menu NO se abra también
// una ventana de consola (además de robar el foco y romper los hotkeys
// de la Tauri window). Para CLI/TUI seguimos usando la consola: si el
// usuario invoca el binario desde PowerShell/cmd con argumentos, en
// `main()` hacemos `AttachConsole(ATTACH_PARENT_PROCESS)` para que
// `println!`/`eprintln!` acaben en el terminal que le lanzó.
#![cfg_attr(
    all(target_os = "windows", feature = "gui", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod auth;
mod config;
mod credentials;
#[cfg(feature = "gui")]
mod dismissed;
#[cfg(feature = "gui")]
mod ffmpeg;
#[cfg(feature = "gui")]
mod gui;
mod keychain;
mod letterboxd;
#[cfg(feature = "gui")]
mod preferences;
mod progress;
mod recommend;
mod stream;
mod subtitles;
mod tmdb;
mod torrents;
mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;

use config::Config;
use letterboxd::LetterboxdClient;
use progress::CliProgress;
use recommend::build_recommendations;
use tmdb::TmdbClient;

#[derive(Parser)]
#[command(
    name = "videodrome",
    about = "Recomendaciones de películas desde Letterboxd"
)]
struct Cli {
    /// Subcomando a ejecutar. Si se omite, arranca la TUI con los valores
    /// por defecto (`count=10`, `min_rating=4.0`).
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Genera recomendaciones de películas basadas en tu historial de Letterboxd
    Recommend {
        /// Número de recomendaciones a mostrar
        #[arg(short, long, default_value_t = 10)]
        count: usize,

        /// Rating mínimo propio para semillas (escala 0.5–5.0)
        #[arg(short = 'r', long, default_value_t = 4.0)]
        min_rating: f32,

        /// Imprime las recomendaciones como JSON en lugar de texto formateado
        #[arg(long)]
        json: bool,
    },

    /// Abre una interfaz interactiva (TUI) para explorar recomendaciones
    Tui {
        /// Número de recomendaciones a mostrar
        #[arg(short, long, default_value_t = 10)]
        count: usize,

        /// Rating mínimo propio para semillas (escala 0.5–5.0)
        #[arg(short = 'r', long, default_value_t = 4.0)]
        min_rating: f32,
    },

    /// Gestiona las credenciales guardadas en el Keychain de macOS
    Keychain {
        #[command(subcommand)]
        action: KeychainAction,
    },

    /// Busca torrents para una película en varios proveedores (YTS, Knaben, Torznab)
    Torrents {
        /// Título de la película (posicional). Si se omite, es obligatorio --imdb.
        #[arg(required_unless_present = "imdb")]
        title: Option<String>,

        /// IMDb ID (con o sin prefijo `tt`) — recomendado para búsqueda precisa
        #[arg(long)]
        imdb: Option<String>,

        /// Año de estreno (ayuda a desambiguar remakes)
        #[arg(short, long)]
        year: Option<u16>,

        /// TMDB ID (informativo; algunos providers lo usan)
        #[arg(long)]
        tmdb_id: Option<u64>,

        /// Filtrar por seeders mínimos
        #[arg(long, default_value_t = 3)]
        min_seeders: u32,

        /// Número máximo de resultados a mostrar
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,

        /// Serie: temporada a buscar. Si se combina con `--episode`
        /// busca el episodio exacto; sin `--episode` busca packs de
        /// temporada. Requiere que el título/imdb sean de una serie
        /// (los providers de series lo asumen a partir de
        /// `kind=Series`).
        #[arg(long)]
        season: Option<u16>,

        /// Serie: episodio a buscar. Requiere `--season`.
        #[arg(long, requires = "season")]
        episode: Option<u16>,

        /// Imprime como JSON en lugar de texto formateado
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum KeychainAction {
    /// Lee las credenciales actuales de .env y las guarda en el Keychain
    Import,
    /// Vuelca las credenciales del Keychain a un .env (por defecto
    /// `~/.config/videodrome/.env`). Útil para evitar el diálogo de
    /// aprobación del Keychain en cada ejecución.
    Export {
        /// Ruta del fichero .env de destino
        #[arg(long)]
        to: Option<std::path::PathBuf>,
        /// Sobreescribe el fichero destino si ya existe
        #[arg(long)]
        force: bool,
    },
    /// Elimina las credenciales guardadas en el Keychain
    Clear,
}

#[cfg(feature = "gui")]
fn has_display() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// Cliente HTTP compartido para todas las llamadas a APIs externas
/// (Letterboxd, TMDB, providers de torrents, OpenSubtitles). Un timeout
/// razonable evita cuelgues indefinidos si un endpoint no responde —
/// especialmente relevante en la TUI, donde una request colgada bloquea
/// el spinner sin feedback al usuario.
fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("videodrome/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("No se pudo construir el cliente HTTP")
}

fn main() -> Result<()> {
    // Windows: si nos han lanzado desde una terminal (con subcomando),
    // engancharse a la consola padre para que la salida del CLI/TUI
    // aparezca allí. En modo GUI (sin args) no atachamos → sin ventana
    // extra de cmd. Ver la nota de `windows_subsystem` arriba del fichero.
    #[cfg(all(target_os = "windows", feature = "gui", not(debug_assertions)))]
    {
        if std::env::args_os().len() > 1 {
            #[link(name = "kernel32")]
            extern "system" {
                fn AttachConsole(dw_process_id: u32) -> i32;
            }
            const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
            unsafe {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }

    let cli = Cli::parse();

    // Sin subcomando explícito + feature `gui` activa + hay display:
    // arrancamos la GUI Tauri. Tauri exige el main thread, así que no
    // podemos vivir dentro de `#[tokio::main]`. En el resto de casos
    // creamos un runtime tokio manual antes de despachar el subcomando.
    #[cfg(feature = "gui")]
    if cli.command.is_none() && has_display() {
        let config = Config::from_env()?;
        return gui::run(config, http_client()?);
    }

    let command = cli.command.unwrap_or(Commands::Tui {
        count: 10,
        min_rating: 4.0,
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(dispatch(command))
}

async fn dispatch(command: Commands) -> Result<()> {
    match command {
        Commands::Recommend {
            count,
            min_rating,
            json,
        } => {
            let config = Config::from_env()?;
            let http = http_client()?;

            let token = auth::get_access_token(&http, &config).await?;

            let lb = LetterboxdClient::new(&http, &token);
            let tmdb = TmdbClient::new(&http, &config.tmdb_bearer_token);

            let recs =
                build_recommendations(&lb, &tmdb, count, min_rating, &CliProgress::new()).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&recs)?);
                return Ok(());
            }

            println!(
                "\n  {}\n",
                format!("Recomendaciones para {}", config.username).bold()
            );

            for (i, rec) in recs.iter().enumerate() {
                let rating_str = match rec.lb_rating {
                    Some(r) => format!("{:.2}", r).yellow().to_string(),
                    None => format!("{:.2} (TMDB)", rec.movie.vote_average / 2.0)
                        .dimmed()
                        .to_string(),
                };
                println!(
                    "  {}  {:<42} ★ {}",
                    format!("{:>2}.", i + 1).dimmed(),
                    rec.movie.title.white().bold(),
                    rating_str,
                );
            }

            if recs.is_empty() {
                println!("  {}", "No se encontraron recomendaciones.".dimmed());
            }

            println!();
        }
        Commands::Tui { count, min_rating } => {
            let config = Config::from_env()?;
            tui::run(config, http_client()?, count, min_rating).await?;
        }
        Commands::Keychain { action } => match action {
            KeychainAction::Import => {
                config::load_env_files();

                let entries = [
                    ("LETTERBOXD_CLIENT_ID", keychain::CLIENT_ID),
                    ("LETTERBOXD_CLIENT_SECRET", keychain::CLIENT_SECRET),
                    ("LETTERBOXD_REFRESH_TOKEN", keychain::REFRESH_TOKEN),
                    ("LETTERBOXD_USERNAME", keychain::USERNAME),
                    ("TMDB_BEARER_TOKEN", keychain::TMDB_BEARER_TOKEN),
                ];

                let mut imported = 0usize;
                let mut skipped = Vec::new();
                for (env_key, kc) in entries {
                    match std::env::var(env_key) {
                        Ok(val) if !val.is_empty() => {
                            keychain::set(kc, &val)?;
                            imported += 1;
                            println!("  {} {} → {}", "✔".green(), env_key, kc);
                        }
                        _ => skipped.push(env_key),
                    }
                }

                if imported == 0 {
                    anyhow::bail!(
                        "No se encontró ninguna variable en el entorno ni en \
                         ~/.config/videodrome/.env. Crea un .env con al \
                         menos una de las variables antes de importar."
                    );
                }

                println!(
                    "\n{}",
                    format!("{imported} credencial(es) guardada(s) en el Keychain.").green()
                );
                if !skipped.is_empty() {
                    println!(
                        "  {} sin cambios (no estaban en .env): {}",
                        "•".dimmed(),
                        skipped.join(", ").dimmed()
                    );
                }
            }
            KeychainAction::Export { to, force } => {
                let path = to.unwrap_or_else(|| {
                    dirs::home_dir()
                        .expect("HOME no definido")
                        .join(".config")
                        .join("videodrome")
                        .join(".env")
                });

                if path.exists() && !force {
                    anyhow::bail!(
                        "{} ya existe. Usa --force para sobreescribir.",
                        path.display()
                    );
                }

                // Lee del Keychain — esto puede disparar los diálogos de
                // aprobación una vez por credencial (5 en total). Tras
                // exportar, las siguientes ejecuciones leerán del .env y no
                // se volverá a preguntar.
                let entries = [
                    ("LETTERBOXD_CLIENT_ID", keychain::CLIENT_ID),
                    ("LETTERBOXD_CLIENT_SECRET", keychain::CLIENT_SECRET),
                    ("LETTERBOXD_REFRESH_TOKEN", keychain::REFRESH_TOKEN),
                    ("LETTERBOXD_USERNAME", keychain::USERNAME),
                    ("TMDB_BEARER_TOKEN", keychain::TMDB_BEARER_TOKEN),
                ];

                let mut lines = Vec::new();
                let mut written = 0usize;
                let mut missing = Vec::new();
                for (env_key, kc) in entries {
                    match keychain::get(kc) {
                        Some(v) if !v.is_empty() => {
                            lines.push(format!("{env_key}={v}"));
                            written += 1;
                        }
                        _ => missing.push(env_key),
                    }
                }

                if written == 0 {
                    anyhow::bail!("El Keychain no tiene ninguna credencial de videodrome.");
                }

                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("No se pudo crear el directorio {}", parent.display())
                    })?;
                }
                let content = format!("{}\n", lines.join("\n"));
                std::fs::write(&path, content)
                    .with_context(|| format!("No se pudo escribir {}", path.display()))?;

                // Permisos 600 para no dejar el .env legible por otros
                // usuarios del sistema.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }

                println!(
                    "{} {} credencial(es) volcada(s) a {} (chmod 600)",
                    "✔".green(),
                    written,
                    path.display()
                );
                if !missing.is_empty() {
                    println!(
                        "  {} no estaban en el Keychain: {}",
                        "•".dimmed(),
                        missing.join(", ").dimmed()
                    );
                }
            }
            KeychainAction::Clear => {
                keychain::delete(keychain::CLIENT_ID)?;
                keychain::delete(keychain::CLIENT_SECRET)?;
                keychain::delete(keychain::REFRESH_TOKEN)?;
                keychain::delete(keychain::USERNAME)?;
                keychain::delete(keychain::TMDB_BEARER_TOKEN)?;

                println!(
                    "{}",
                    "Credenciales eliminadas del Keychain de macOS.".green()
                );
            }
        },
        Commands::Torrents {
            title,
            imdb,
            year,
            tmdb_id,
            min_seeders,
            limit,
            season,
            episode,
            json,
        } => {
            config::load_env_files();

            let http = http_client()?;

            let imdb_norm = imdb.as_ref().map(|s| {
                let s = s.trim();
                if s.starts_with("tt") {
                    s.to_string()
                } else {
                    format!("tt{s}")
                }
            });

            // Si el usuario solo dio --imdb sin título, resolvemos título+año
            // vía TMDB. Los providers necesitan keywords legibles (Knaben,
            // Torznab) o al menos un query_term (YTS) — pasarles "tt0120737"
            // no sirve.
            let (mut effective_title, mut effective_year) = (title.clone(), year);
            if effective_title.is_none() {
                if let Some(id) = imdb_norm.as_deref() {
                    let bearer = config::tmdb_bearer().context(
                        "Se ha pasado --imdb sin título, pero TMDB_BEARER_TOKEN \
                         no está configurado (necesario para resolver IMDb → título).",
                    )?;
                    let tmdb = TmdbClient::new(&http, &bearer);
                    match tmdb.find_by_imdb(id).await? {
                        Some(lookup) => {
                            if !json {
                                let y = lookup.year.map(|y| format!(" ({y})")).unwrap_or_default();
                                println!(
                                    "  {} IMDb {} → {}{}",
                                    "»".dimmed(),
                                    id.dimmed(),
                                    lookup.title.bold(),
                                    y.dimmed()
                                );
                            }
                            effective_title = Some(lookup.title);
                            if effective_year.is_none() {
                                effective_year = lookup.year;
                            }
                        }
                        None => {
                            anyhow::bail!("TMDB no conoce el IMDb ID {id}");
                        }
                    }
                }
            }

            let display_title = effective_title
                .clone()
                .or_else(|| imdb_norm.clone())
                .unwrap_or_else(|| "?".to_string());

            let query = torrents::MovieQuery {
                title: effective_title.unwrap_or_default(),
                year: effective_year,
                imdb_id: imdb_norm,
                tmdb_id,
                original_language: None,
                title_variants: Vec::new(),
                kind: if season.is_some() {
                    crate::tmdb::MediaKind::Series
                } else {
                    crate::tmdb::MediaKind::Movie
                },
                season,
                episode,
            };

            let providers = torrents::default_providers();
            let provider_names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
            if !json {
                println!(
                    "\n  Buscando torrents para {} (providers: {})...",
                    display_title.bold(),
                    provider_names.join(", ").dimmed()
                );
            }

            let outcome = torrents::search_all(&http, &providers, &query, min_seeders, limit).await;

            if json {
                // Serializamos el outcome completo (results +
                // providers) — el consumidor decide qué mirar.
                println!("{}", serde_json::to_string_pretty(&outcome)?);
                return Ok(());
            }

            // Fase 1b — observabilidad: al final del listado
            // imprimimos una línea con el estado por provider.
            let status_line: Vec<String> = outcome
                .providers
                .iter()
                .map(|s| {
                    if s.ok {
                        format!("{} ✓ {}", s.name, s.hits)
                    } else {
                        format!("{} ✗ {}", s.name, s.error.as_deref().unwrap_or("error"))
                    }
                })
                .collect();

            let results = outcome.results;
            if results.is_empty() {
                println!("\n  {}", "Sin resultados con esos filtros.".dimmed());
                if !status_line.is_empty() {
                    println!("  {}", status_line.join(" · ").dimmed());
                }
                return Ok(());
            }

            println!();
            for (i, t) in results.iter().enumerate() {
                let q = t.quality.as_deref().unwrap_or("?");
                let size = torrents::format_size(t.size_bytes);
                println!(
                    "  {}  {}",
                    format!("{:>2}.", i + 1).dimmed(),
                    t.title.white().bold()
                );
                println!(
                    "      {} · seeds {} · leech {} · {} · {}",
                    size.yellow(),
                    t.seeders.to_string().green(),
                    t.leechers.to_string().red(),
                    q.cyan(),
                    t.source.dimmed()
                );
                println!("      {}", t.magnet.dimmed());
            }
            if !status_line.is_empty() {
                println!("\n  {}", status_line.join(" · ").dimmed());
            }
            println!();
        }
    }

    Ok(())
}
