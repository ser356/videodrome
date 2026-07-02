mod auth;
mod config;
mod letterboxd;
mod progress;
mod recommend;
mod tmdb;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;

use config::Config;
use letterboxd::LetterboxdClient;
use progress::CliProgress;
use recommend::build_recommendations;
use tmdb::TmdbClient;

#[derive(Parser)]
#[command(
    name = "letterboxd-cli",
    about = "Recomendaciones de películas desde Letterboxd"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_env()?;

    let http = reqwest::Client::builder()
        .user_agent("letterboxd-cli/0.1")
        .build()?;

    match cli.command {
        Commands::Recommend {
            count,
            min_rating,
            json,
        } => {
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
            tui::run(config, http, count, min_rating).await?;
        }
    }

    Ok(())
}
