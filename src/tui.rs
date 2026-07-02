use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;

use crate::auth;
use crate::config::Config;
use crate::letterboxd::LetterboxdClient;
use crate::progress::Progress;
use crate::recommend::{build_recommendations, Recommendation};
use crate::tmdb::TmdbClient;

const HELP: &str = "↑/↓ o j/k mover · r recargar · +/- min rating · [ ] nº resultados · q salir";

enum WorkerEvent {
    Stage(String, u64),
    Inc,
    Done(Vec<Recommendation>),
    Failed(String),
}

struct ChannelProgress {
    tx: mpsc::UnboundedSender<WorkerEvent>,
}

impl Progress for ChannelProgress {
    fn stage(&self, msg: &str, total: u64) {
        let _ = self.tx.send(WorkerEvent::Stage(msg.to_string(), total));
    }

    fn inc(&self) {
        let _ = self.tx.send(WorkerEvent::Inc);
    }

    fn finish(&self) {}
}

struct App {
    username: String,
    count: usize,
    min_rating: f32,
    recs: Vec<Recommendation>,
    list_state: ListState,
    loading: bool,
    stale: bool,
    stage_msg: String,
    stage_total: u64,
    stage_pos: u64,
    error: Option<String>,
}

impl App {
    fn new(username: String, count: usize, min_rating: f32) -> Self {
        Self {
            username,
            count,
            min_rating,
            recs: Vec::new(),
            list_state: ListState::default(),
            loading: false,
            stale: true,
            stage_msg: String::new(),
            stage_total: 0,
            stage_pos: 0,
            error: None,
        }
    }

    fn select_next(&mut self) {
        if self.recs.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) if i + 1 < self.recs.len() => i + 1,
            Some(_) => 0,
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn select_prev(&mut self) {
        if self.recs.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(0) | None => self.recs.len() - 1,
            Some(i) => i - 1,
        };
        self.list_state.select(Some(i));
    }
}

pub async fn run(
    config: Config,
    http: reqwest::Client,
    count: usize,
    min_rating: f32,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, config, http, count, min_rating).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: Config,
    http: reqwest::Client,
    count: usize,
    min_rating: f32,
) -> Result<()> {
    let mut app = App::new(config.username.clone(), count, min_rating);
    let mut rx: Option<mpsc::UnboundedReceiver<WorkerEvent>> = None;

    spawn_fetch(&config, &http, &mut app, &mut rx);

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        if let Some(r) = rx.as_mut() {
            while let Ok(evt) = r.try_recv() {
                match evt {
                    WorkerEvent::Stage(msg, total) => {
                        app.stage_msg = msg;
                        app.stage_total = total;
                        app.stage_pos = 0;
                    }
                    WorkerEvent::Inc => app.stage_pos += 1,
                    WorkerEvent::Done(recs) => {
                        app.loading = false;
                        app.stale = false;
                        app.error = None;
                        app.list_state
                            .select(if recs.is_empty() { None } else { Some(0) });
                        app.recs = recs;
                    }
                    WorkerEvent::Failed(e) => {
                        app.loading = false;
                        app.error = Some(e);
                    }
                }
            }
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
                    KeyCode::Char('r') if !app.loading => {
                        spawn_fetch(&config, &http, &mut app, &mut rx);
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        app.min_rating = (app.min_rating + 0.5).min(5.0);
                        app.stale = true;
                    }
                    KeyCode::Char('-') | KeyCode::Char('_') => {
                        app.min_rating = (app.min_rating - 0.5).max(0.5);
                        app.stale = true;
                    }
                    KeyCode::Char(']') => {
                        app.count += 5;
                        app.stale = true;
                    }
                    KeyCode::Char('[') => {
                        app.count = app.count.saturating_sub(5).max(1);
                        app.stale = true;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn spawn_fetch(
    config: &Config,
    http: &reqwest::Client,
    app: &mut App,
    rx: &mut Option<mpsc::UnboundedReceiver<WorkerEvent>>,
) {
    let (tx, new_rx) = mpsc::unbounded_channel();
    *rx = Some(new_rx);
    app.loading = true;
    app.stale = false;
    app.error = None;
    app.stage_msg = "Iniciando…".to_string();
    app.stage_total = 0;
    app.stage_pos = 0;

    let config = config.clone();
    let http = http.clone();
    let count = app.count;
    let min_rating = app.min_rating;

    tokio::spawn(async move {
        let progress = ChannelProgress { tx: tx.clone() };
        let result = async {
            let token = auth::get_access_token(&http, &config).await?;
            let lb = LetterboxdClient::new(&http, &token);
            let tmdb = TmdbClient::new(&http, &config.tmdb_bearer_token);
            build_recommendations(&lb, &tmdb, count, min_rating, &progress).await
        }
        .await;

        match result {
            Ok(recs) => {
                let _ = tx.send(WorkerEvent::Done(recs));
            }
            Err(e) => {
                let _ = tx.send(WorkerEvent::Failed(e.to_string()));
            }
        }
    });
}

fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "🎬 letterboxd-cli",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  —  {}   ★ ≥ {:.1}   top {}",
            app.username, app.min_rating, app.count
        )),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    let items: Vec<ListItem> = if app.recs.is_empty() && !app.loading {
        vec![ListItem::new(
            "  (sin recomendaciones todavía — pulsa 'r' para cargar)",
        )]
    } else {
        app.recs
            .iter()
            .enumerate()
            .map(|(i, rec)| {
                let rating = match rec.lb_rating {
                    Some(r) => format!("★ {r:.2}"),
                    None => format!("★ {:.2} (TMDB)", rec.movie.vote_average / 2.0),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:>2}. ", i + 1),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("{:<40}", rec.movie.title),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(rating, Style::default().fg(Color::Yellow)),
                ]))
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Recomendaciones "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, chunks[1], &mut app.list_state);

    let status = if let Some(err) = &app.error {
        Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        ))
    } else if app.loading {
        let progress = if app.stage_total > 0 {
            format!("{} ({}/{})", app.stage_msg, app.stage_pos, app.stage_total)
        } else {
            app.stage_msg.clone()
        };
        Line::from(Span::styled(progress, Style::default().fg(Color::Cyan)))
    } else if app.stale {
        Line::from(Span::styled(
            "Parámetros modificados — pulsa 'r' para recargar",
            Style::default().fg(Color::Yellow),
        ))
    } else {
        Line::from(Span::raw(format!(
            "{HELP}  ({} resultados)",
            app.recs.len()
        )))
    };

    let footer = Paragraph::new(status).block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
