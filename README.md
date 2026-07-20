# videodrome

Recomendaciones basadas en tu historial de Letterboxd, búsqueda de
torrents en varios providers (Torrentio, YTS, EZTV, Apibay, Knaben,
Torznab opt-in) y streaming BitTorrent con player embebido (HLS —
nativo en macOS, `hls.js` en Windows/Linux) o VLC externo como
fallback. Soporta películas y series (`SxxEyy`), pistas de audio y
subtítulos embebidos al estilo Stremio.

App de escritorio (Tauri + React, UI en 6 idiomas: en/es/fr/de/it/pt)
y CLI/TUI en el **mismo binario**: doble click abre la GUI,
subcomandos por terminal ejecutan la CLI.

![demo](resources/demo.gif)

---

## Instalación

Los paquetes de macOS y Windows traen el binario **prebuilt** (~30 s) y
crean:

- Entrada en Launchpad / Menú Inicio (GUI).
- Symlink `videodrome` en el `PATH` (CLI/TUI).

Linux por ahora es CLI-only (sin bundle GUI).

### macOS · Homebrew Cask

```bash
brew tap ser356/cask https://github.com/ser356/homebrew-cask
brew install --cask videodrome
```

`ffmpeg` entra automáticamente como dependencia (lo usa el player
embebido para transmux). Si prefieres también VLC externo como
fallback: `brew install --cask vlc`.

La app lleva **firma ad-hoc** (no está firmada con Developer ID de
Apple), pero el cask limpia `com.apple.quarantine` en postinstall —
abre con doble click sin más pasos.

Si por lo que sea sigue bloqueada:

```bash
xattr -cr /Applications/Videodrome.app
```

Actualizar: `brew upgrade --cask videodrome`.

### Windows · Scoop (one-liner)

En PowerShell **no admin**:

```powershell
irm https://ser356.github.io/videodrome/install.ps1 | iex
```

Instala Scoop si falta, añade el bucket `ser356`, y descarga el
binario prebuilt. `ffmpeg` viene del bucket `main` por defecto (no
hace falta añadirlo). VLC ya no es dependencia obligatoria — el
player embebido no lo necesita.

Flujo manual si ya tienes Scoop:

```powershell
scoop bucket add ser356 https://github.com/ser356/scoop-bucket
scoop install ser356/videodrome
```

Si quieres VLC como fallback externo:

```powershell
scoop bucket add extras
scoop install extras/vlc
```

Actualizar: `scoop update videodrome`.

Si prefieres winget:

```powershell
winget install Gyan.FFmpeg
# opcional (solo si quieres el fallback externo):
winget install VideoLAN.VLC
```

Notas específicas de Windows (mirrors YTS bloqueados por ISP,
extensión HEVC opcional, checklist de smoke test, resolución de
problemas) en [docs/WINDOWS.md](docs/WINDOWS.md).

### Linux · tarball CLI

```bash
curl -sL https://github.com/ser356/videodrome/releases/latest/download/videodrome-v1.7.1-linux-x86_64.tar.gz | tar -xz
sudo mv videodrome /usr/local/bin/
sudo apt install ffmpeg
```

(En Fedora/Arch: `sudo dnf install ffmpeg` / `sudo pacman -S ffmpeg`.)
Opcional: instala también `vlc` si quieres el player externo como
fallback.

### Compilar desde código

CLI-only (sin GUI, no requiere Node/webkit):

```bash
git clone https://github.com/ser356/videodrome
cd videodrome
cargo install --path .
```

Con GUI (necesita Node 20+ y libwebkit2gtk en Linux):

```bash
cd ui && npm ci && npm run build && cd ..
cargo tauri build --features gui
```

---

## Uso

### GUI

Doble click en Launchpad / Menú Inicio, o desde terminal sin args:

```bash
videodrome
```

La primera vez te pide login de Letterboxd. Todo lo demás (TMDB,
OpenSubtitles) va bakeado en el binario. En el primer arranque la UI
detecta el idioma con `navigator.language` (fallback `en`) y lo
persiste en preferencias — puedes cambiarlo luego en **Ajustes**
(en / es / fr / de / it / pt).

### CLI / TUI

Con cualquier subcomando cae al modo terminal (mismo binario):

```bash
videodrome recommend --count 20 --min-rating 3.5
videodrome torrents "the green mile" --year 1999
videodrome torrents "the wire" --season 1 --episode 3
videodrome tui
videodrome keychain import
```

#### `recommend`

Genera recomendaciones a partir de tus películas mejor valoradas.

| Opción | Descripción | Por defecto |
|---|---|---|
| `-c, --count <N>` | Número de recomendaciones | `10` |
| `-m, --min-rating <R>` | Rating mínimo propio para usar como semilla (0.5–5.0) | `4.0` |
| `--json` | Salida JSON en stdout (útil para scripting) | `false` |

Las películas ya vistas o en watchlist se excluyen automáticamente. El
ranking es `frecuencia × rating_LB` (cuántas semillas la recomiendan,
ponderado por rating de la comunidad Letterboxd).

Los defaults (`--count`, `--min-rating`, idiomas de subtítulos) se
pueden persistir desde la vista **Ajustes** de la GUI —
`~/.config/videodrome/preferences.json`.

Ejemplo JSON:

```bash
videodrome recommend --json | jq '.[].movie.title'
```

#### `torrents`

Busca torrents para una película en varios providers a la vez, dedupea
por infohash y ordena por `seeders × calidad`.

```bash
videodrome torrents "dune" --year 2021 --min-seeders 20 -n 15
videodrome torrents --imdb tt0120689     # resuelve título vía TMDB
```

| Opción | Descripción | Por defecto |
|---|---|---|
| `<TITLE>` | Título (obligatorio salvo `--imdb`) | — |
| `--imdb <ID>` | IMDb ID (con o sin `tt`) | — |
| `--year <YYYY>` | Año (desambigua remakes) | — |
| `--tmdb-id <ID>` | TMDB ID (informativo; algún provider lo usa) | — |
| `--season <N>` | Serie: temporada. Sin `--episode` busca packs | — |
| `--episode <N>` | Serie: episodio. Requiere `--season` | — |
| `--min-seeders <N>` | Filtro mínimo de seeders | `3` |
| `-n, --limit <N>` | Número máximo de resultados | `20` |
| `--json` | Salida JSON | `false` |

Providers activos por defecto (todos en paralelo, dedupe por
infohash):

- **Torrentio** (Stremio addon) — meta-agregador (RARBG-legacy, 1337x,
  TPB, YTS, EZTV…) direccionable por IMDb, con `fileIdx` pre-resuelto
  para packs de series. Va primero porque tiene el mejor recall; los
  demás quedan como fallback / redundancia.
- **YTS** (`yts.mx`) — solo cine, JSON público.
- **EZTV** (`eztv.re` + mirrors) — solo series, con retry entre hosts.
- **Knaben** (`api.knaben.org`) — agregador 1337x, TPB, TorrentGalaxy,
  YTS, Nyaa, RuTracker…
- **Apibay** (`apibay.org`) — API pública de The Pirate Bay.
- **Torznab** — opt-in. Se activa si defines `TORZNAB_URL` +
  `TORZNAB_APIKEY` (Jackett / Prowlarr). Preferimos `t=movie&imdbid=`
  o `t=tvsearch` cuando el indexer lo soporta; fallback silencioso a
  `t=search` para configuraciones antiguas.

Cada provider tiene un presupuesto de 8 s por búsqueda y un
reintento único (backoff 500 ms) solo para errores de transporte. El
estado por provider (`ok`/`error`, número de hits, latencia, o `↺`
para hits desde caché) se expone en la GUI como línea discreta bajo
el título y sirve como telemetría honesta cuando la lista queda corta.

Matching de releases: la GUI construye hasta 3 variantes de título
(original, inglés, alternativa de TMDB) y las lanza en paralelo. El
filtrado central de `search_all` parsea cada release con un parser
estructurado (`release_name::parse`) — título, año, temporada/episodio,
resolución, source y codec salen como campos tipados. La consulta lleva
un `kind` explícito (`Movie` / `Series`) que descarta cruces (una peli
no matchea `SxxEyy`, una serie no matchea packs de películas), y
además se filtran CAMs / screeners y releases cuyo `parsed.title`
normalizado no matchea ninguna variante.

Ranking: `seeders × calidad × idioma`. La calidad prioriza 2160p >
1080p > 720p. El idioma promociona releases con el audio original de
la película (o etiqueta `Multi`) frente a doblajes.

#### `tui`

Interfaz interactiva con hotkeys tipo vim.

| Tecla | Acción |
|---|---|
| `↑`/`↓` o `j`/`k` | Mover selección |
| `Enter` | Detalle / abrir magnet (vista torrents) |
| `t` | Buscar torrents para la película seleccionada |
| `s` | **Stream en VLC** (torrent seleccionado) |
| `x` | Buscar subtítulos (OpenSubtitles) |
| `m` | Panel de detalles del release |
| `r` | Recargar recomendaciones con parámetros actuales |
| `+` / `-` | Rating mínimo ± 0.5 |
| `[` / `]` | Count ± 5 |
| `b` / `Esc` | Volver |
| `q` | Salir |

La TUI también incluye una vista **Search** para buscar torrents por
título directamente, sin pasar por recomendaciones.

Al cambiar `count` o `min_rating` con las teclas hay que pulsar `r`
para recargar — la barra de estado avisa si los parámetros mostrados
están desactualizados.

Streaming (TUI): `s` arranca `librqbit` en un tempdir, sirve el fichero
más grande vía HTTP local (soporte `Range`) y abre VLC apuntando a esa
URL. Descarga secuencial priorizada por el player. Al salir de la TUI
se cancela y borra todo el temporal.

Streaming (GUI): por defecto el player es **embebido en la propia
app** — `<video>` HTML alimentado por `ffmpeg` en modo HLS transmux.
En macOS el `<video>` reproduce HLS de forma nativa (WKWebView); en
Windows y Linux se usa `hls.js` sobre WebView2 / WebKitGTK. Elige entre
`Auto` (copy si el códec es compatible, transcode si no), `Copy`
(forzar `-c:v copy`, cero pérdida) o `Transcode` (forzar CRF 18) en
Ajustes. Ofrece cambio de pista de audio, subtítulos embebidos
(estilo Stremio) y subtítulos externos de OpenSubtitles (SRT→WebVTT
al vuelo). Requiere `ffmpeg` y `ffprobe` en PATH — los packagers
oficiales (Homebrew cask, Scoop) los declaran como dependencia. Si
prefieres VLC externo hay un toggle en Ajustes (`default_player`) y
"Abrir en VLC" queda siempre en el menú contextual del release.

#### `keychain` (solo macOS)

```bash
videodrome keychain import
videodrome keychain export --to ~/.config/videodrome/.env
videodrome keychain clear
```

Ver la sección [Configuración](#configuración) abajo.

---

## Configuración

Credenciales de app (Letterboxd client_id/secret, TMDB bearer,
OpenSubtitles API key) van **bakeadas** en los binarios oficiales — no
tienes que configurarlas.

Solo necesitas tu **refresh_token** y **username** de Letterboxd, que
la GUI captura por ti en el login. Si prefieres el flujo `.env`:

### `.env` (Linux/Windows)

Ruta canónica (según SO):

- Linux: `~/.config/videodrome/.env`
- Windows: `%APPDATA%\videodrome\.env`

Contenido mínimo:

```env
LETTERBOXD_REFRESH_TOKEN=<tu_refresh_token>
LETTERBOXD_USERNAME=<tu_username>
```

Búsqueda en cascada: `<config_dir>/videodrome/.env` → `~/.config/videodrome/.env`
(legacy en Windows) → `.env` en el CWD.

Opcional para activar el provider Torznab (Jackett / Prowlarr):

```env
TORZNAB_URL=http://localhost:9117/api/v2.0/indexers/all/results/torznab
TORZNAB_APIKEY=<tu_api_key>
```

### Keychain (macOS)

En macOS las credenciales viven en el Keychain. La GUI las guarda
automáticamente tras el login. Import manual desde `.env`:

```bash
vim ~/.config/videodrome/.env
videodrome keychain import
rm ~/.config/videodrome/.env
```

Los items aparecen en el Keychain con `Cuenta = videodrome` y
`Ubicación = letterboxd-<credencial>`.

Keychain **local** (no iCloud): en un Mac nuevo hay que volver a
importar. La sync iCloud requiere firma con perfil Apple, que un CLI
sin firmar no tiene.

### Preferencias (GUI)

La vista **Ajustes** persiste en `preferences.json` (junto al `.env`).
Los campos editables:

| Campo | Descripción | Por defecto |
|---|---|---|
| `default_min_rating` | Rating mínimo por defecto en Recs (0.5 – 5.0) | `4.0` |
| `subtitle_languages` | Idiomas de subs para OpenSubtitles (ISO 639-1, coma) | `en,es` |
| `stream_cache_ttl_days` | Días antes de purgar caché de streams | `7` |
| `ui_language` | Idioma UI (`en`/`es`/`fr`/`de`/`it`/`pt`) | auto |
| `default_player` | `html` (embebido) o `vlc` (externo) | `html` |
| `quality_mode` | `auto` / `copy` (sin pérdida, fuerza copy) / `transcode` (CRF 18) | `auto` |
| `hls_disk_budget_gb` | Presupuesto de disco de segmentos `.ts` con LRU | `8` |
| `glass_opacity` | Opacidad de superficies "liquid glass" (0–100) | `0` |

---

## Rutas del sistema

| Uso | macOS | Linux | Windows |
|---|---|---|---|
| Config + `.env` + prefs + tokens | `~/Library/Application Support/videodrome/` | `~/.config/videodrome/` | `%APPDATA%\videodrome\` |
| Caché de streams (segmentos `.ts`, resume) | `~/Library/Caches/videodrome/streams/` | `~/.cache/videodrome/streams/` | `%LOCALAPPDATA%\videodrome\streams\` |
| Logs (`debug.log`) | `~/Library/Application Support/videodrome/` | `~/.local/share/videodrome/` | `%LOCALAPPDATA%\videodrome\` |

### Ficheros de caché (config_dir)

| Fichero | TTL |
|---|---|
| `token.json` | renovación automática al expirar |
| `credentials.json` | persistente (fallback de Keychain) |
| `log_entries.json` | 1 h |
| `watchlist.json` | 1 h |
| `tmdb_recs_cache.json` | 24 h |
| `search_cache.json` | 24 h (hits de TMDB por texto en la vista Search) |
| `torrent_search_cache.json` | 30 min con hits · 60 s si algún provider fallaba · 5 min si el resultado quedó vacío |
| `preferences.json` | persistente |

Desde la GUI, la vista **Ajustes** permite limpiar cada caché
individualmente o todas de golpe, incluyendo la caché de streams
(`<cache_dir>/videodrome/streams/`, con segmentos `.ts` y ficheros
`resume.json` por infohash).

---

## Desarrollo

CLI/TUI (sin GUI):

```bash
cargo run -- recommend --count 5
```

GUI (Tauri dev, hot-reload React + backend):

```bash
cd ui && npm ci && cd ..
cargo tauri dev --features gui
```

Feature flag `gui` es opt-in (default `[]`) para que `cargo build`
compile CLI-only sin webkit ni `ui/dist`. El CI valida el CLI en
Linux/macOS/Windows y la GUI en macOS/Windows en cada PR; `release.yml`
publica los assets al taggear.

Antes de tagear, checklist local (el CI de release NO corre clippy):

```bash
cargo check
cargo check --features gui
cargo clippy --all-targets -- -D warnings
cargo clippy --features gui --all-targets -- -D warnings
cargo test --features gui
cd ui && npm run lint
```

---

## Reportar bugs (logs)

Todo el stderr se ha migrado a `tracing`. Desde v1.1.6 la capa
fichero está **activa por defecto** a nivel `info` con rotación
diaria: los ficheros viven en `<data_local>/videodrome/logs/` con
nombre `videodrome.log.YYYY-MM-DD`, y al arrancar la app se borra
cualquier fichero con más de 7 días. Rutas concretas:

| Sistema | Carpeta de logs |
|---|---|
| macOS   | `~/Library/Application Support/videodrome/logs/` |
| Linux   | `~/.local/share/videodrome/logs/` |
| Windows | `%LOCALAPPDATA%\videodrome\logs\` |

En Ajustes → **Acerca de** tienes la ruta exacta y un botón "Abrir
carpeta de logs".

Opt-out (sin fichero, solo stderr):

```bash
VIDEODROME_LOG=0 videodrome
```

Forzar una ruta concreta (útil para adjuntar a un issue; sin
rotación ni prune, gestionas el fichero tú):

```bash
VIDEODROME_LOG=/tmp/videodrome-bug.log videodrome
```

Subir la verbosidad para cazar un bug intermitente con
`VIDEODROME_LOG_LEVEL` (formato `EnvFilter`):

```bash
VIDEODROME_LOG_LEVEL=debug videodrome
VIDEODROME_LOG_LEVEL="info,videodrome::stream=debug" videodrome
```

Targets útiles para filtrar: `video`, `probe`, `warmup`, `hls`,
`hls-evict`, `ffmpeg-hls`, `torrent`, `resume`, `subs`, `tmdb`,
`gui`, `eztv`, `torrentio`, `ffmpeg`, `logging`.

Al abrir issue, adjunta el fichero del día. La app corre 100% local —
el log solo contiene rangos de bytes servidos, timings de ffprobe/
ffmpeg, y decisiones del scheduler de librqbit. Sin credenciales, sin
infohashes, sin nombres de ficheros.
