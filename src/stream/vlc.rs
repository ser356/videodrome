//! Integración con VLC como reproductor externo. Handle + spawn +
//! kill activo por SO. Extraído de `stream.rs` en el refactor
//! (commit paso 2). Sin cambios de comportamiento.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
use crate::winutil::HideConsoleExt;

/// Handle del reproductor externo (VLC). Contiene:
///
/// * `alive` — flag compartido que pasa a `false` cuando VLC termina
///   (por cierre del user o por `kill()`). El caller lo consulta en
///   loop para saber cuándo liberar el stream.
/// * `kill_token` — cancellation token que, al ser disparado, cierra
///   VLC de forma activa. Necesario porque en macOS spawneamos VLC vía
///   `open -W -a VLC` (LaunchServices lanza VLC en su propio proceso);
///   matar el proceso hijo `open` NO cierra VLC. Idem en Windows con
///   `cmd /C start /wait vlc`. Por eso el kill efectivo lo hace
///   `quit_vlc()` invocando el mecanismo nativo (`osascript` /
///   `pkill` / `taskkill`) sobre el proceso VLC por nombre.
pub struct PlayerHandle {
    pub alive: Arc<std::sync::atomic::AtomicBool>,
    kill_token: CancellationToken,
}

impl PlayerHandle {
    /// Cierra VLC de forma activa. Idempotente: si VLC ya no está
    /// corriendo, `quit_vlc()` es un no-op silencioso.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn kill(&self) {
        self.kill_token.cancel();
    }
}

/// Abre la URL con VLC y devuelve un [`PlayerHandle`] con el flag
/// `alive` (pasa a `false` cuando VLC termina) y un token para cerrar
/// VLC activamente vía `PlayerHandle::kill`.
///
/// Si `sub_path` está informado, se le pasa a VLC como `--sub-file=…` para
/// que arranque con los subtítulos ya cargados.
///
/// Si `start_seconds` está informado, se pasa como `--start-time=<seg>`
/// para reanudar desde una posición concreta (feature "resume desde
/// donde lo dejaste"). Solo tiene efecto en la reproducción actual — VLC
/// hace seek dentro del stream HTTP igual que un seek de usuario.
///
/// * macOS: `open -W -a VLC --args <url> [--sub-file=<path>] [--start-time=N]`
///   — `-W` hace que `open` bloquee hasta que VLC salga del todo (⌘Q).
///   Cerrar solo la ventana no cuenta (patrón estándar macOS). Si VLC no
///   está instalado, cae a `open <url>` (sin `-W`; el flag queda `false`
///   inmediatamente).
/// * Linux: `vlc <url> [--sub-file=<path>] [--start-time=N]` directo.
/// * Windows: `start /wait vlc <url> [--sub-file=<path>] [--start-time=N]`
///   — bloquea hasta que VLC cierre.
///
/// Si no se puede lanzar ningún reproductor, el flag queda en `false` para
/// que la TUI limpie el stream inmediatamente en lugar de dejarlo colgando.
pub fn open_in_vlc(
    url: &str,
    sub_path: Option<&std::path::Path>,
    start_seconds: Option<u32>,
) -> PlayerHandle {
    use std::sync::atomic::{AtomicBool, Ordering};

    let alive = Arc::new(AtomicBool::new(true));
    let kill_token = CancellationToken::new();

    // Preparamos el arg de sub UNA sola vez para no repetir la lógica en
    // cada rama de SO. VLC acepta `--sub-file=/ruta/absoluta.srt`.
    let sub_arg: Option<String> = sub_path.map(|p| format!("--sub-file={}", p.display()));
    // `--start-time=` en segundos (VLC acepta decimales, pero aquí no los
    // necesitamos: el resume viene de una fracción sobre file_len que ya
    // se redondea en el frontend).
    let start_arg: Option<String> = start_seconds
        .filter(|n| *n > 0)
        .map(|n| format!("--start-time={n}"));

    let child_result: std::io::Result<tokio::process::Child> = {
        #[cfg(target_os = "macos")]
        {
            // `open` con -a spawnea el subproceso aunque VLC no esté
            // instalado (el error viene después en tiempo de ejecución),
            // así que el fallback Err(_) NUNCA se ejecutaba. Comprobamos
            // la existencia primero con `mdfind` / rutas estándar.
            if !macos_vlc_installed() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "VLC no est\u{e1} instalado",
                ))
            } else {
                let mut cmd = tokio::process::Command::new("open");
                cmd.args(["-W", "-a", "VLC", "--args", url]);
                if let Some(a) = sub_arg.as_deref() {
                    cmd.arg(a);
                }
                if let Some(a) = start_arg.as_deref() {
                    cmd.arg(a);
                }
                cmd.spawn()
            }
        }
        #[cfg(target_os = "linux")]
        {
            // Sin VLC en Linux no podemos abrir un stream local: xdg-open
            // sobre http://127.0.0.1:... abriría el navegador, no un
            // reproductor. Propagamos el error de spawn tal cual.
            let mut cmd = tokio::process::Command::new("vlc");
            cmd.arg(url);
            if let Some(a) = sub_arg.as_deref() {
                cmd.arg(a);
            }
            if let Some(a) = start_arg.as_deref() {
                cmd.arg(a);
            }
            cmd.spawn()
        }
        #[cfg(target_os = "windows")]
        {
            // Windows: VLC NO se añade al PATH por el instalador
            // oficial. `cmd /C start vlc` fallaría silenciosamente
            // en la mayoría de instalaciones (cmd retorna 0 aunque
            // start no encuentre el binario). Y `start` tiene un
            // parseo de comillas frágil que puede destrozar el
            // `--sub-file=` cuando la ruta lleva espacios.
            //
            // Solución: localizar `vlc.exe` con el mismo patrón que
            // en macOS (PATH → rutas estándar → registro) y
            // spawnearlo DIRECTAMENTE. Sin `start`, args como
            // elementos separados → sin quoting a mano.
            match windows_vlc_path() {
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "VLC no est\u{e1} instalado. Descargalo de https://videolan.org o inst\u{e1}lalo con `winget install VideoLAN.VLC`.",
                )),
                Some(vlc_exe) => {
                    let mut cmd = tokio::process::Command::new(vlc_exe);
                    cmd.arg(url);
                    if let Some(a) = sub_arg.as_deref() {
                        cmd.arg(a);
                    }
                    if let Some(a) = start_arg.as_deref() {
                        cmd.arg(a);
                    }
                    // VLC.exe es subsistema GUI (no debería crear
                    // consola), pero pasamos `CREATE_NO_WINDOW` por
                    // consistencia con el resto de spawns Windows —
                    // así si Windows cambia el subsistema en una
                    // futura versión no nos pilla desprevenidos.
                    cmd.hide_console();
                    cmd.spawn()
                }
            }
        }
    };

    match child_result {
        Ok(mut child) => {
            let alive_task = alive.clone();
            let kill_token_task = kill_token.clone();
            tokio::spawn(async move {
                // Race entre "VLC terminó por su cuenta" (⌘Q, cierre de
                // ventana en Linux/Windows) y "el user pulsó Detener"
                // (kill_token). En el segundo caso llamamos a
                // `quit_vlc()` — que sí sabe cerrar VLC por nombre —
                // y después esperamos a `child.wait` para no dejar
                // procesos zombies.
                tokio::select! {
                    _ = child.wait() => {}
                    _ = kill_token_task.cancelled() => {
                        quit_vlc().await;
                        let _ = child.wait().await;
                    }
                }
                alive_task.store(false, Ordering::Relaxed);
            });
        }
        Err(_) => {
            // Nada se pudo lanzar → marca como no vivo para que el caller no
            // se quede pensando que está streamando eternamente.
            alive.store(false, Ordering::Relaxed);
        }
    }

    PlayerHandle { alive, kill_token }
}

/// Cierra VLC de forma activa usando el mecanismo nativo de cada SO.
/// Idempotente: si VLC no está corriendo, cada plataforma devuelve un
/// error que ignoramos silenciosamente.
///
/// * macOS: `osascript -e 'tell application "VLC" to quit'` — envía el
///   Apple Event `quit` a VLC, que cierra limpiamente. Matar el
///   proceso hijo `open` no serviría (VLC lo lanza LaunchServices en
///   un PID independiente).
/// * Linux: `pkill -TERM -x vlc` — mata el binario `vlc` por nombre
///   exacto. Coincide con lo que spawneamos en `open_in_vlc`.
/// * Windows: `taskkill /IM vlc.exe /T` — sin `/F` para dar chance a
///   VLC de guardar estado; `/T` recoge subprocesos del árbol.
///
/// Nota: el método es "cierra CUALQUIER VLC abierto en el sistema".
/// Si el user tiene una segunda ventana de VLC con contenido no
/// relacionado, también se cerrará. Es el trade-off por no poder
/// rastrear el PID exacto (macOS/Windows lo esconden detrás del
/// launcher). En una app de streaming es el comportamiento esperado.
async fn quit_vlc() {
    #[cfg(target_os = "macos")]
    {
        let _ = tokio::process::Command::new("osascript")
            .args(["-e", "tell application \"VLC\" to quit"])
            .status()
            .await;
    }
    #[cfg(target_os = "linux")]
    {
        let _ = tokio::process::Command::new("pkill")
            .args(["-TERM", "-x", "vlc"])
            .status()
            .await;
    }
    #[cfg(target_os = "windows")]
    {
        // Sin `CREATE_NO_WINDOW` el propio `taskkill` parpadearía
        // una consola cada vez que el user pulsa Detener con VLC
        // como player.
        let mut cmd = tokio::process::Command::new("taskkill");
        cmd.args(["/IM", "vlc.exe", "/T"]);
        cmd.hide_console();
        let _ = cmd.status().await;
    }
}

/// Chequea si VLC.app está instalado en el Mac. Necesario porque
/// `open -a VLC` spawnea el proceso `open` con éxito aunque VLC no
/// exista — el error real solo aparece en tiempo de ejecución, cuando
/// `open` ya devolvió Ok. Sin este check el `Err(_)` fallback nunca
/// se disparaba.
#[cfg(target_os = "macos")]
fn macos_vlc_installed() -> bool {
    use std::path::Path;
    for path in ["/Applications/VLC.app", "/System/Applications/VLC.app"] {
        if Path::new(path).exists() {
            return true;
        }
    }
    // Fallback por si el user tiene VLC en ~/Applications u otra ruta.
    if let Some(home) = dirs::home_dir() {
        if home.join("Applications/VLC.app").exists() {
            return true;
        }
    }
    false
}

/// Localiza `vlc.exe` en Windows con la misma estrategia que
/// `macos_vlc_installed` — primero PATH (usuarios de scoop/choco),
/// luego rutas estándar (`Program Files\VideoLAN\VLC`), finalmente
/// registro (`HKLM\SOFTWARE\VideoLAN\VLC` = ruta al binario). El
/// instalador oficial de VideoLAN NO añade VLC al PATH, así que sin
/// las dos últimas ramas el `Err(NotFound)` se disparaba en el 90%
/// de las instalaciones típicas.
///
/// Devuelve `None` si VLC no está instalado — el caller propaga un
/// error explicativo con el comando de winget.
#[cfg(target_os = "windows")]
fn windows_vlc_path() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    // 1. PATH.
    if let Ok(p) = which::which("vlc.exe") {
        return Some(p);
    }

    // 2. Rutas estándar de instalación (Program Files x64 + x86).
    // Usamos las variables de entorno para respetar instalaciones
    // en volúmenes distintos de C:.
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Ok(base) = std::env::var(var) {
            let candidate = PathBuf::from(base)
                .join("VideoLAN")
                .join("VLC")
                .join("vlc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // 3. Registro. `HKLM\SOFTWARE\VideoLAN\VLC` tiene un valor por
    // defecto (`""`) que es la ruta absoluta al `vlc.exe`. Lo pone
    // el instalador oficial. Silenciamos errores de lectura — es
    // solo el último cartucho.
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        for subkey in [
            r"SOFTWARE\VideoLAN\VLC",
            r"SOFTWARE\WOW6432Node\VideoLAN\VLC",
        ] {
            if let Ok(key) = hklm.open_subkey(subkey) {
                if let Ok(path) = key.get_value::<String, _>("") {
                    let p = PathBuf::from(path);
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }

    None
}
