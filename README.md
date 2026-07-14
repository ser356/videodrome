# letterboxd-cli

CLI en Rust que genera recomendaciones de películas a partir de tu historial, watchlist y ratings en Letterboxd, cruzando con la API de TMDB. Incluye salida de texto/JSON y una interfaz interactiva de terminal (TUI).

![demo](resources/demo.gif)

---

## Requisitos

- **VLC** para la funcionalidad de streaming BitTorrent (`s` en la TUI). Se instala
  automáticamente si usas alguno de los gestores de paquetes recomendados abajo.

Para compilar desde código además:
- Rust 1.75+ (`rustup` recomendado)

---

## Instalación

Recomendado: **usa un gestor de paquetes**. Los tres compilan desde
código en tu máquina, así que:

- El binario resultante nunca dispara Gatekeeper (macOS) ni SmartScreen
  (Windows) porque no lleva la marca "descargado de internet".
- Funciona en cualquier arquitectura (arm64/x86_64 mac, x86_64 windows,
  todas las distros Linux) sin publicar binarios prebuilt para cada una.

Contrapartida: la instalación tarda **2–4 minutos** por compilar.

### macOS · Homebrew ⭐️

```bash
brew tap ser356/tap
brew trust ser356/tap
brew install letterboxd-cli
brew install --cask vlc
```

Actualización: `brew upgrade letterboxd-cli`.

> `brew trust` es un paso obligatorio desde Homebrew 4.5+ para taps de
> terceros — solo hay que hacerlo una vez por tap.
>
> VLC se instala aparte porque Homebrew ya no permite que una fórmula
> dependa de un cask.
>
> `brew install` compilará el CLI en local (~2–4 min); rust se instala
> automáticamente como build-dependency si no lo tienes.

### Windows · Scoop ⭐️

**Una línea en PowerShell** (no admin):

```powershell
irm https://ser356.github.io/letterboxd-cli/install.ps1 | iex
```

Instala Scoop (si no lo tienes), añade los buckets necesarios, compila
letterboxd-cli desde código en tu máquina y trae VLC + rustup como
dependencias. ~5-10 min la primera vez. Actualización: `scoop update
letterboxd-cli`.

Si ya tienes Scoop y prefieres el flujo manual:

```powershell
scoop bucket add main
scoop bucket add extras
scoop bucket add ser356 https://github.com/ser356/scoop-bucket
scoop install ser356/letterboxd-cli
```

### Linux ⭐️

Compila desde código con `cargo` (rustup suele venir preinstalado en
distros de desarrollo). Instala VLC con tu gestor nativo:

```bash
cargo install --git https://github.com/ser356/letterboxd-cli
sudo apt install vlc
```

**NixOS / Nix**: si usas Nix hay un flake preparado:

```bash
nix profile install github:ser356/letterboxd-cli
```

Compila desde código de forma reproducible y trae VLC en el `PATH` del
binario automáticamente.

### Alternativa — Binarios prebuilt (releases)

Si no quieres esperar a la compilación local, descarga el archivo de tu
plataforma desde [Releases](https://github.com/ser356/letterboxd-cli/releases):

- `letterboxd-cli-macos-arm64.tar.gz`
- `letterboxd-cli-linux-x86_64.tar.gz`
- `letterboxd-cli-windows-x86_64.zip`

Descomprime, mueve el binario a algún directorio del `PATH`, e instala VLC
por tu cuenta. En macOS puede saltar Gatekeeper la primera vez — quítale la
cuarentena con:

```bash
xattr -d com.apple.quarantine letterboxd-cli
```

### Compilar en clon local

```bash
git clone https://github.com/ser356/letterboxd-cli
cd letterboxd-cli
cargo install --path .
```

El binario queda en `~/.cargo/bin/letterboxd-cli`, que ya está en el `PATH`
si tienes Rust instalado con `rustup`. Instala VLC por tu cuenta si
quieres usar streaming.

---

## Configuración

La fuente de credenciales depende del sistema operativo:

- **macOS: Keychain, sin fallback a `.env`.** Poblar el Keychain con `letterboxd-cli keychain import` (ver más abajo). Si una credencial no está en el Keychain, el CLI aborta con un mensaje claro.
- **Linux / Windows:** variables de entorno o `.env`.

### `.env` (Linux/Windows, o macOS solo para importar)

```bash
mkdir -p ~/.config/letterboxd-cli
```

```env
LETTERBOXD_CLIENT_ID=<tu_client_id>
LETTERBOXD_CLIENT_SECRET=<tu_client_secret>
LETTERBOXD_REFRESH_TOKEN=<tu_refresh_token>
LETTERBOXD_USERNAME=<tu_username>
TMDB_BEARER_TOKEN=<tu_tmdb_bearer_token>
```

Se busca primero `~/.config/letterboxd-cli/.env`, y como fallback `.env` en el directorio actual.

### Credenciales en el Keychain de macOS

En macOS todas las credenciales viven en el Keychain, incluidas `LETTERBOXD_USERNAME` y
`TMDB_BEARER_TOKEN`. Flujo típico:

```bash
# 1. Crea un .env temporal con las variables que quieras importar
#    (basta con las que falten; import es tolerante)
vim ~/.config/letterboxd-cli/.env

# 2. Vuelca al Keychain
letterboxd-cli keychain import

# 3. (Opcional) borra el .env — las credenciales viven ya en el Keychain
rm ~/.config/letterboxd-cli/.env

# Para eliminarlas del Keychain más adelante:
letterboxd-cli keychain clear
```

En el Keychain aparecen como items de contraseña genérica con `Cuenta = letterboxd-cli` y
`Ubicación = letterboxd-<credencial>` (`letterboxd-client-id`, `letterboxd-client-secret`,
`letterboxd-refresh-token`, `letterboxd-username`, `letterboxd-tmdb-bearer-token`).

> **Nota:** esto usa el Keychain local de ese Mac (login keychain), no el Keychain de iCloud. Un
> item de Keychain solo se sincroniza por iCloud si se marca explícitamente como
> `kSecAttrSynchronizable`, y eso requiere que el binario esté firmado con un perfil de
> aprovisionamiento de Apple — algo que un CLI sin firmar (`cargo install`) no tiene. Si usas
> `letterboxd-cli` en varios Macs, hay que ejecutar `keychain import` en cada uno.
>
> Este comando solo funciona compilado para macOS; en Linux/Windows devuelve un error explicando
> que el Keychain no está disponible.

---

## Uso

```
letterboxd-cli [COMANDO] [OPCIONES]
```

Si se omite el comando, arranca la **TUI** con los valores por defecto
(`count=10`, `min_rating=4.0`). Es decir, `letterboxd-cli` ≡ `letterboxd-cli tui`.

### recommend

Genera recomendaciones basadas en las películas que mejor has valorado.

```bash
letterboxd-cli recommend
```

Opciones:

| Opción | Descripción | Por defecto |
|---|---|---|
| `-c, --count <N>` | Número de recomendaciones | `10` |
| `-m, --min-rating <R>` | Rating mínimo propio para usar una película como semilla (escala 0.5–5.0) | `4.0` |
| `--json` | Imprime las recomendaciones como JSON en stdout (útil para scripting) | `false` |

Las películas que ya están en tu watchlist se excluyen automáticamente, igual que las que ya has visto.

Ejemplos:

```bash
# Top 10 con la config por defecto
letterboxd-cli recommend

# Top 20 incluyendo películas con rating >= 3.5
letterboxd-cli recommend --count 20 --min-rating 3.5

# Salida JSON para scripting
letterboxd-cli recommend --json | jq '.[].movie.title'
```

Salida de ejemplo:

```
  Recomendaciones para sekitoguapo

   1.  La milla verde                             ★ 4.29
   2.  Terminator                                 ★ 3.88
   3.  Upgrade (Ilimitado)                        ★ 3.65
   ...
```

El rating mostrado es el de la comunidad de Letterboxd (escala 0.5–5.0). El ranking se calcula como `frecuencia × rating_LB`: cuántas de tus películas semilla recomiendan esa película, ponderado por su valoración en Letterboxd.

### tui

Abre una interfaz interactiva de terminal que carga las recomendaciones en segundo plano.

```bash
letterboxd-cli tui
letterboxd-cli tui --count 20 --min-rating 3.5
```

Atajos de teclado (vista de recomendaciones):

| Tecla | Acción |
|---|---|
| `↑`/`↓` o `j`/`k` | Mover selección |
| `t` | Buscar torrents para la película seleccionada |
| `r` | (Re)cargar recomendaciones con los parámetros actuales |
| `+`/`-` | Subir/bajar el rating mínimo en 0.5 |
| `[`/`]` | Bajar/subir el número de resultados en 5 |
| `q` / `Esc` | Salir |

Atajos en la vista de torrents:

| Tecla | Acción |
|---|---|
| `↑`/`↓` o `j`/`k` | Mover selección |
| `Enter` | Abrir el magnet con el handler del sistema (Transmission, qBittorrent…) |
| `s` | **Stream en VLC**: arranca un cliente BitTorrent embebido (librqbit), descarga secuencialmente el fichero más grande y lo sirve por HTTP local; abre VLC apuntando a esa URL para previsualizarlo mientras baja. |
| `b` / `Esc` | Volver a la lista de recomendaciones |
| `q` | Salir (para el streaming y borra el temporal) |

Al cambiar `count` o `min_rating` con las teclas, hay que pulsar `r` para recargar — la barra de estado avisa cuando los parámetros mostrados están desactualizados.

### torrents

Busca torrents para una película concreta en varios providers a la vez.

```bash
letterboxd-cli torrents "the green mile" --year 1999
letterboxd-cli torrents --imdb tt0120689     # resuelve título vía TMDB
letterboxd-cli torrents "dune" --min-seeders 20 -n 15 --json
```

Opciones:

| Opción | Descripción | Por defecto |
|---|---|---|
| `<TITLE>` (posicional) | Título (obligatorio salvo que se pase `--imdb`) | — |
| `--imdb <ID>` | IMDb ID (con o sin `tt`). Si no se pasa título, se resuelve vía TMDB | — |
| `--year <YYYY>` | Año (ayuda a desambiguar remakes) | — |
| `--tmdb-id <N>` | TMDB ID (informativo por ahora) | — |
| `--min-seeders <N>` | Filtro mínimo de seeders | `3` |
| `-n, --limit <N>` | Número máximo de resultados | `20` |
| `--json` | Salida JSON en lugar de texto | `false` |

**Providers** (activados por defecto):

- **YTS** (`yts.mx`) — API JSON pública, solo cine. Puede fallar si tu red bloquea el dominio.
- **Knaben** (`api.knaben.org`) — agregador que consulta 1337x, TPB, TorrentGalaxy (cuando funciona), YTS, Nyaa, RuTracker, etc. Es el que da más cobertura.
- **Torznab** (Jackett/Prowlarr) — **opt-in**: se activa si defines las variables:

  ```bash
  export TORZNAB_URL="http://localhost:9117/api/v2.0/indexers/all/results/torznab/api"
  export TORZNAB_APIKEY="tu_apikey_de_jackett"
  ```

  Con eso puedes usar cualquier indexer que tengas configurado en tu Jackett o Prowlarr.

Los resultados se dedupean por infohash y se ordenan por `seeders × calidad`
(2160p pesa más que 1080p, etc.). Cada torrent muestra tamaño, seeders,
leechers, calidad detectada, provider y magnet completo.

### keychain (solo macOS)

Gestiona las credenciales guardadas en el Keychain de macOS. Ver [Credenciales en el Keychain de
macOS](#credenciales-en-el-keychain-de-macos) arriba.

```bash
letterboxd-cli keychain import
letterboxd-cli keychain clear
```

---

## Caché

Para evitar llamadas repetidas a la API:

- Token OAuth: `~/.config/letterboxd-cli/token.json` — se renueva automáticamente al expirar
- Historial de Letterboxd: `~/.config/letterboxd-cli/log_entries.json` — TTL 1 hora
- Watchlist de Letterboxd: `~/.config/letterboxd-cli/watchlist.json` — TTL 1 hora
- Recomendaciones de TMDB por película: `~/.config/letterboxd-cli/tmdb_recs_cache.json` — TTL 24 horas

---

## Compilar sin instalar

```bash
cargo build --release
./target/release/letterboxd-cli recommend
```

---

## Estado del CLAUDE.md

El fichero [CLAUDE.md](CLAUDE.md) contiene la especificación técnica del proyecto y sirve como referencia para el desarrollo.
