# videodrome

Recomendaciones basadas en tu historial de Letterboxd, búsqueda de
torrents en varios providers y streaming BitTorrent integrado en VLC.

App de escritorio (Tauri + React) y CLI/TUI en el **mismo binario**:
doble click abre la GUI, subcomandos por terminal ejecutan la CLI.

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

VLC entra automáticamente como dependencia. La app lleva **firma
ad-hoc** (no está firmada con Developer ID de Apple), pero el cask
limpia `com.apple.quarantine` en postinstall — abre con doble click sin
más pasos.

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

Instala Scoop si falta, añade los buckets `extras` (VLC) y `ser356`, y
descarga el binario prebuilt.

Flujo manual si ya tienes Scoop:

```powershell
scoop bucket add extras
scoop bucket add ser356 https://github.com/ser356/scoop-bucket
scoop install ser356/videodrome
```

Actualizar: `scoop update videodrome`.

### Linux · tarball CLI

```bash
curl -sL https://github.com/ser356/videodrome/releases/latest/download/videodrome-v0.4.4-linux-x86_64.tar.gz | tar -xz
sudo mv videodrome /usr/local/bin/
sudo apt install vlc  # o el gestor que uses
```

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
OpenSubtitles) va bakeado en el binario.

### CLI / TUI

Con cualquier subcomando cae al modo terminal (mismo binario):

```bash
videodrome recommend --count 20 --min-rating 3.5
videodrome torrents "the green mile" --year 1999
videodrome tui                       # TUI ratatui
videodrome keychain import           # solo macOS
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
| `--min-seeders <N>` | Filtro mínimo de seeders | `3` |
| `-n, --limit <N>` | Número máximo de resultados | `20` |
| `--json` | Salida JSON | `false` |

Providers activos por defecto (todos en paralelo, dedupe por
infohash):

- **YTS** (`yts.mx`) — solo cine, JSON público.
- **Apibay** (`apibay.org`) — API pública de The Pirate Bay.
- **Knaben** (`api.knaben.org`) — agregador 1337x, TPB, TorrentGalaxy,
  YTS, Nyaa, RuTracker…
- **Torznab** — opt-in. Se activa si defines `TORZNAB_URL` +
  `TORZNAB_APIKEY` (Jackett / Prowlarr).

Ranking: `seeders × calidad × idioma`. La calidad prioriza 2160p >
1080p > 720p. El idioma promociona releases con el audio original de
la película (o etiqueta `Multi`) frente a doblajes.

#### `tui`

Interfaz interactiva con hotkeys tipo vim.

| Tecla | Acción |
|---|---|
| `↑`/`↓` o `j`/`k` | Mover selección |
| `Enter` | Detalle (o abrir magnet en vista torrents) |
| `t` | Buscar torrents para la película seleccionada |
| `s` | **Stream en VLC** (torrent seleccionado) |
| `x` | Buscar subtítulos (OpenSubtitles) |
| `r` | Recargar recomendaciones con parámetros actuales |
| `+` / `-` | Rating mínimo ± 0.5 |
| `[` / `]` | Count ± 5 |
| `b` / `Esc` | Volver |
| `q` | Salir |

Al cambiar `count` o `min_rating` con las teclas hay que pulsar `r`
para recargar — la barra de estado avisa si los parámetros mostrados
están desactualizados.

Streaming: `s` arranca `librqbit` en un tempdir, sirve el fichero más
grande vía HTTP local (soporte `Range`) y abre VLC apuntando a esa URL.
Descarga secuencial priorizada por el player. Al salir de la TUI se
cancela y borra todo el temporal.

#### `keychain` (solo macOS)

```bash
videodrome keychain import   # lee .env / entorno y guarda en Keychain
videodrome keychain clear    # borra las credenciales del Keychain
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

```bash
mkdir -p ~/.config/videodrome
```

```env
LETTERBOXD_REFRESH_TOKEN=<tu_refresh_token>
LETTERBOXD_USERNAME=<tu_username>
```

Búsqueda: primero `~/.config/videodrome/.env`, luego `.env` en el CWD.

### Keychain (macOS)

En macOS las credenciales viven en el Keychain. La GUI las guarda
automáticamente tras el login. Import manual desde `.env`:

```bash
vim ~/.config/videodrome/.env       # variables que quieras importar
videodrome keychain import          # vuelca al Keychain
rm ~/.config/videodrome/.env        # opcional: limpia el .env
```

Los items aparecen en el Keychain con `Cuenta = videodrome` y
`Ubicación = letterboxd-<credencial>`.

Keychain **local** (no iCloud): en un Mac nuevo hay que volver a
importar. La sync iCloud requiere firma con perfil Apple, que un CLI
sin firmar no tiene.

---

## Caché

En `~/.config/videodrome/`:

| Fichero | TTL |
|---|---|
| `token.json` | renovación automática al expirar |
| `log_entries.json` | 1 h |
| `watchlist.json` | 1 h |
| `tmdb_recs_cache.json` | 24 h |
| `search_cache.json` | 24 h (búsquedas TMDB + torrents desde la GUI) |
| `preferences.json` | persistente (defaults de la vista Recs, idiomas de subs) |

Desde la GUI, la vista **Ajustes** permite limpiar cada caché
individualmente o todas de golpe.

---

## Desarrollo

```bash
# CLI/TUI (sin GUI)
cargo run -- recommend --count 5

# GUI (Tauri dev, hot-reload React + backend)
cd ui && npm ci && cd ..
cargo tauri dev --features gui
```

Feature flag `gui` es opt-in (default `[]`) para que `cargo build`
compile CLI-only sin webkit ni `ui/dist`. El CI valida el CLI en cada
PR; la GUI se valida en `release.yml`.
