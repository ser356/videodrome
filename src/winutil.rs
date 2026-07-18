//! Utilidades específicas de Windows aplicables desde cualquier
//! módulo (gui y CLI/TUI). Compila en todas las plataformas — el
//! comportamiento real está gateado a `cfg(windows)`. Fuera de
//! Windows todo son no-ops.
//!
//! Vive en su propio módulo (no en `ffmpeg.rs`) porque
//! `mod ffmpeg` está detrás de `#[cfg(feature = "gui")]` y esta
//! utilidad la necesitan además `stream::open_in_vlc` y
//! `stream::quit_vlc`, que también son alcanzables desde la TUI.

/// `CREATE_NO_WINDOW` — evita que Windows abra una ventana de consola
/// al spawnear un proceso hijo con subsistema de consola desde un
/// proceso GUI (Tauri, o el atajo del Start Menu).
///
/// Usamos ESTE flag y NO:
///   * `DETACHED_PROCESS` — rompería el `AttachConsole(ATTACH_PARENT_PROCESS)`
///     del modo CLI que vive en `main.rs`.
///   * `CREATE_NEW_PROCESS_GROUP` — innecesario aquí; alteraría la
///     entrega de Ctrl+C entre procesos.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Añade `CREATE_NO_WINDOW` al `Command` para que el spawn no cree
/// una ventana `conhost.exe` en Windows. Fuera de Windows es no-op
/// — así los call sites no necesitan ensuciarse con `#[cfg(windows)]`
/// y funciona igual para `std::process::Command` y
/// `tokio::process::Command`.
///
/// Aplicable a: ffmpeg, ffprobe, taskkill, VLC (por consistencia, aun
/// siendo GUI) y cualquier otro subproceso que la app lance.
#[allow(dead_code)] // sin call sites en builds no-gui + no-windows
pub trait HideConsoleExt {
    fn hide_console(&mut self) -> &mut Self;
}

impl HideConsoleExt for std::process::Command {
    fn hide_console(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

impl HideConsoleExt for tokio::process::Command {
    fn hide_console(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}
