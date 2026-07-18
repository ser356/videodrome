# videodrome en Windows — smoke test y notas de instalación

Documento de referencia para la PRIMERA vez que ejecutes videodrome
en Windows 10/11 (x64). Cubre requisitos, instalación de
dependencias y el checklist manual que valida la retrocompatibilidad
antes de fusionar en `main`.

## Requisitos

Windows 10 build 17763+ o Windows 11. Todo lo demás Tauri lo
descubre solo (WebView2 viene preinstalado en 11 y actualizado por
Windows Update en 10 moderno).

### Dependencias externas (opcionales según flujo)

| Dep | Para | Instalación recomendada |
|---|---|---|
| **ffmpeg + ffprobe** | Player HTML embebido (transmux MKV/HEVC → HLS) | `winget install Gyan.FFmpeg` |
| **VLC** | Fallback externo (`default_player = "vlc"`) o modo manual | Instalador oficial de [videolan.org](https://videolan.org) o `winget install VideoLAN.VLC` |
| **HEVC Video Extensions** | Reproducir MP4/HEVC en modo DIRECT dentro de WebView2 (opcional; sin él caemos a HLS transmux automáticamente) | Microsoft Store: "HEVC Video Extensions" ($0.99 oficial, o gratis vía el "device manufacturer" link) |

La app funciona sin la extensión HEVC — el frontend detecta con
`canPlayType('hvc1…')` y fuerza la ruta transmux si el codec no
está disponible. La extensión solo mejora latencia de arranque en
MP4/HEVC (evita spawnear ffmpeg).

### Descubrimiento automático

- **ffmpeg**: si `ffmpeg.exe` no está en `PATH`, la app busca en:
  - `C:\ffmpeg\bin` (unzip manual desde gyan.dev / BtbN)
  - `%LOCALAPPDATA%\Microsoft\WinGet\Links` (winget)
  - `%USERPROFILE%\scoop\shims` (scoop)
  - `%ChocolateyInstall%\bin` (chocolatey)
- **VLC**: si `vlc.exe` no está en `PATH`, la app busca en:
  - `%ProgramFiles%\VideoLAN\VLC\vlc.exe`
  - `%ProgramFiles(x86)%\VideoLAN\VLC\vlc.exe`
  - Registro: `HKLM\SOFTWARE\VideoLAN\VLC` (valor por defecto)

Ninguna requiere modificar `PATH` a mano — es lo que se salió mal
en las builds pre-audit.

## Smoke test checklist

Ejecutar UNA vez en máquina Windows real (no VM sin GPU si vas a
tocar HEVC) tras cada cambio grande. Marca ✅ / ❌ y anota
observaciones.

### Instalación

- [ ] `winget install Gyan.FFmpeg` completa OK. `ffmpeg -version` en
      una nueva ventana de PowerShell responde.
- [ ] Instalador oficial de VLC completa OK. NO añade VLC al PATH
      (verificado: `where vlc` falla). `%ProgramFiles%\VideoLAN\VLC\vlc.exe`
      existe.
- [ ] `videodrome-Setup-x.y.z.exe` (o el `.msi`) del release se
      instala sin warnings de SmartScreen bloqueantes.

### Arranque

- [ ] La app abre sin mostrar consola cmd (verificar: sin ventana
      negra parpadeando).
- [ ] Login con credenciales de Letterboxd funciona. `credentials.json`
      aparece en `%APPDATA%\videodrome\`.
- [ ] Vista Recomendaciones carga con posters de TMDB. Ningún error
      de CSP en DevTools (F12 en la webview).

### Búsqueda de torrents

- [ ] Buscar "The Matrix 1999". La línea de estado bajo el título
      muestra `yts ✓ N · knaben ✓ N · apibay ✓ N` (o `↻` en algunos).
      Ninguno debe estar en `✗ error de red` de forma sistemática.
- [ ] Si `yts ✗ ...` aparece, verificar que no es DNS bloqueado por
      el ISP; la app hace fallback entre 5 mirrors, uno debería
      cazar.

### Player HTML (crítico — la razón de este audit)

- [ ] MKV/H.264: reproduce vía HLS transmux. hls.js aparece en las
      requests de DevTools cargando `.ts` desde `127.0.0.1`.
- [ ] MP4/H.264 puro: reproduce vía path DIRECT (no ffmpeg
      spawneado; ver ausencia de proceso `ffmpeg.exe` en Task
      Manager mientras suena).
- [ ] MP4/HEVC:
  - Con extensión HEVC instalada: reproduce DIRECT.
  - Sin ella: la UI detecta `hvc1` no soportado y cae a HLS
    transmux automáticamente (ffmpeg spawnea, funciona).
- [ ] Seek largo (drag a mitad de peli con playback ya
    empezado): el StremioLoader aparece, buffer se rellena, playback
    resume en el offset correcto. NO se queda colgado.
- [ ] Subs: seleccionar un `.srt` de OpenSubtitles → aparece
    superpuesto correctamente. `[` `]` ajustan sync.
- [ ] Fullscreen con `F`: se activa/desactiva y el cursor
    desaparece a los ~2.5 s sin mover el ratón.
- [ ] Volver atrás con `Esc` mata el stream (verificar en Task
    Manager: `ffmpeg.exe` desaparece antes de 2s).

### Sin ventanas de consola parásitas (regresión Scoop)

Contexto: hasta la corrección del audit "Windows: consola ffprobe +
huérfanos Scoop", instalar ffmpeg con Scoop provocaba (a) una ventana
`conhost.exe` visible sobre el player y (b) procesos ffprobe/ffmpeg
huérfanos que no morían al cerrar. Estos checks reproducen ambos
casos.

- [ ] Con ffmpeg instalado por **Scoop** (`scoop install ffmpeg`),
    reproducir un vídeo por transmux (MKV/H.264). En NINGÚN momento
    aparece una ventana de consola — ni al arrancar el probe, ni al
    lanzar ffmpeg, ni al hacer seek.
- [ ] Tras pulsar Detener, `Get-Process ffmpeg,ffprobe -ErrorAction
    SilentlyContinue` en PowerShell devuelve vacío (ni shim ni
    binario real vivos).
- [ ] Hacer 3 seeks largos seguidos (drag a distintas posiciones)
    y volver a comprobar: durante reproducción SOLO un `ffmpeg.exe`
    vivo; tras Detener, cero.
- [ ] Repetir el primer punto con ffmpeg instalado por **winget**
    (`winget install Gyan.FFmpeg`) — sin shims — para confirmar que
    la resolución de shims no rompe el caso normal.

### Fallback VLC

- [ ] Cambiar `default_player = "vlc"` en Ajustes.
- [ ] Reproducir un torrent: VLC.exe se lanza directamente (sin
    consola cmd intermedia). El stream se ve.
- [ ] Cerrar VLC con la X: la app detecta la muerte del proceso
    y libera el slot del stream.
- [ ] Pulsar "Detener" en la app: VLC se cierra limpiamente
    (`taskkill /IM vlc.exe /T`).

### Limpieza

- [ ] Tras cerrar la app, `%TEMP%` NO acumula carpetas
    `videodrome-hls-*` / `videodrome-stream-*` — el barrido al
    siguiente arranque (o el cleanup normal en cierre limpio) las
    borra. Si queda alguna huérfana tras una sesión limpia, es un
    bug: reportarlo con el nombre del dir.
- [ ] Ajustes → Limpiar caché → seleccionar `torrent_search`.
    `%APPDATA%\videodrome\torrent_search_cache.json` queda vacío o
    borrado.

## Problemas conocidos

- **SmartScreen bloquea el .exe descargado** — el binario NO está
  firmado con un cert EV. El user tiene que pulsar "Más información
  → Ejecutar de todas formas". Documentado en README como
  limitación aceptada por coste del cert.
- **WebView2 primera ejecución tarda 2-3s** — la runtime hace
  cold-start del proceso hijo. No es un bug de la app.
- **`ffmpeg` versión ≥ 5.0 recomendado** — versiones antiguas
  (Ubuntu 20.04 default = 4.2) fallan al transmuxear HEVC 10-bit.
  En Windows con winget siempre te da la última.

## Reportar problemas

Si algo del checklist falla, abre issue en el repo con:
- Versión de Windows (`winver`).
- Versión de ffmpeg (`ffmpeg -version | head -1`).
- Contenido de `%APPDATA%\videodrome\` (sin `.env` ni
  `credentials.json`).
- DevTools console de la webview (F12 → Console).
