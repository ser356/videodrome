# letterboxd-cli

CLI en Rust para obtener recomendaciones de películas basadas en el historial y ratings de Letterboxd,
usando la API no oficial de Letterboxd y la API de TMDB.

---

## Credenciales

Todas las credenciales van en `.env` en la raíz del proyecto (nunca en el repositorio):

```env
LETTERBOXD_CLIENT_ID=<tu_client_id>
LETTERBOXD_CLIENT_SECRET=<tu_client_secret>
LETTERBOXD_REFRESH_TOKEN=<tu_refresh_token>
LETTERBOXD_USERNAME=<tu_username>
TMDB_API_KEY=<tu_tmdb_api_key>
```

El `.env` está en `.gitignore`. El binario lee las variables con `dotenvy`.

---

## Arquitectura

```
letterboxd-cli/
├── src/
│   ├── main.rs          # Punto de entrada, CLI (clap)
│   ├── auth.rs          # OAuth2: refresh_token → access_token, caché en disco
│   ├── letterboxd.rs    # Cliente HTTP para la API de Letterboxd
│   ├── tmdb.rs          # Cliente HTTP para la API de TMDB
│   ├── recommend.rs     # Lógica de recomendación
│   └── config.rs        # Carga de variables de entorno
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
Devuelve `{ "id": "<member_lid>", ... }`. Guardar el LID para las siguientes llamadas.

### Obtener log entries (películas vistas con rating)
```
GET /members/<member_lid>/log-entries
  ?perPage=100
  &cursor=<cursor>        # paginación
  &sort=WhenRated
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

Solo nos interesan entradas con `rating` presente. Filtrar las que no tienen rating.

---

## API de TMDB (`tmdb.rs`)

Base URL: `https://api.themoviedb.org/3`
Auth: query param `api_key=<TMDB_API_KEY>`

### Obtener recomendaciones por película
```
GET /movie/<tmdb_id>/recommendations?api_key=...&language=es-ES
```

Devuelve lista de películas similares con `id`, `title`, `vote_average`, `popularity`.

---

## Lógica de recomendación (`recommend.rs`)

1. Cargar todos los log entries del usuario → lista de `(tmdb_id, rating)`.
2. Construir un set de `tmdb_id` vistos (para excluir después).
3. Tomar las películas con rating ≥ 4.0 (en escala Letterboxd de 0.5 a 5.0).
4. Para cada una, llamar a TMDB `/recommendations` → acumular películas candidatas.
5. Excluir candidatas que estén en el set de vistas.
6. Puntuar candidatas: frecuencia de aparición × `vote_average` de TMDB.
7. Ordenar por puntuación descendente y devolver las top N (configurable, por defecto 10).

---

## Comandos CLI

```
letterboxd-cli recommend [--count <N>] [--min-rating <R>]
```

- `--count`: número de recomendaciones a mostrar (por defecto 10)
- `--min-rating`: rating mínimo propio para considerar una película como semilla (por defecto 4.0)

Salida esperada (stdout):
```
🎬 Recomendaciones para <username>

 1. Título de la película (TMDB: 8.2)
 2. Otro título (TMDB: 7.9)
...
```

---

## Caché

- Token OAuth: `~/.config/letterboxd-cli/token.json`
- Log entries: `~/.config/letterboxd-cli/log_entries.json` con TTL de 1 hora (evitar llamadas repetidas durante desarrollo)

---

## Notas de implementación

- Usar `reqwest` en modo async con `tokio`.
- Todos los errores con `anyhow::Result` — no usar `unwrap` fuera de `main`.
- La paginación de Letterboxd usa cursor opaco: el campo `next` del response. Parar cuando `next` sea null.
- TMDB puede devolver películas sin `tmdb_id` en el link de Letterboxd (algunos títulos oscuros). Saltarlas silenciosamente.
- Rate limiting: añadir `tokio::time::sleep(Duration::from_millis(100))` entre llamadas a TMDB para no saturar.

---

## Estado actual

- [ ] Estructura del proyecto inicializada
- [ ] Auth con refresh_token funcionando
- [ ] Paginación de log entries completa
- [ ] Integración TMDB
- [ ] Lógica de scoring
- [ ] CLI con clap
- [ ] Caché de log entries
