//! Logging estructurado con `tracing` — stderr + fichero con rotación diaria.
//!
//! Motivación (audit "Logging a fichero e instrumentación"):
//!
//!   * En Windows la app corre con `windows_subsystem = "windows"` en
//!     modo GUI. Los `eprintln!` van a un handle de stderr inválido y
//!     se pierden silenciosamente — ni redirigiendo desde PowerShell
//!     se capturan. Sin la capa fichero, en producción Windows no hay
//!     NINGUNA visibilidad de bugs intermitentes.
//!
//!   * `tracing_appender::non_blocking` desacopla el write a fichero
//!     del await point de `axum` / `tokio` — un `info!` desde el
//!     handler `/video` no bloquea la respuesta HTTP ni el probe.
//!     El `WorkerGuard` que devuelve DEBE mantenerse vivo hasta el
//!     exit del proceso, o las últimas líneas se pierden en el drop.
//!
//! ## Activación
//!
//!   * Capa stderr: SIEMPRE activa salvo en modo TUI (donde
//!     corrompería la alternate screen igual que `eprintln!`).
//!   * Capa fichero: **activa por defecto** en `info`. Se apaga solo
//!     con `VIDEODROME_LOG=0` (opt-out explícito para casos con
//!     disco lleno o entornos read-only).
//!   * Rotación **diaria** (`tracing_appender::rolling::daily`) sobre
//!     `<data_local>/videodrome/logs/videodrome.log`. Al arrancar se
//!     purga cualquier fichero del directorio con mtime > 7 días.
//!     Suficiente para reproducir bugs de la semana sin engordar el
//!     disco de usuarios que nunca miran el log.
//!
//! ## Env vars
//!
//!   * `VIDEODROME_LOG=0`         → opt-out total, no se escribe fichero.
//!   * `VIDEODROME_LOG=1`         → forzar default (redundante).
//!   * `VIDEODROME_LOG=/ruta.log` → fichero explícito, SIN rotación
//!     ni prune (el user gestiona ese fichero él mismo).
//!   * `VIDEODROME_LOG_LEVEL="info,videodrome::stream=debug"` →
//!     formato `EnvFilter`. Default `info`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

pub use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Prefijo del fichero de log usado por la rotación diaria. El
/// appender produce `videodrome.log.YYYY-MM-DD` en el directorio de
/// logs. El prune al arranque solo toca ficheros con este prefijo
/// para no borrar por accidente basura de otros procesos.
const LOG_FILE_PREFIX: &str = "videodrome.log";

/// Antigüedad máxima antes del prune de arranque. 7 días cubre un
/// ciclo de fin-de-semana + reproducción típica del bug reportado
/// por el usuario, sin acumular gigas si alguien deja la app
/// corriendo semanas.
const LOG_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Info sobre la capa fichero — se rellena en `init()` y luego la
/// GUI la consume vía `#[tauri::command] log_info()` para pintar
/// la ruta en Ajustes/About + habilitar el botón "abrir carpeta".
///
/// `allow(dead_code)`: los campos solo se leen desde `gui.rs` bajo
/// `feature = "gui"`. En builds CLI/TUI la struct existe (para que
/// `init()` la pueble sin ramificar por feature) pero nadie mira su
/// contenido.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct LogInfo {
    /// `true` si la capa fichero se activó. `false` cuando
    /// `VIDEODROME_LOG=0` u otro motivo (dir no resoluble).
    pub enabled: bool,
    /// Directorio donde viven los ficheros de log rotados. `Some`
    /// para el default con rotación; `None` cuando el user ha
    /// forzado un fichero absoluto (`VIDEODROME_LOG=/path/x.log`).
    pub dir: Option<PathBuf>,
    /// Ruta del fichero de log actual (con la fecha de hoy si hay
    /// rotación, o la ruta literal si el user forzó una). Puede no
    /// existir todavía si aún no se ha escrito ninguna línea.
    pub file: Option<PathBuf>,
    /// `true` cuando el user forzó fichero fijo por env
    /// (`VIDEODROME_LOG=/path/x.log`). En ese caso el prune diario
    /// se salta — respetamos su elección literal.
    pub explicit_path: bool,
}

static LOG_INFO: OnceLock<LogInfo> = OnceLock::new();

/// Devuelve la info del log inicializado. Solo válido tras `init()`.
/// La CLI/TUI nunca la consume — solo la GUI (Tauri) — de ahí el
/// `allow(dead_code)` en builds sin `feature = "gui"`.
#[allow(dead_code)]
pub fn log_info() -> Option<LogInfo> {
    LOG_INFO.get().cloned()
}

/// Destino de la capa `tracing`. Se elige en `main.rs` según el modo:
///
///   * `Enabled`: modo GUI / CLI puro / dispatch de subcomando.
///   * `Suppressed`: modo TUI (`ratatui` con alternate screen). Se
///     usa SOLO la capa fichero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StderrPolicy {
    Enabled,
    Suppressed,
}

/// Configuración resuelta de la capa fichero.
enum FileTarget {
    /// Modo default: rotación diaria en `dir/videodrome.log.*`.
    Rotating(PathBuf),
    /// Modo explícito: un único fichero, sin rotación ni prune.
    Explicit(PathBuf),
    /// Opt-out o directorio no resoluble.
    Disabled,
}

/// Inicializa el subscriber global de `tracing`. Idempotente — si ya
/// se llamó, silenciosamente no hace nada.
///
/// Return: `(path, guard)`.
///
///   * `path` = `Some` si se activó la capa fichero (path del fichero
///     actual — con rotación, el del día de hoy).
///   * `guard` = `Some(WorkerGuard)` cuando hay capa fichero. El
///     caller (`main`) DEBE mantenerlo vivo hasta el final del
///     proceso. Al drop del guard se flushea el canal MPSC interno
///     del appender no-blocking; si se dropea antes de tiempo, o
///     nunca (p.ej. guardado en un `static` — los statics no
///     dropean al retornar de main), las últimas líneas se pierden
///     silenciosamente y el log queda amputado.
pub fn init(stderr_policy: StderrPolicy) -> anyhow::Result<(Option<PathBuf>, Option<WorkerGuard>)> {
    let filter =
        EnvFilter::try_from_env("VIDEODROME_LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("info"));

    // Timer con hora local. `OffsetTime::local_rfc_3339` puede fallar
    // si el proceso lanza threads antes de leer el offset (linux
    // multi-thread + soundness bug conocido de `chrono`). Inicializamos
    // ANTES de spawnear cualquier task, así que es seguro; si aun así
    // fallara, caemos a UTC.
    let timer = OffsetTime::local_rfc_3339().unwrap_or_else(|_| {
        OffsetTime::new(
            time::UtcOffset::UTC,
            time::format_description::well_known::Rfc3339,
        )
    });

    let stderr_enabled = matches!(stderr_policy, StderrPolicy::Enabled);
    let file_target = resolve_file_target();

    // Componer capas del subscriber con un `match` explícito sobre
    // (stderr, file). Las ramas construyen el subscriber final in
    // situ para que Rust pueda inferir el parámetro `S` de cada
    // `fmt::Layer` en cadena — factorizarlo a funciones/closures fija
    // `S = Registry` y rompe la composición con `Layered<...>`.
    let (file_result, guard_result, info) = match (stderr_enabled, file_target) {
        (true, FileTarget::Rotating(dir)) => {
            prune_old_logs(&dir);
            let (writer, current, guard) = open_rotating_writer(&dir);
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(std::io::stderr)
                        .with_ansi(atty_stderr())
                        .with_target(true)
                        .with_timer(timer.clone()),
                )
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false)
                        .with_target(true)
                        .with_timer(timer),
                )
                .try_init()
                .ok();
            let info = LogInfo {
                enabled: true,
                dir: Some(dir),
                file: Some(current.clone()),
                explicit_path: false,
            };
            (Some(current), Some(guard), info)
        }
        (true, FileTarget::Explicit(path)) => {
            let (writer, current, guard) = open_explicit_writer(path);
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(std::io::stderr)
                        .with_ansi(atty_stderr())
                        .with_target(true)
                        .with_timer(timer.clone()),
                )
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false)
                        .with_target(true)
                        .with_timer(timer),
                )
                .try_init()
                .ok();
            let info = LogInfo {
                enabled: true,
                dir: current.parent().map(|p| p.to_path_buf()),
                file: Some(current.clone()),
                explicit_path: true,
            };
            (Some(current), Some(guard), info)
        }
        (true, FileTarget::Disabled) => {
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(std::io::stderr)
                        .with_ansi(atty_stderr())
                        .with_target(true)
                        .with_timer(timer),
                )
                .try_init()
                .ok();
            let info = LogInfo {
                enabled: false,
                dir: None,
                file: None,
                explicit_path: false,
            };
            (None, None, info)
        }
        (false, FileTarget::Rotating(dir)) => {
            prune_old_logs(&dir);
            let (writer, current, guard) = open_rotating_writer(&dir);
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false)
                        .with_target(true)
                        .with_timer(timer),
                )
                .try_init()
                .ok();
            let info = LogInfo {
                enabled: true,
                dir: Some(dir),
                file: Some(current.clone()),
                explicit_path: false,
            };
            (Some(current), Some(guard), info)
        }
        (false, FileTarget::Explicit(path)) => {
            let (writer, current, guard) = open_explicit_writer(path);
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false)
                        .with_target(true)
                        .with_timer(timer),
                )
                .try_init()
                .ok();
            let info = LogInfo {
                enabled: true,
                dir: current.parent().map(|p| p.to_path_buf()),
                file: Some(current.clone()),
                explicit_path: true,
            };
            (Some(current), Some(guard), info)
        }
        (false, FileTarget::Disabled) => {
            tracing_subscriber::registry().with(filter).try_init().ok();
            let info = LogInfo {
                enabled: false,
                dir: None,
                file: None,
                explicit_path: false,
            };
            (None, None, info)
        }
    };

    let _ = LOG_INFO.set(info);
    Ok((file_result, guard_result))
}

/// Appender con rotación diaria: los ficheros salen como
/// `<dir>/videodrome.log.YYYY-MM-DD`. Devolvemos también el path del
/// fichero de hoy — es el que la GUI muestra en Ajustes/About.
fn open_rotating_writer(
    dir: &Path,
) -> (
    tracing_appender::non_blocking::NonBlocking,
    PathBuf,
    WorkerGuard,
) {
    std::fs::create_dir_all(dir).ok();
    let appender = tracing_appender::rolling::daily(dir, LOG_FILE_PREFIX);
    let today = today_string();
    let current = dir.join(format!("{LOG_FILE_PREFIX}.{today}"));
    let (writer, guard) = tracing_appender::non_blocking(appender);
    (writer, current, guard)
}

/// Appender sin rotación — modo `VIDEODROME_LOG=/ruta/x.log` donde el
/// user pide un fichero literal (típico: adjuntar a issue).
fn open_explicit_writer(
    path: PathBuf,
) -> (
    tracing_appender::non_blocking::NonBlocking,
    PathBuf,
    WorkerGuard,
) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("debug.log"));
    let appender = tracing_appender::rolling::never(dir, name);
    let (writer, guard) = tracing_appender::non_blocking(appender);
    (writer, path, guard)
}

/// Resuelve dónde escribir. Reglas (ver módulo doc):
///
///   1. `VIDEODROME_LOG=0` → `Disabled`.
///   2. `VIDEODROME_LOG=/ruta` con separador o `.log` → `Explicit`.
///   3. Cualquier otro caso (incluida ausencia total) → `Rotating`
///      en `<data_local>/videodrome/logs/`.
///   4. Si `dirs::data_local_dir()` falla → `Disabled` (nunca
///      escribimos en `.` para no ensuciar el cwd del user).
fn resolve_file_target() -> FileTarget {
    let env = std::env::var("VIDEODROME_LOG").ok();
    if let Some(val) = env.as_deref() {
        // Opt-out explícito. Aceptamos las formas más comunes.
        let trimmed = val.trim();
        if trimmed == "0"
            || trimmed.eq_ignore_ascii_case("off")
            || trimmed.eq_ignore_ascii_case("false")
            || trimmed.eq_ignore_ascii_case("no")
        {
            return FileTarget::Disabled;
        }
        // Path literal (contiene separador o termina en `.log`).
        let looks_like_path = trimmed.contains(std::path::MAIN_SEPARATOR)
            || trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed.ends_with(".log");
        if looks_like_path && !trimmed.is_empty() {
            return FileTarget::Explicit(PathBuf::from(trimmed));
        }
        // `VIDEODROME_LOG=1` / `on` / vacío → cae al default rotativo.
    }
    match dirs::data_local_dir() {
        Some(base) => FileTarget::Rotating(base.join("videodrome").join("logs")),
        None => FileTarget::Disabled,
    }
}

/// Borra ficheros `videodrome.log.*` con mtime > `LOG_MAX_AGE`. Solo
/// se llama en modo rotativo (para no tocar la ruta explícita del
/// user). Errores individuales se ignoran — no queremos que un
/// permiso torpe rompa el arranque de la app.
fn prune_old_logs(dir: &Path) {
    let Ok(iter) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in iter.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        // Solo tocamos ficheros de log nuestros. El appender genera
        // `videodrome.log`, `videodrome.log.YYYY-MM-DD`, o similares.
        if !name_str.starts_with(LOG_FILE_PREFIX) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        if now
            .duration_since(mtime)
            .map(|d| d > LOG_MAX_AGE)
            .unwrap_or(false)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Cadena `YYYY-MM-DD` para nombrar el fichero de hoy. Espejo del
/// naming interno de `tracing_appender::rolling::daily`.
fn today_string() -> String {
    use time::OffsetDateTime;
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

/// Detección barata de tty en stderr. Evita añadir `atty`/`is-terminal`
/// como dep — `std::io::IsTerminal` existe estable desde 1.70.
fn atty_stderr() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}
