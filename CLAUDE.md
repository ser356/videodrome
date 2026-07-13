# letterboxd-cli

CLI en Rust para obtener recomendaciones de películas basadas en el historial y ratings de Letterboxd,
usando la API no oficial de Letterboxd y la API de TMDB. Incluye una interfaz interactiva (TUI) además
del modo de texto plano.

---

## Credenciales

Todas las credenciales van en `.env`, en la raíz del proyecto (desarrollo) o en
`~/.config/letterboxd-cli/.env` (uso global — tiene prioridad si existe). Nunca se commitean.

```env
LETTERBOXD_CLIENT_ID=<tu_client_id>
LETTERBOXD_CLIENT_SECRET=<tu_client_secret>
LETTERBOXD_REFRESH_TOKEN=<tu_refresh_token>
LETTERBOXD_USERNAME=<tu_username>
TMDB_BEARER_TOKEN=<tu_tmdb_bearer_token>
```

`.env` está en `.gitignore`. El binario lee las variables con `dotenvy` (ver `config.rs`).

> Nota: TMDB se usa con **Bearer token** (`Authorization: Bearer <token>`, el "API Read Access Token"
> de TMDB v4), no con el `api_key` de query string de la v3.

### Keychain de macOS (opcional, `keychain.rs`)

En macOS, cada credencial sensible (`client_id`, `client_secret`, `refresh_token`,
`tmdb_bearer_token` — `username` no) se busca primero en el Keychain local del Mac
(`Config::from_env()` en `config.rs`) y, si no está, se cae a `.env`. Se gestiona con:

```
letterboxd-cli keychain import   # lee .env y lo guarda en el Keychain
letterboxd-cli keychain clear    # borra esas entradas del Keychain
```

Implementado con el crate `keyring` (feature `apple-native`), como dependencia
`[target.'cfg(target_os = "macos")'.dependencies]` — no se compila en otros sistemas operativos.
`keychain.rs` expone `get`/`set`/`delete` con la misma firma en todas las plataformas: en
Linux/Windows `get` devuelve `None` (fallback silencioso a `.env`) y `set`/`delete` devuelven
error explicando que el Keychain no está disponible.

Es el Keychain **local** de ese Mac (login keychain), no el Keychain de iCloud: un item solo se
sincroniza por iCloud si se marca `kSecAttrSynchronizable`, y eso requiere que el binario esté
firmado con un perfil de aprovisionamiento — algo que un CLI sin firmar no tiene. En un Mac nuevo
hay que volver a ejecutar `keychain import`.

---

## Arquitectura

```
letterboxd-cli/
├── src/
│   ├── main.rs          # Punto de entrada, CLI (clap): subcomandos `recommend` y `tui`
│   ├── auth.rs          # OAuth2: refresh_token → access_token, caché en disco
│   ├── letterboxd.rs    # Cliente HTTP para la API de Letterboxd (log-entries, watchlist, ratings)
│   ├── tmdb.rs          # Cliente HTTP para la API de TMDB (recomendaciones, con caché)
│   ├── recommend.rs     # Lógica de recomendación (funciones puras + orquestación async)
│   ├── progress.rs      # Trait `Progress` + implementación indicatif para la CLI
│   ├── tui.rs            # Interfaz interactiva (ratatui + crossterm)
│   ├── keychain.rs      # Acceso al Keychain de macOS (get/set/delete, no-op en otros SO)
│   └── config.rs        # Carga de variables de entorno (Keychain → .env)
├── .github/workflows/ci.yml  # fmt, clippy, test, build (matrix ubuntu/macos)
├── .env                 # Credenciales (no commitear)
├── .gitignore
├── Cargo.toml
└── CLAUDE.md
```

---

## Dependencias principales

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
reqwest = { version = "0.12", features = ["json", "gzip"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dotenvy = "0.15"
anyhow = "1"
dirs = "5"
indicatif = "0.17"
colored = "2"
ratatui = "0.29"
crossterm = "0.28"
futures = "0.3"

# Solo en macOS:
keyring = { version = "3", features = ["apple-native"] }
```

---

## Flujo de autenticación (`auth.rs`)

La API de Letterboxd usa OAuth2. El `access_token` caduca cada 3600 segundos.

1. Al arrancar, leer el token cacheado de `~/.config/letterboxd-cli/token.json`.
2. Si no existe o ha expirado, llamar a `/auth/token` con `grant_type=refresh_token`.
3. Guardar el nuevo `access_token` y su `expires_at` (Unix timestamp) en el caché.
4. Adjuntar `Authorization: Bearer <access_token>` en todas las llamadas autenticadas.

Endpoint de refresh:
```
POST https://api.letterboxd.com/api/v0/auth/token
Content-Type: application/x-www-form-urlencoded

grant_type=refresh_token
&refresh_token=<LETTERBOXD_REFRESH_TOKEN>
&client_id=<LETTERBOXD_CLIENT_ID>
&client_secret=<LETTERBOXD_CLIENT_SECRET>
```

Respuesta relevante:
```json
{
  "access_token": "...",
  "token_type": "Bearer",
  "expires_in": 3600,
  "refresh_token": "..."
}
```

---

## API de Letterboxd (`letterboxd.rs`)

Base URL: `https://api.letterboxd.com/api/v0`

### Obtener el member ID del usuario autenticado
```
GET /me
Authorization: Bearer <access_token>
```
Devuelve `{ "member": { "id": "<member_lid>", ... } }`. Guardar el LID para las siguientes llamadas.

### Obtener log entries (películas vistas, con o sin rating)
```
GET /log-entries?member=<member_lid>&perPage=100&cursor=<cursor>
Authorization: Bearer <access_token>
```

Iterar con cursor hasta agotar todas las páginas. Cada entrada tiene:
```json
{
  "film": {
    "id": "<film_lid>",
    "name": "...",
    "links": [{ "type": "tmdb", "id": "<tmdb_id>" }]
  },
  "rating": 4.5
}
```

Todas las entradas cuentan como "ya vistas" (se excluyen de las recomendaciones). Solo las que
tienen `rating >= min_rating` se usan como semillas.

### Obtener la watchlist del usuario
```
GET /members/<member_lid>/watchlist?perPage=100&cursor=<cursor>
Authorization: Bearer <access_token>
```

Paginado igual que log-entries. Las películas de la watchlist también se excluyen de las
recomendaciones (ya las conoces, aunque no las hayas visto).

---

## API de TMDB (`tmdb.rs`)

Base URL: `https://api.themoviedb.org/3`
Auth: header `Authorization: Bearer <TMDB_BEARER_TOKEN>`

### Obtener recomendaciones por película
```
GET /movie/<tmdb_id>/recommendations?language=es-ES&page=1
Authorization: Bearer <TMDB_BEARER_TOKEN>
```

Devuelve lista de películas similares con `id`, `title`, `vote_average`, `popularity`. Los
resultados se cachean en disco por `tmdb_id` (TTL 24h) para no repetir la misma consulta en
ejecuciones sucesivas.

---

## Lógica de recomendación (`recommend.rs`)

1. Cargar en paralelo los log entries y la watchlist del usuario.
2. Construir un set de `tmdb_id` "vistos o en watchlist" (para excluir después).
3. Tomar las películas con rating ≥ `min_rating` (en escala Letterboxd de 0.5 a 5.0) como semillas.
4. Para cada semilla, llamar a TMDB `/recommendations` (en paralelo, con límite de concurrencia y
   caché) → acumular películas candidatas y su frecuencia de aparición.
5. Excluir candidatas que estén en el set de vistas/watchlist.
6. Pre-seleccionar `count * 3` candidatas por `frecuencia × vote_average` de TMDB.
7. Para esa preselección, obtener el rating comunitario de Letterboxd (en paralelo) y recalcular
   el score final: `frecuencia × rating_LB` (o `frecuencia × vote_average/2` si Letterboxd no
   tiene el film).
8. Ordenar por score descendente y devolver las top N (configurable, por defecto 10).

La lógica pura (extracción de semillas/vistos, pre-scoring, score final) está separada de la
orquestación async y cubierta por tests unitarios (`#[cfg(test)]` en `recommend.rs`).

El progreso de estas etapas se reporta a través del trait `Progress` (`progress.rs`), implementado
con barras `indicatif` en la CLI y con un canal `tokio::mpsc` en la TUI — la lógica de negocio no
sabe ni le importa cuál de los dos consume los eventos.

---

## Comandos CLI

```
letterboxd-cli recommend [--count <N>] [--min-rating <R>] [--json]
letterboxd-cli tui [--count <N>] [--min-rating <R>]
letterboxd-cli keychain <import|clear>
```

### `recommend`

- `--count`: número de recomendaciones a mostrar (por defecto 10)
- `--min-rating`: rating mínimo propio para considerar una película como semilla (por defecto 4.0)
- `--json`: imprime las recomendaciones como JSON en stdout en lugar de texto formateado (para
  scripting; las barras de progreso van a stderr y no interfieren)

Salida esperada (stdout, modo texto):
```
🎬 Recomendaciones para <username>

 1. Título de la película ★ 4.29
 2. Otro título            ★ 3.88
...
```

### `tui`

Abre una interfaz interactiva de terminal (ratatui) que carga las recomendaciones en segundo plano
sin bloquear el redibujado. Atajos:

- `↑/↓` o `j/k`: mover selección
- `r`: (re)cargar recomendaciones con los parámetros actuales
- `+`/`-`: subir/bajar `min_rating` en 0.5
- `[`/`]`: bajar/subir `count` en 5
- `q` / `Esc`: salir

Cambiar `min_rating` o `count` no vuelve a consultar automáticamente — hay que pulsar `r`
(se muestra un aviso de "parámetros modificados" mientras tanto).

---

## Caché

- Token OAuth: `~/.config/letterboxd-cli/token.json` — se renueva automáticamente al expirar.
- Log entries: `~/.config/letterboxd-cli/log_entries.json` — TTL 1 hora.
- Watchlist: `~/.config/letterboxd-cli/watchlist.json` — TTL 1 hora.
- Recomendaciones TMDB por película: `~/.config/letterboxd-cli/tmdb_recs_cache.json` — TTL 24 horas.

---

## Notas de implementación

- `reqwest` en modo async con `tokio`.
- Todos los errores con `anyhow::Result` — no usar `unwrap` fuera de `main` (excepto en tests y en
  bloqueos de `Mutex`/aritmética de tiempo que no pueden fallar en la práctica).
- La paginación de Letterboxd usa cursor opaco: el campo `next` del response. Parar cuando `next`
  sea null.
- TMDB puede devolver películas sin `tmdb_id` en el link de Letterboxd (algunos títulos oscuros).
  Saltarlas silenciosamente.
- Las llamadas a TMDB (`/recommendations`) y a Letterboxd (`get_lb_rating`) se hacen en paralelo con
  `futures::stream::buffer_unordered`, limitando la concurrencia (8 y 6 respectivamente) en lugar de
  serializar con `sleep` — más rápido sin saturar las APIs.
- La TUI nunca debe usar directamente las barras `indicatif` (escriben ANSI fuera de la pantalla
  alternativa de ratatui); usa siempre el trait `Progress` para que la lógica de negocio sea
  agnóstica del frontend.

---

## Estado actual

- [x] Estructura del proyecto inicializada
- [x] Auth con refresh_token funcionando
- [x] Paginación de log entries completa
- [x] Integración TMDB
- [x] Lógica de scoring
- [x] CLI con clap
- [x] Caché de log entries
- [x] Exclusión de watchlist
- [x] Caché de recomendaciones TMDB
- [x] Llamadas concurrentes (TMDB / ratings de Letterboxd)
- [x] Interfaz TUI (ratatui)
- [x] Salida `--json`
- [x] Tests unitarios de la lógica de scoring
- [x] CI (fmt, clippy, test, build; matrix ubuntu/macos)
- [x] Credenciales desde el Keychain de macOS (`keychain import`/`clear`)
- [ ] Tests de integración de los clientes HTTP (mocking)
