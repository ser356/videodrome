# letterboxd-cli

CLI en Rust que genera recomendaciones de películas a partir de tu historial, watchlist y ratings en Letterboxd, cruzando con la API de TMDB. Incluye salida de texto/JSON y una interfaz interactiva de terminal (TUI).

---

## Requisitos

- Rust 1.75+ (`rustup` recomendado)
- Credenciales de la [API de Letterboxd](https://letterboxd.com/api-beta/) (solicitud manual)
- API key de [TMDB](https://www.themoviedb.org/settings/api)

---

## Instalación

```bash
git clone <repo>
cd letterboxd-cli
cargo install --path .
```

El binario queda en `~/.cargo/bin/letterboxd-cli`, que ya está en el PATH si tienes Rust instalado con `rustup`.

---

## Configuración

Crea el fichero de credenciales en `~/.config/letterboxd-cli/.env` para que funcione desde cualquier directorio:

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

También se puede poner un `.env` en el directorio de trabajo actual (útil durante el desarrollo). Si existen los dos, el global tiene prioridad.

---

## Uso

```
letterboxd-cli <COMANDO> [OPCIONES]
```

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

Atajos de teclado:

| Tecla | Acción |
|---|---|
| `↑`/`↓` o `j`/`k` | Mover selección |
| `r` | (Re)cargar recomendaciones con los parámetros actuales |
| `+`/`-` | Subir/bajar el rating mínimo en 0.5 |
| `[`/`]` | Bajar/subir el número de resultados en 5 |
| `q` / `Esc` | Salir |

Al cambiar `count` o `min_rating` con las teclas, hay que pulsar `r` para recargar — la barra de estado avisa cuando los parámetros mostrados están desactualizados.

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
