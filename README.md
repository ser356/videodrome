# letterboxd-cli

CLI en Rust que genera recomendaciones de películas a partir de tu historial y ratings en Letterboxd, cruzando con la API de TMDB.

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

Crea un fichero `.env` en la raíz del proyecto (nunca lo subas al repo):

```env
LETTERBOXD_CLIENT_ID=<tu_client_id>
LETTERBOXD_CLIENT_SECRET=<tu_client_secret>
LETTERBOXD_REFRESH_TOKEN=<tu_refresh_token>
LETTERBOXD_USERNAME=<tu_username>
TMDB_API_KEY=<tu_tmdb_api_key>
TMDB_BEARER_TOKEN=<tu_tmdb_bearer_token>
```

El `.env` se lee desde el directorio de trabajo donde ejecutes el comando.

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

Ejemplos:

```bash
# Top 10 con la config por defecto
letterboxd-cli recommend

# Top 20 incluyendo películas con rating >= 3.5
letterboxd-cli recommend --count 20 --min-rating 3.5
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

---

## Caché

Para evitar llamadas repetidas a la API durante desarrollo:

- Historial de Letterboxd: `~/.config/letterboxd-cli/log_entries.json` — TTL 1 hora
- Token OAuth: `~/.config/letterboxd-cli/token.json` — se renueva automáticamente al expirar

---

## Compilar sin instalar

```bash
cargo build --release
./target/release/letterboxd-cli recommend
```

---

## Estado del CLAUDE.md

El fichero [CLAUDE.md](CLAUDE.md) contiene la especificación técnica del proyecto y sirve como referencia para el desarrollo.
