use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;

use crate::auth;
use crate::config::Config;
use crate::letterboxd::LetterboxdClient;
use crate::progress::Progress;
use crate::recommend::{build_recommendations, Recommendation};
use crate::subtitles::{self, Subtitle};
use crate::tmdb::TmdbClient;
use crate::torrents::{self, release_starts_with, split_trailing_year, MovieQuery, Torrent};

const HELP_MENU: &str = "j/k mover · Enter seleccionar · q salir";
const HELP_RECS: &str =
    "j/k mover · t torrents · r recargar · -/+ rating · [/] top · b menú · q salir";
const HELP_TORRENTS: &str =
    "j/k mover · Enter magnet · s stream · x subtítulos · m panel · Esc volver · q salir";
const HELP_SEARCH: &str = "escribe título · Enter buscar · Esc volver";
const HELP_SUBS: &str = "j/k mover · Enter descargar · Esc volver · q salir";

/// Frames del spinner braille (10 pasos, ~100 ms cada uno).
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

enum WorkerEvent {
    Stage(String, u64),
    Inc,
    Done(Vec<Recommendation>),
    Failed(String),
}

/// Eventos del worker de torrents. Independiente del de recomendaciones para
/// que cada uno tenga su propio canal y estado.
enum TorrentEvent {
    Status(String),
    /// Idioma original de la película (ISO 639-1). Se emite al principio
    /// para que la TUI pueda clasificar el audio de cada release cuando
    /// llegue el Done.
    Language(Option<String>),
    /// IMDb ID resuelto vía TMDB. Se guarda en `App` para poder usarlo
    /// luego como filtro al buscar subtítulos en OpenSubtitles.
    Imdb(Option<String>),
    Done(Vec<Torrent>),
    Failed(String),
}

/// Eventos del worker de streaming.
enum StreamEvent {
    Starting(String),
    Ready(Box<crate::stream::StreamHandle>),
    Failed(String),
}

/// Eventos del worker de login.
enum LoginEvent {
    /// Login OK: refresh_token + username que hay que persistir.
    Ok {
        refresh_token: String,
        username: String,
    },
    Failed(String),
}

/// Eventos del worker de subtítulos.
enum SubsEvent {
    /// Resultados de búsqueda listos.
    Found(Vec<Subtitle>),
    /// Descarga completada: ruta local del `.srt` + release name para
    /// mostrar en el status.
    Downloaded {
        path: PathBuf,
        release: String,
    },
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

/// Vista activa de la TUI.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    /// Menú de bienvenida: recomendaciones vs búsqueda directa.
    Menu,
    /// Login de Letterboxd (usuario/contraseña) — solo si aún no hay
    /// refresh_token guardado.
    Login,
    /// Lista de recomendaciones desde Letterboxd.
    Recs,
    /// Formulario para escribir un título y buscar torrents directamente.
    Search,
    /// Resultados de torrents (venido de Recs o de Search).
    Torrents,
    /// Lista de subtítulos de OpenSubtitles para el torrent seleccionado.
    Subs,
}

/// Campo enfocado en la vista de login.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LoginField {
    Username,
    Password,
}

/// De dónde venimos al entrar a la vista de torrents. Determina a qué vista
/// se vuelve con `b`/`Esc`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TorrentsSource {
    FromRecs,
    FromSearch,
}

struct App {
    username: String,
    count: usize,
    min_rating: f32,
    recs: Vec<Recommendation>,
    list_state: TableState,
    loading: bool,
    stale: bool,
    stage_msg: String,
    stage_total: u64,
    stage_pos: u64,
    error: Option<String>,

    // Estado de la vista de torrents
    view: View,
    tor_movie: String,
    tor_status: String,
    tor_loading: bool,
    tor_error: Option<String>,
    tor_results: Vec<Torrent>,
    tor_state: TableState,
    /// Idioma original de la película en curso (ISO 639-1). Se usa para
    /// clasificar el audio de cada release.
    tor_original_language: Option<String>,
    /// IMDb ID de la película en curso (`ttXXXXXXX`). Se usa para acotar
    /// la búsqueda de subtítulos en OpenSubtitles.
    tor_imdb_id: Option<String>,
    /// Desde qué vista se saltó a Torrents (para saber a dónde volver).
    tor_source: TorrentsSource,

    // Menú principal
    menu_state: TableState,

    // Búsqueda directa
    search_input: String,

    // Login de Letterboxd
    login_user: String,
    login_pass: String,
    login_focus: LoginField,
    login_busy: bool,
    login_error: Option<String>,

    // Subtítulos (OpenSubtitles)
    subs_results: Vec<Subtitle>,
    subs_state: TableState,
    subs_loading: bool,
    subs_error: Option<String>,
    /// Ruta al `.srt` ya descargado que se pasará a VLC como `--sub-file`
    /// al arrancar el próximo stream. Se resetea cuando cambia la
    /// película (nueva navegación a Torrents).
    sub_path: Option<PathBuf>,
    /// Release name del sub actualmente cargado (para mostrarlo en la
    /// barra de estado).
    sub_release: Option<String>,

    // Estado de streaming (vive mientras la TUI está abierta)
    stream: Option<crate::stream::StreamHandle>,
    stream_msg: Option<String>,
    /// Flag que se pone `false` cuando el proceso de VLC termina. Se poll-ea
    /// en el bucle principal para detectar cierre del reproductor y liberar
    /// el `StreamHandle` automáticamente.
    stream_player_alive: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Si true, el panel inferior muestra el magnet del torrent seleccionado.
    /// Si false (default), muestra el progreso del stream activo (o un
    /// placeholder si no hay stream).
    show_magnet: bool,

    /// Contador de frames del spinner, incrementado cada tick del loop
    /// principal (~100 ms).
    spinner_frame: usize,
}

impl App {
    fn new(username: String, count: usize, min_rating: f32) -> Self {
        let mut menu_state = TableState::default();
        menu_state.select(Some(0));
        Self {
            username,
            count,
            min_rating,
            recs: Vec::new(),
            list_state: TableState::default(),
            loading: false,
            stale: true,
            stage_msg: String::new(),
            stage_total: 0,
            stage_pos: 0,
            error: None,
            view: View::Menu,
            tor_movie: String::new(),
            tor_status: String::new(),
            tor_loading: false,
            tor_error: None,
            tor_results: Vec::new(),
            tor_state: TableState::default(),
            tor_original_language: None,
            tor_imdb_id: None,
            tor_source: TorrentsSource::FromRecs,
            menu_state,
            search_input: String::new(),
            login_user: String::new(),
            login_pass: String::new(),
            login_focus: LoginField::Username,
            login_busy: false,
            login_error: None,
            subs_results: Vec::new(),
            subs_state: TableState::default(),
            subs_loading: false,
            subs_error: None,
            sub_path: None,
            sub_release: None,
            stream: None,
            stream_msg: None,
            stream_player_alive: None,
            show_magnet: false,
            spinner_frame: 0,
        }
    }

    fn spinner(&self) -> &'static str {
        SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()]
    }

    fn select_next(&mut self) {
        let (state, len) = match self.view {
            View::Menu => (&mut self.menu_state, MENU_ITEMS.len()),
            View::Recs => (&mut self.list_state, self.recs.len()),
            View::Torrents => (&mut self.tor_state, self.tor_results.len()),
            View::Subs => (&mut self.subs_state, self.subs_results.len()),
            View::Search | View::Login => return,
        };
        if len == 0 {
            return;
        }
        let i = match state.selected() {
            Some(i) if i + 1 < len => i + 1,
            Some(_) => 0,
            None => 0,
        };
        state.select(Some(i));
    }

    fn select_prev(&mut self) {
        let (state, len) = match self.view {
            View::Menu => (&mut self.menu_state, MENU_ITEMS.len()),
            View::Recs => (&mut self.list_state, self.recs.len()),
            View::Torrents => (&mut self.tor_state, self.tor_results.len()),
            View::Subs => (&mut self.subs_state, self.subs_results.len()),
            View::Search | View::Login => return,
        };
        if len == 0 {
            return;
        }
        let i = match state.selected() {
            Some(0) | None => len - 1,
            Some(i) => i - 1,
        };
        state.select(Some(i));
    }
}

/// Opciones del menú principal (orden respetado).
const MENU_ITEMS: &[(&str, &str)] = &[
    (
        "Recomendaciones desde Letterboxd",
        "Genera y navega por películas recomendadas basadas en tu historial.",
    ),
    (
        "Buscar torrents directamente",
        "Escribe un título y busca torrents sin pasar por Letterboxd.",
    ),
    ("Salir", "Cerrar la aplicación."),
];

pub async fn run(
    config: Config,
    http: reqwest::Client,
    count: usize,
    min_rating: f32,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // EnableBracketedPaste hace que Cmd+V / Ctrl+Shift+V en el terminal
    // emita un `Event::Paste(String)` en vez de un chorro de Char events
    // — necesario para pegar contraseñas largas en el login.
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, config, http, count, min_rating).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
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
    let mut config = config;
    let mut app = App::new(config.username.clone(), count, min_rating);
    let mut rx: Option<mpsc::UnboundedReceiver<WorkerEvent>> = None;
    let mut tor_rx: Option<mpsc::UnboundedReceiver<TorrentEvent>> = None;
    let mut stream_rx: Option<mpsc::UnboundedReceiver<StreamEvent>> = None;
    let mut login_rx: Option<mpsc::UnboundedReceiver<LoginEvent>> = None;
    let mut subs_rx: Option<mpsc::UnboundedReceiver<SubsEvent>> = None;

    // Sender del canal de streaming, compartido: cada spawn_stream lo clona.
    // Lo mantenemos en un slot que se rellena en el primer uso.
    let mut stream_tx: Option<mpsc::UnboundedSender<StreamEvent>> = None;

    // Arrancamos en el menú principal — no dispara ninguna llamada de red
    // hasta que el usuario elija una opción.

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        // Anima el spinner cada tick (~100 ms del event::poll).
        app.spinner_frame = app.spinner_frame.wrapping_add(1);

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
                        // Si el user movió -/+/[/] durante la carga, los
                        // parámetros mostrados ya no coinciden con los
                        // usados para calcular `recs` → mantenemos el
                        // aviso "par\u00e1metros modificados" (stale=true).
                        if !app.stale {
                            app.stale = false;
                        }
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

        if let Some(r) = tor_rx.as_mut() {
            while let Ok(evt) = r.try_recv() {
                match evt {
                    TorrentEvent::Status(msg) => app.tor_status = msg,
                    TorrentEvent::Language(lang) => app.tor_original_language = lang,
                    TorrentEvent::Imdb(id) => app.tor_imdb_id = id,
                    TorrentEvent::Done(list) => {
                        app.tor_loading = false;
                        app.tor_error = None;
                        app.tor_state
                            .select(if list.is_empty() { None } else { Some(0) });
                        app.tor_results = list;
                        app.tor_status = format!("{} resultado(s)", app.tor_results.len());
                    }
                    TorrentEvent::Failed(e) => {
                        app.tor_loading = false;
                        app.tor_error = Some(e);
                    }
                }
            }
        }

        if let Some(r) = stream_rx.as_mut() {
            while let Ok(evt) = r.try_recv() {
                match evt {
                    StreamEvent::Starting(msg) => {
                        app.stream_msg = Some(msg);
                    }
                    StreamEvent::Ready(handle) => {
                        // Auto-abre VLC apuntando a la URL del stream. El
                        // El PlayerHandle contiene el flag `alive` que
                        // se pone a false cuando VLC termina — así
                        // detectamos el cierre del reproductor y
                        // liberamos el stream automáticamente. En TUI
                        // solo nos interesa el flag; el `kill_token` no
                        // se usa (no hay botón "Detener").
                        let player =
                            crate::stream::open_in_vlc(&handle.url, app.sub_path.as_deref(), None);
                        let sub_note = app
                            .sub_release
                            .as_deref()
                            .map(|r| format!(" · sub: {r}"))
                            .unwrap_or_default();
                        app.stream_msg = Some(format!(
                            "▶ Streaming {} — {}{sub_note}",
                            handle.file_name, handle.url
                        ));
                        // Mantén el handle vivo mientras la TUI y VLC estén
                        // abiertos (Drop apaga el server + limpia el
                        // tempdir).
                        app.stream = Some(*handle);
                        app.stream_player_alive = Some(player.alive);
                    }
                    StreamEvent::Failed(e) => {
                        app.stream_msg = Some(format!("❌ Stream: {e}"));
                    }
                }
            }
        }

        if let Some(r) = login_rx.as_mut() {
            while let Ok(evt) = r.try_recv() {
                match evt {
                    LoginEvent::Ok {
                        refresh_token,
                        username,
                    } => {
                        // Persiste en disco para futuras ejecuciones.
                        let creds = crate::credentials::Credentials {
                            refresh_token: Some(refresh_token.clone()),
                            username: Some(username.clone()),
                        };
                        if let Err(e) = crate::credentials::save(&creds) {
                            app.login_error = Some(format!("Guardado local falló: {e}"));
                            app.login_busy = false;
                            continue;
                        }
                        // Actualiza config en memoria y salta a recs.
                        config.refresh_token = Some(refresh_token);
                        config.username = username.clone();
                        app.username = username;
                        app.login_busy = false;
                        app.login_error = None;
                        app.login_pass.clear();
                        app.view = View::Recs;
                        if app.recs.is_empty() && !app.loading {
                            spawn_fetch(&config, &http, &mut app, &mut rx);
                        }
                    }
                    LoginEvent::Failed(e) => {
                        app.login_busy = false;
                        app.login_error = Some(e);
                    }
                }
            }
        }

        if let Some(r) = subs_rx.as_mut() {
            while let Ok(evt) = r.try_recv() {
                match evt {
                    SubsEvent::Found(list) => {
                        app.subs_loading = false;
                        app.subs_results = list;
                        app.subs_error = None;
                        if !app.subs_results.is_empty() {
                            app.subs_state.select(Some(0));
                        }
                    }
                    SubsEvent::Downloaded { path, release } => {
                        app.subs_loading = false;
                        app.sub_path = Some(path);
                        app.sub_release = Some(release.clone());
                        app.stream_msg = Some(format!(
                            "🎬 Sub cargado ({release}). Pulsa 's' para stream con subs."
                        ));
                        // Vuelve automáticamente a la lista de torrents
                        // — el user ya ha elegido, no hay más que hacer
                        // en la vista de subs.
                        app.view = View::Torrents;
                    }
                    SubsEvent::Failed(e) => {
                        app.subs_loading = false;
                        app.subs_error = Some(e);
                    }
                }
            }
        }

        // Detección de VLC cerrado: si el proceso del reproductor terminó,
        // liberamos el StreamHandle (su Drop apaga axum + libr qbit + borra
        // el tempdir) y actualizamos el mensaje para que el user sepa que
        // ya puede elegir otra opción.
        if let Some(alive) = &app.stream_player_alive {
            if !alive.load(std::sync::atomic::Ordering::Relaxed) {
                app.stream = None;
                app.stream_player_alive = None;
                app.stream_msg = Some("Reproductor cerrado — stream detenido.".to_string());
            }
        }

        if event::poll(Duration::from_millis(100))? {
            let evt = event::read()?;
            // Pegar (Cmd+V / Ctrl+Shift+V) llega como Event::Paste gracias a
            // EnableBracketedPaste. Lo aceptamos solo en vistas con input
            // de texto.
            if let Event::Paste(s) = &evt {
                match app.view {
                    View::Login => {
                        if !app.login_busy {
                            let target = match app.login_focus {
                                LoginField::Username => &mut app.login_user,
                                LoginField::Password => &mut app.login_pass,
                            };
                            target.push_str(s.trim_end_matches(['\n', '\r']));
                        }
                    }
                    View::Search => {
                        app.search_input.push_str(s.trim_end_matches(['\n', '\r']));
                    }
                    _ => {}
                }
                continue;
            }
            let Event::Key(key) = evt else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // 'q' cierra la app desde cualquier vista *excepto* Search
            // y Login (donde 'q' es una tecla válida para escribir en
            // el input).
            if matches!(key.code, KeyCode::Char('q'))
                && app.view != View::Search
                && app.view != View::Login
            {
                break;
            }

            match app.view {
                View::Menu => match key.code {
                    KeyCode::Esc => break,
                    KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
                    KeyCode::Enter => match app.menu_state.selected().unwrap_or(0) {
                        0 => {
                            // Recomendaciones — si aún no hay
                            // refresh_token, primero login.
                            if config.refresh_token.is_none() {
                                app.view = View::Login;
                                app.login_focus = LoginField::Username;
                                app.login_error = None;
                                app.login_busy = false;
                            } else {
                                app.view = View::Recs;
                                if app.recs.is_empty() && !app.loading {
                                    spawn_fetch(&config, &http, &mut app, &mut rx);
                                }
                            }
                        }
                        1 => {
                            // Búsqueda directa (no requiere login de LB).
                            app.view = View::Search;
                            app.search_input.clear();
                        }
                        _ => break,
                    },
                    _ => {}
                },
                View::Login => match key.code {
                    KeyCode::Esc => {
                        if !app.login_busy {
                            app.view = View::Menu;
                            app.login_error = None;
                        }
                    }
                    KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
                        app.login_focus = match app.login_focus {
                            LoginField::Username => LoginField::Password,
                            LoginField::Password => LoginField::Username,
                        };
                    }
                    KeyCode::Enter => {
                        if app.login_busy {
                            // ignorar
                        } else if app.login_focus == LoginField::Username
                            && app.login_pass.is_empty()
                        {
                            // Enter en username salta a password.
                            app.login_focus = LoginField::Password;
                        } else if !app.login_user.trim().is_empty() && !app.login_pass.is_empty() {
                            // Submit
                            if login_rx.is_none() {
                                let (tx, rx_new) = mpsc::unbounded_channel();
                                login_rx = Some(rx_new);
                                spawn_login(&http, &config, &mut app, tx);
                            } else {
                                // Ya hay canal — creamos uno nuevo (por
                                // si el user reintenta tras error).
                                let (tx, rx_new) = mpsc::unbounded_channel();
                                login_rx = Some(rx_new);
                                spawn_login(&http, &config, &mut app, tx);
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        if !app.login_busy {
                            match app.login_focus {
                                LoginField::Username => {
                                    app.login_user.pop();
                                }
                                LoginField::Password => {
                                    app.login_pass.pop();
                                }
                            }
                        }
                    }
                    KeyCode::Char(c) if !app.login_busy => match app.login_focus {
                        LoginField::Username => app.login_user.push(c),
                        LoginField::Password => app.login_pass.push(c),
                    },
                    _ => {}
                },
                View::Search => match key.code {
                    KeyCode::Esc => {
                        app.view = View::Menu;
                    }
                    KeyCode::Enter => {
                        let q = app.search_input.trim().to_string();
                        if !q.is_empty() {
                            spawn_direct_search(&http, &config, &q, &mut app, &mut tor_rx);
                        }
                    }
                    KeyCode::Backspace => {
                        app.search_input.pop();
                    }
                    KeyCode::Char(c) => {
                        // 'q' se acepta como cualquier otro carácter: el
                        // handler global de 'q' que cierra la app está
                        // gateado a `!= View::Search && != View::Login`,
                        // así que aquí sí llega. En Search se sale con
                        // Esc.
                        app.search_input.push(c);
                    }
                    _ => {}
                },
                View::Recs => match key.code {
                    KeyCode::Esc | KeyCode::Char('b') => {
                        app.view = View::Menu;
                    }
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
                    KeyCode::Char('t') if !app.tor_loading => {
                        // Guard: si ya estamos cargando torrents, ignoramos
                        // el 't' para no arrancar dos búsquedas y machacar
                        // el canal.
                        spawn_torrents(&config, &http, &mut app, &mut tor_rx);
                    }
                    _ => {}
                },
                View::Torrents => match key.code {
                    KeyCode::Esc | KeyCode::Char('b') => {
                        app.view = match app.tor_source {
                            TorrentsSource::FromRecs => View::Recs,
                            TorrentsSource::FromSearch => View::Search,
                        };
                    }
                    KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
                    KeyCode::Enter => {
                        if let Some(i) = app.tor_state.selected() {
                            if let Some(t) = app.tor_results.get(i) {
                                // macOS: `open` con un magnet lo pasa al
                                // handler registrado (Transmission,
                                // qBittorrent, etc.). En Linux se usa
                                // xdg-open; en Windows `start`.
                                open_magnet(&t.magnet);
                                app.tor_status = format!("Magnet abierto: {}", t.title);
                            }
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(i) = app.tor_state.selected() {
                            if let Some(t) = app.tor_results.get(i).cloned() {
                                // Guard: si ya arrancamos un stream y aún
                                // no llegó el `Ready`, ignoramos la
                                // pulsación. Sin esto dos 's' seguidas
                                // spawn dos librqbit y dos VLC; el segundo
                                // `Ready` pisa el handle del primero y el
                                // player anterior se queda huérfano.
                                let starting = app
                                    .stream_msg
                                    .as_deref()
                                    .map(|m| m.starts_with("Iniciando stream:"))
                                    .unwrap_or(false);
                                if starting {
                                    continue;
                                }
                                // Si había un stream anterior (por
                                // ejemplo, VLC ya cerrado pero handle
                                // aún en memoria), lo tiramos ahora
                                // para que su Drop libere puertos y
                                // borre el tempdir ANTES de arrancar
                                // la nueva sesión librqbit.
                                app.stream = None;
                                app.stream_player_alive = None;
                                // Inicializa el canal de stream la primera
                                // vez que se usa.
                                if stream_tx.is_none() {
                                    let (tx, rx) = mpsc::unbounded_channel();
                                    stream_tx = Some(tx);
                                    stream_rx = Some(rx);
                                }
                                let tx = stream_tx.as_ref().unwrap().clone();
                                app.stream_msg = Some(format!("Iniciando stream: {}…", t.title));
                                tokio::spawn(async move {
                                    let _ = tx.send(StreamEvent::Starting(
                                        "Resolviendo metadata del torrent (magnet)…".to_string(),
                                    ));
                                    match crate::stream::start(t.magnet.clone()).await {
                                        Ok(h) => {
                                            let _ = tx.send(StreamEvent::Ready(Box::new(h)));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(StreamEvent::Failed(e.to_string()));
                                        }
                                    }
                                });
                            }
                        }
                    }
                    KeyCode::Char('m') => {
                        app.show_magnet = !app.show_magnet;
                    }
                    KeyCode::Char('x') => {
                        // Búsqueda de subtítulos en OpenSubtitles para
                        // el torrent seleccionado. Como `query` mandamos
                        // el título completo del release
                        // (`Foo.2007.1080p.BluRay.x264`) para que
                        // OpenSubtitles rankee por match de edición.
                        if !subtitles::is_available() {
                            app.stream_msg = Some(
                                "Subtítulos deshabilitados: sin OPENSUBTITLES_API_KEY".to_string(),
                            );
                        } else if let Some(i) = app.tor_state.selected() {
                            if let Some(t) = app.tor_results.get(i).cloned() {
                                // Nuevo canal por búsqueda para
                                // descartar resultados de búsquedas
                                // anteriores (evita race si el user
                                // pulsa 'x' varias veces seguidas).
                                let (tx, new_rx) = mpsc::unbounded_channel();
                                subs_rx = Some(new_rx);
                                app.view = View::Subs;
                                app.subs_loading = true;
                                app.subs_error = None;
                                app.subs_results.clear();
                                app.subs_state.select(None);
                                let http_c = http.clone();
                                let imdb = app.tor_imdb_id.clone();
                                let query = t.title.clone();
                                tokio::spawn(async move {
                                    match subtitles::search(
                                        &http_c,
                                        None,
                                        imdb.as_deref(),
                                        Some(&query),
                                        None,
                                        None,
                                        subtitles::DEFAULT_LANGUAGES,
                                    )
                                    .await
                                    {
                                        Ok(list) => {
                                            let _ = tx.send(SubsEvent::Found(list));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(SubsEvent::Failed(e.to_string()));
                                        }
                                    }
                                });
                            }
                        }
                    }
                    _ => {}
                },
                View::Subs => match key.code {
                    KeyCode::Esc | KeyCode::Char('b') => {
                        app.view = View::Torrents;
                    }
                    KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
                    KeyCode::Enter => {
                        if let Some(i) = app.subs_state.selected() {
                            if let Some(sub) = app.subs_results.get(i).cloned() {
                                let (tx, new_rx) = mpsc::unbounded_channel();
                                subs_rx = Some(new_rx);
                                app.subs_loading = true;
                                app.subs_error = None;
                                let http_c = http.clone();
                                // Directorio efímero por sesión. Se
                                // limpia al salir el proceso.
                                let dest = std::env::temp_dir().join("videodrome-subs");
                                tokio::spawn(async move {
                                    match subtitles::download(&http_c, &sub, &dest).await {
                                        Ok(path) => {
                                            let _ = tx.send(SubsEvent::Downloaded {
                                                path,
                                                release: sub.release.clone(),
                                            });
                                        }
                                        Err(e) => {
                                            let _ = tx.send(SubsEvent::Failed(e.to_string()));
                                        }
                                    }
                                });
                            }
                        }
                    }
                    _ => {}
                },
            }
        }
    }

    Ok(())
}

fn open_magnet(magnet: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(magnet).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(magnet).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", magnet])
        .spawn();
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

/// Lanza la búsqueda de torrents para la recomendación seleccionada. Cambia
/// la vista a Torrents inmediatamente y va poblándola cuando llegan
/// resultados. Requiere que haya una selección válida y no estar ya cargando.
fn spawn_torrents(
    config: &Config,
    http: &reqwest::Client,
    app: &mut App,
    rx: &mut Option<mpsc::UnboundedReceiver<TorrentEvent>>,
) {
    let Some(i) = app.list_state.selected() else {
        return;
    };
    let Some(rec) = app.recs.get(i).cloned() else {
        return;
    };

    let (tx, new_rx) = mpsc::unbounded_channel();
    *rx = Some(new_rx);

    app.view = View::Torrents;
    app.tor_source = TorrentsSource::FromRecs;
    // Incluye el año en la cabecera para desambiguar remakes (Funny Games
    // 1997 vs 2007, etc.).
    app.tor_movie = match rec.movie.year() {
        Some(y) => format!("{} ({y})", rec.movie.title),
        None => rec.movie.title.clone(),
    };
    app.tor_status = "Buscando IMDb ID en TMDB…".to_string();
    app.tor_loading = true;
    app.tor_error = None;
    app.tor_results.clear();
    app.tor_state.select(None);
    // Reset del IMDb id + del sub cacheado: nueva película, nueva
    // búsqueda de subtítulos si el user pulsa 'x'.
    app.tor_imdb_id = None;
    app.sub_path = None;
    app.sub_release = None;

    let http = http.clone();
    let bearer = config.tmdb_bearer_token.clone();
    let tmdb_id = rec.movie.id;
    let fallback_title = rec.movie.title.clone();
    let fallback_year = rec.movie.year();

    tokio::spawn(async move {
        let result = async {
            let tmdb = TmdbClient::new(&http, &bearer);
            // Detalles enriquecidos: IMDb ID + título original (inglés) +
            // título ruso + idioma original + año. El título original es
            // clave para acertar en Knaben/YTS: TMDB devuelve el traducido
            // ("La milla verde") pero las releases usan el original ("The
            // Green Mile"). El ruso se usa como fallback cuando el original
            // no encuentra nada.
            let details = tmdb.get_movie_details(tmdb_id).await.ok().flatten();
            let (title, russian_title, year, imdb_id, original_language) = match details {
                Some(d) => (
                    d.original_title
                        .or(d.fallback_title)
                        .unwrap_or(fallback_title),
                    d.russian_title,
                    d.year.or(fallback_year),
                    d.imdb_id,
                    d.original_language,
                ),
                None => (fallback_title, None, fallback_year, None, None),
            };

            // Emitimos el idioma antes del Done para que el renderer pueda
            // clasificar el audio de cada release.
            let _ = tx.send(TorrentEvent::Language(original_language.clone()));
            // Y el IMDb ID resuelto, para que la vista de subtítulos
            // pueda pedir solo subs de esta película concreta.
            let _ = tx.send(TorrentEvent::Imdb(imdb_id.clone()));

            let _ = tx.send(TorrentEvent::Status(match &imdb_id {
                Some(id) => format!("Buscando \"{title}\" (imdb {id})…"),
                None => format!("Buscando \"{title}\"…"),
            }));

            let providers = torrents::default_providers();

            let primary_query = MovieQuery {
                title: title.clone(),
                year,
                imdb_id: imdb_id.clone(),
                tmdb_id: Some(tmdb_id),
                original_language: original_language.clone(),
                title_variants: Vec::new(),
                kind: crate::tmdb::MediaKind::Movie,
                season: None,
                episode: None,
            };
            // min_seeders=1 en la TUI: los reportes de seeders de los
            // indexers son aproximados y filtrar por 3 pierde demasiadas
            // pelis de nicho.
            let mut list = torrents::search_all(&http, &providers, &primary_query, 3, 40)
                .await
                .results;

            // Fallback: si no hay resultados con el título original y
            // tenemos título ruso, reintentar. Los indexers rusos (RuTracker,
            // rutor…) indexan con cirílico.
            if list.is_empty() {
                if let Some(ru) = russian_title.filter(|s| s != &title) {
                    let _ = tx.send(TorrentEvent::Status(format!(
                        "Sin resultados. Reintentando en ruso: \"{ru}\"…"
                    )));
                    let ru_query = MovieQuery {
                        title: ru.clone(),
                        year,
                        imdb_id,
                        tmdb_id: Some(tmdb_id),
                        original_language: original_language.clone(),
                        title_variants: Vec::new(),
                        kind: crate::tmdb::MediaKind::Movie,
                        season: None,
                        episode: None,
                    };
                    let raw = torrents::search_all(&http, &providers, &ru_query, 3, 40)
                        .await
                        .results;
                    // Filtro estricto adicional para el fallback ruso: los
                    // scene rusos siguen el patrón `<Nombre ruso> / <Nombre
                    // original> [año, ...]`, así que el release TIENE que
                    // empezar con el título ruso. Sin esto, Knaben cuela
                    // pelis distintas que solo *mencionan* el título en su
                    // descripción (visto con "Забавные игры" apareciendo
                    // como referencia dentro del release de "Гости / The
                    // Visitors").
                    list = raw
                        .into_iter()
                        .filter(|t| release_starts_with(&t.title, &ru))
                        .collect();
                }
            }

            anyhow::Ok(list)
        }
        .await;

        match result {
            Ok(list) => {
                let _ = tx.send(TorrentEvent::Done(list));
            }
            Err(e) => {
                let _ = tx.send(TorrentEvent::Failed(e.to_string()));
            }
        }
    });
}

/// Búsqueda de torrents directa: no pasa por Letterboxd/TMDB, usa el título
/// tal cual lo teclea el usuario. Si el título acaba en un año (4 dígitos),
/// lo extrae para pasar `year` a los providers (mejora precisión en remakes).
fn spawn_direct_search(
    http: &reqwest::Client,
    _config: &Config,
    query: &str,
    app: &mut App,
    rx: &mut Option<mpsc::UnboundedReceiver<TorrentEvent>>,
) {
    let (title, year) = split_trailing_year(query);

    let (tx, new_rx) = mpsc::unbounded_channel();
    *rx = Some(new_rx);

    app.view = View::Torrents;
    app.tor_source = TorrentsSource::FromSearch;
    app.tor_movie = query.to_string();
    app.tor_status = "Buscando…".to_string();
    app.tor_loading = true;
    app.tor_error = None;
    app.tor_results.clear();
    app.tor_state.select(None);
    // Sin TMDB no podemos saber el idioma original — el badge de audio
    // quedará como Unknown salvo que el título del release lo declare.
    app.tor_original_language = None;
    // Search directa: no tenemos IMDb ID a priori (no pasamos por TMDB).
    // Los subs se buscarán solo por `query`.
    app.tor_imdb_id = None;
    app.sub_path = None;
    app.sub_release = None;

    let http = http.clone();

    tokio::spawn(async move {
        let query = MovieQuery {
            title: title.clone(),
            year,
            imdb_id: None,
            tmdb_id: None,
            original_language: None,
            title_variants: Vec::new(),
            kind: crate::tmdb::MediaKind::Movie,
            season: None,
            episode: None,
        };
        let _ = tx.send(TorrentEvent::Status(format!("Buscando \"{title}\"…")));

        let providers = torrents::default_providers();
        let list = torrents::search_all(&http, &providers, &query, 3, 40)
            .await
            .results;
        let _ = tx.send(TorrentEvent::Done(list));
    });
}

// `split_trailing_year` y `release_starts_with` viven en
// `torrents::` y se importan arriba — la definición local se retiró
// para no divergir con la GUI (ver informe de revisión).

/// Lanza en tokio el intento de login con usuario/contraseña. Los datos se
/// leen del `App` (los inputs de la vista Login) y el resultado se emite
/// por el canal `tx`.
fn spawn_login(
    http: &reqwest::Client,
    config: &Config,
    app: &mut App,
    tx: mpsc::UnboundedSender<LoginEvent>,
) {
    app.login_busy = true;
    app.login_error = None;

    let http = http.clone();
    let client_id = config.client_id.clone();
    let client_secret = config.client_secret.clone();
    let user = app.login_user.trim().to_string();
    let pass = app.login_pass.clone();

    tokio::spawn(async move {
        match crate::auth::login_with_password(&http, &client_id, &client_secret, &user, &pass)
            .await
        {
            Ok(result) => {
                let _ = tx.send(LoginEvent::Ok {
                    refresh_token: result.refresh_token,
                    username: user,
                });
            }
            Err(e) => {
                let _ = tx.send(LoginEvent::Failed(e.to_string()));
            }
        }
    });
}

// (release_starts_with movido a torrents::)

fn draw(f: &mut Frame, app: &mut App) {
    match app.view {
        View::Menu => draw_menu(f, app),
        View::Login => draw_login(f, app),
        View::Search => draw_search(f, app),
        View::Recs => draw_recs(f, app),
        View::Torrents => draw_torrents(f, app),
        View::Subs => draw_subs(f, app),
    }
}

fn draw_login(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // título
            Constraint::Length(3), // usuario
            Constraint::Length(3), // contraseña
            Constraint::Length(3), // status/error
            Constraint::Min(1),    // spacer
            Constraint::Length(4), // footer
        ])
        .split(f.area());

    let title = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "🔐  Login en Letterboxd",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(Span::styled(
            "Necesario una sola vez — el refresh_token se guardará en ~/.config/videodrome/credentials.json",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    let user_focused = app.login_focus == LoginField::Username;
    let pass_focused = app.login_focus == LoginField::Password;

    // Cursor sólo en el campo enfocado.
    let user_text = if user_focused {
        format!("{}▏", app.login_user)
    } else {
        app.login_user.clone()
    };
    let user_style = if user_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let user_border = if user_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let user_panel = Paragraph::new(user_text).style(user_style).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(user_border)
            .title(" Usuario "),
    );
    f.render_widget(user_panel, chunks[1]);

    let pass_masked = "*".repeat(app.login_pass.chars().count());
    let pass_text = if pass_focused {
        format!("{pass_masked}▏")
    } else {
        pass_masked
    };
    let pass_style = if pass_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let pass_border = if pass_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let pass_panel = Paragraph::new(pass_text).style(pass_style).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(pass_border)
            .title(" Contraseña "),
    );
    f.render_widget(pass_panel, chunks[2]);

    let status = if app.login_busy {
        Line::from(Span::styled(
            format!("{}  Autenticando…", app.spinner()),
            Style::default().fg(Color::Cyan),
        ))
    } else if let Some(err) = &app.login_error {
        Line::from(Span::styled(
            format!("❌  {err}"),
            Style::default().fg(Color::Red),
        ))
    } else {
        Line::from(Span::styled(
            "Introduce tu usuario y contraseña de Letterboxd.",
            Style::default().fg(Color::DarkGray),
        ))
    };
    let status_panel = Paragraph::new(status)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Estado "));
    f.render_widget(status_panel, chunks[3]);

    let footer = Paragraph::new("Tab/↓↑ cambiar campo · Enter enviar · Esc volver al menú")
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[5]);
}

fn draw_menu(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // banner
            Constraint::Min(3),    // menu
            Constraint::Length(4), // footer
        ])
        .split(f.area());

    let banner = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "🎬 videodrome",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(Span::styled(
            format!("   {}", app.username),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
    ])
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(banner, chunks[0]);

    let rows: Vec<Row> = MENU_ITEMS
        .iter()
        .map(|(label, desc)| {
            Row::new(vec![
                Cell::from((*label).to_string())
                    .style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from((*desc).to_string()).style(Style::default().fg(Color::DarkGray)),
            ])
            .height(2)
        })
        .collect();

    let widths = [Constraint::Length(36), Constraint::Fill(1)];
    let table = Table::new(rows, widths)
        .block(Block::default().borders(Borders::ALL).title(" Menú "))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(table, chunks[1], &mut app.menu_state);

    let footer = Paragraph::new(HELP_MENU)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}

fn draw_search(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Length(3), // input
            Constraint::Min(1),    // spacer
            Constraint::Length(4), // footer
        ])
        .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "🔎 Búsqueda directa",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "   Escribe el título (y opcionalmente el año al final: `Dune 2021`).",
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Cursor `▏` para indicar el punto de escritura.
    let input_line = format!("{}▏", app.search_input);
    let input = Paragraph::new(input_line)
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).title(" Título "));
    f.render_widget(input, chunks[1]);

    let footer = Paragraph::new(HELP_SEARCH)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[3]);
}

fn draw_recs(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "🎬 videodrome",
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

    if app.recs.is_empty() && !app.loading {
        let placeholder = Paragraph::new("  (sin recomendaciones todavía — pulsa 'r' para cargar)")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Recomendaciones "),
            );
        f.render_widget(placeholder, chunks[1]);
    } else {
        let rows: Vec<Row> = app
            .recs
            .iter()
            .enumerate()
            .map(|(i, rec)| {
                let rating = match rec.lb_rating {
                    Some(r) => format!("★ {r:.2}"),
                    None => format!("★ {:.2} (TMDB)", rec.movie.vote_average / 2.0),
                };
                let year = rec
                    .movie
                    .year()
                    .map(|y| y.to_string())
                    .unwrap_or_else(|| "—".to_string());
                Row::new(vec![
                    Cell::from(format!("{:>2}.", i + 1))
                        .style(Style::default().fg(Color::DarkGray)),
                    Cell::from(rec.movie.title.clone()).style(Style::default().fg(Color::White)),
                    Cell::from(year).style(Style::default().fg(Color::DarkGray)),
                    Cell::from(rating).style(Style::default().fg(Color::Yellow)),
                ])
            })
            .collect();

        // Columnas responsive: nº + año + rating fijos, título ocupa el
        // resto. El año es clave: muchas pelis tienen remakes/versiones
        // con el mismo título (Funny Games 1997 vs 2007, Dune 1984 vs
        // 2021, etc.) y sin año no sabes cuál te está recomendando TMDB.
        let widths = [
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(6),
            Constraint::Length(16),
        ];

        let table = Table::new(rows, widths)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Recomendaciones "),
            )
            .row_highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        f.render_stateful_widget(table, chunks[1], &mut app.list_state);
    }

    let status = if let Some(err) = &app.error {
        Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        ))
    } else if app.loading {
        let progress = if app.stage_total > 0 {
            format!(
                "{} {} ({}/{})",
                app.spinner(),
                app.stage_msg,
                app.stage_pos,
                app.stage_total
            )
        } else {
            format!("{} {}", app.spinner(), app.stage_msg)
        };
        Line::from(Span::styled(progress, Style::default().fg(Color::Cyan)))
    } else if app.stale {
        Line::from(Span::styled(
            "Parámetros modificados — pulsa 'r' para recargar",
            Style::default().fg(Color::Yellow),
        ))
    } else {
        Line::from(Span::raw(format!(
            "{HELP_RECS}  ({} resultados)",
            app.recs.len()
        )))
    };

    let footer = Paragraph::new(status)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}

fn draw_torrents(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(5),
            Constraint::Length(4),
        ])
        .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "🧲 Torrents",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  —  "),
        Span::styled(
            &app.tor_movie,
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    if app.tor_results.is_empty() {
        let placeholder_text = if app.tor_loading {
            format!("\n   {}  Buscando torrents…", app.spinner())
        } else if app.tor_error.is_some() {
            "\n   (error — ver barra inferior)".to_string()
        } else {
            "\n   (sin resultados)".to_string()
        };
        let placeholder = Paragraph::new(placeholder_text)
            .block(Block::default().borders(Borders::ALL).title(" Resultados "));
        f.render_widget(placeholder, chunks[1]);
    } else {
        let rows: Vec<Row> = app
            .tor_results
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let size = torrents::format_size(t.size_bytes);
                let q = t.quality.as_deref().unwrap_or("?");
                let audio =
                    torrents::classify_audio(&t.title, app.tor_original_language.as_deref());
                let audio_color = match audio {
                    torrents::AudioHint::Original => Color::LightGreen,
                    torrents::AudioHint::Multi => Color::LightBlue,
                    torrents::AudioHint::Dubbed(_) => Color::LightMagenta,
                    torrents::AudioHint::Unknown => Color::DarkGray,
                };
                // §7 audit series: prefijo visible cuando el release
                // es de series. Movie queda sin prefijo (compat).
                let mk_prefix = match t.match_kind {
                    torrents::MatchKind::Movie => "",
                    torrents::MatchKind::Episode => "[EP] ",
                    torrents::MatchKind::SeasonPack => "[PACK] ",
                    torrents::MatchKind::SeriesPack => "[SERIE] ",
                };
                let title_cell = format!("{mk_prefix}{}", t.title);
                Row::new(vec![
                    Cell::from(format!("{:>2}.", i + 1))
                        .style(Style::default().fg(Color::DarkGray)),
                    Cell::from(title_cell).style(Style::default().fg(Color::White)),
                    Cell::from(size).style(Style::default().fg(Color::Yellow)),
                    Cell::from(format!("↑{}", t.seeders)).style(Style::default().fg(Color::Green)),
                    Cell::from(format!("↓{}", t.leechers)).style(Style::default().fg(Color::Red)),
                    Cell::from(q.to_string()).style(Style::default().fg(Color::Cyan)),
                    Cell::from(audio.badge().to_string()).style(Style::default().fg(audio_color)),
                    Cell::from(t.source.clone()).style(Style::default().fg(Color::DarkGray)),
                ])
            })
            .collect();

        // Columnas responsive: número + métricas + badges anchos fijos, el
        // título absorbe todo el espacio libre.
        let widths = [
            Constraint::Length(4),  // nº
            Constraint::Fill(1),    // título
            Constraint::Length(10), // tamaño
            Constraint::Length(6),  // seeders
            Constraint::Length(6),  // leechers
            Constraint::Length(6),  // calidad
            Constraint::Length(7),  // audio
            Constraint::Length(8),  // provider
        ];

        let table = Table::new(rows, widths)
            .block(Block::default().borders(Borders::ALL).title(" Resultados "))
            .row_highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ")
            .column_spacing(1);
        f.render_stateful_widget(table, chunks[1], &mut app.tor_state);
    }

    // Panel dual: por defecto muestra el progreso del stream activo (o un
    // placeholder si no hay stream); con `m` alterna al magnet del torrent
    // seleccionado.
    let (panel_title, panel_text, panel_style) = if app.show_magnet {
        let text = app
            .tor_state
            .selected()
            .and_then(|i| app.tor_results.get(i))
            .map(|t| t.magnet.clone())
            .unwrap_or_else(|| "(selecciona un torrent para ver su magnet)".to_string());
        (" Magnet ", text, Style::default().fg(Color::DarkGray))
    } else if let Some(stream) = app.stream.as_ref() {
        let s = stream.stats();
        let pct = if s.total_bytes > 0 {
            (s.progress_bytes as f64 / s.total_bytes as f64) * 100.0
        } else {
            0.0
        };
        let text = format!(
            "▶ {}\n\n{:.1} %   {} / {}   ↓ {:.2} MiB/s   {} peers\n\n{}",
            stream.file_name,
            pct,
            torrents::format_size(s.progress_bytes),
            torrents::format_size(s.total_bytes),
            s.down_mbps,
            s.live_peers,
            stream.url,
        );
        (" Progreso stream ", text, Style::default().fg(Color::Green))
    } else {
        (
            " Progreso stream ",
            "(pulsa 's' sobre un torrent para empezar a streamear · 'm' para ver magnet)"
                .to_string(),
            Style::default().fg(Color::DarkGray),
        )
    };
    let panel = Paragraph::new(panel_text)
        .wrap(Wrap { trim: true })
        .style(panel_style)
        .block(Block::default().borders(Borders::ALL).title(panel_title));
    f.render_widget(panel, chunks[2]);

    let status = if let Some(err) = &app.tor_error {
        Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        ))
    } else if app.tor_loading {
        Line::from(Span::styled(
            format!("{} {}", app.spinner(), app.tor_status),
            Style::default().fg(Color::Cyan),
        ))
    } else if let Some(msg) = &app.stream_msg {
        Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Magenta),
        ))
    } else {
        Line::from(Span::raw(format!("{HELP_TORRENTS}  ·  {}", app.tor_status)))
    };
    let footer = Paragraph::new(status)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[3]);
}

fn draw_subs(f: &mut Frame, app: &mut App) {
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
            "💬 Subtítulos",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  —  OpenSubtitles"),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    if app.subs_results.is_empty() {
        let text = if app.subs_loading {
            format!("\n   {}  Buscando subtítulos…", app.spinner())
        } else if let Some(err) = &app.subs_error {
            format!("\n   Error: {err}")
        } else {
            "\n   (sin resultados)".to_string()
        };
        let placeholder = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" Resultados "));
        f.render_widget(placeholder, chunks[1]);
    } else {
        let rows: Vec<Row> = app
            .subs_results
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let lang = s.language.to_uppercase();
                let sdh = if s.hearing_impaired { "SDH" } else { "" };
                Row::new(vec![
                    Cell::from(format!("{:>2}.", i + 1))
                        .style(Style::default().fg(Color::DarkGray)),
                    Cell::from(lang).style(Style::default().fg(Color::LightCyan)),
                    Cell::from(s.release.clone()).style(Style::default().fg(Color::White)),
                    Cell::from(format!("↓ {}", s.downloads))
                        .style(Style::default().fg(Color::Green)),
                    Cell::from(if s.rating > 0.0 {
                        format!("★ {:.1}", s.rating)
                    } else {
                        String::new()
                    })
                    .style(Style::default().fg(Color::Yellow)),
                    Cell::from(sdh).style(Style::default().fg(Color::LightMagenta)),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(4),  // nº
            Constraint::Length(4),  // lang
            Constraint::Fill(1),    // release
            Constraint::Length(10), // downloads
            Constraint::Length(6),  // rating
            Constraint::Length(4),  // SDH
        ];

        let table = Table::new(rows, widths)
            .block(Block::default().borders(Borders::ALL).title(" Resultados "))
            .row_highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ")
            .column_spacing(1);
        f.render_stateful_widget(table, chunks[1], &mut app.subs_state);
    }

    let footer_line = if app.subs_loading {
        Line::from(Span::styled(
            format!("{} Trabajando…", app.spinner()),
            Style::default().fg(Color::Cyan),
        ))
    } else if let Some(err) = &app.subs_error {
        Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        ))
    } else {
        Line::from(Span::raw(HELP_SUBS))
    };
    let footer = Paragraph::new(footer_line)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
