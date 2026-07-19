import { invoke } from '@tauri-apps/api/core'

/**
 * Type-safe wrappers around Tauri commands defined in src/gui.rs.
 * All commands validate the app is running under Tauri; on plain
 * `vite dev` outside Tauri (running the UI in Safari), invocations
 * throw and the UI shows a friendly error banner instead.
 */

export function isTauri(): boolean {
  return typeof (window as unknown as { __TAURI_INTERNALS__?: unknown })
    .__TAURI_INTERNALS__ !== 'undefined'
}

// -------- Types (mirror the Rust structs) --------

export interface Movie {
  id: number
  title: string
  vote_average: number
  popularity: number
  release_date: string | null
  poster_path: string | null
  /// Presente en hits de Cinemeta (fallback anti-caída de TMDB). Cuando
  /// `id === 0` este campo lleva el IMDb id y la GUI enruta por texto
  /// directo en vez de por TMDB.
  imdb_id?: string | null
  /** Discriminador movie/series. Default `movie` para compat con
   * caches viejos y consumidores que solo esperaban pelis. */
  kind?: 'movie' | 'series'
}

export interface Recommendation {
  movie: Movie
  score: number
  frequency: number
  lb_rating: number | null
}

export interface MovieView {
  id: number
  title: string
  original_title: string | null
  overview: string | null
  tagline: string | null
  poster_path: string | null
  backdrop_path: string | null
  release_date: string | null
  runtime: number | null
  vote_average: number
  genres: string[]
}

export interface Torrent {
  title: string
  magnet: string
  size_bytes: number
  seeders: number
  leechers: number
  quality: string | null
  source: string
  /** ISO 639-1 (`en`, `es`, `ru`…) o marcador (`multi`, `unknown`, `dub`, `orig`). */
  audio: string
  /** Cómo matchea contra la query: `movie` (pre-audit), `episode`,
   * `season_pack`, `series_pack`. La UI pinta un badge acorde
   * (E03 / Pack S01 / Serie completa). */
  match_kind: 'movie' | 'episode' | 'season_pack' | 'series_pack'
  /** Índice de fichero pre-resuelto por el provider (Torrentio.fileIdx).
   * Cuando está presente, el frontend lo pasa a startStreamHtml como
   * `fileHint` — el backend salta la heurística de parseo de nombres y
   * sirve el fichero exacto (crítico para packs con numeración de
   * anime u otras rarezas). `null` = el provider no lo resolvió;
   * backend cae al parser + fallback al mayor. */
  file_hint?: number | null
}

export interface TorrentSearchResult {
  title: string
  imdb_id: string | null
  original_language: string | null
  year: number | null
  /** Duración TMDB en minutos; se usa para calcular los segundos de
   * `--start-time` cuando el user acepta reanudar. `null` en búsquedas
   * directas (sin TMDB) o si TMDB no lo expone. */
  runtime_minutes: number | null
  results: Torrent[]
  /** Estado por provider (Fase 1b — observabilidad). Vacío en modos
   * legacy o cuando la respuesta viene 100 % de caché (Fase 4). */
  providers?: ProviderStatus[]
  /** Fecha de estreno TMDB (`YYYY-MM-DD`). Fase 4b: la vista Torrents
   * la usa para pintar "Estrenada en cines el X — sin releases
   * digitales todavía" cuando `results` está vacío y el estreno es
   * reciente / futuro. `null` en búsquedas directas o si TMDB no la
   * expone. */
  release_date?: string | null
}

/** Espejo de `torrents::ProviderStatus` del backend. La UI pinta una
 * línea discreta tipo `knaben ✓ 34 · apibay ✗ timeout · yts ✓ 5` para
 * que el usuario vea si la lista corta viene de filtros o de un
 * provider caído. `error` solo llega con `ok = false`. */
export interface ProviderStatus {
  name: string
  ok: boolean
  hits: number
  elapsed_ms: number
  error?: string | null
  retried: boolean
  /** `true` si el resultado se sirvió del caché en disco (Fase 4a).
   * La UI pinta `↺` en vez de `✓`. */
  from_cache?: boolean
}

export interface StreamInfo {
  id: number
  url: string
  file_name: string
}

export interface StreamStats {
  progress_bytes: number
  total_bytes: number
  live_peers: number
  down_mbps: number
  alive: boolean
}

export interface Subtitle {
  file_id: number
  language: string
  release: string
  downloads: number
  rating: number
  hearing_impaired: boolean
  /** Verificado por moderador de OpenSubtitles. La UI le pinta un
   * badge y sube en el orden dentro del idioma. */
  from_trusted: boolean
  file_name: string | null
}

// -------- Session --------

export const hasSession = () => invoke<boolean>('has_session')
export const getUsername = () => invoke<string>('get_username')
export const login = (username: string, password: string) =>
  invoke<string>('login', { username, password })
export const logout = () => invoke<void>('logout')

// -------- Recommendations --------

/** Página de recomendaciones para el scroll infinito de la view Recs. */
export interface RecsPage {
  items: Recommendation[]
  /** `true` si quedan más elementos: el frontend puede pedir la
   * siguiente página con `offset += limit`. */
  has_more: boolean
  /** Nº total de recs disponibles (post-filtro de dismissed). */
  total: number
}

/**
 * Sirve una página del pool cacheado de recomendaciones. El primer
 * hit (o cuando `forceRefresh = true` o cambia `minRating`) computa el
 * pool entero (RECS_POOL_SIZE en backend, ~200 items) y las siguientes
 * llamadas devuelven slices sin volver a pegar a TMDB/Letterboxd.
 */
export const getRecommendationsPage = (
  offset: number,
  limit: number,
  minRating: number,
  forceRefresh = false,
) =>
  invoke<RecsPage>('get_recommendations_page', {
    offset,
    limit,
    minRating,
    forceRefresh,
  })

export const getMovieView = (tmdbId: number) =>
  invoke<MovieView | null>('get_movie_view', { tmdbId })

/** Marca una película como "no sugerir". El servidor solo persiste el
 * dismissed store; el frontend elimina localmente y la próxima página
 * del scroll infinito la filtrará. */
export const dismissRecommendation = (
  tmdbId: number,
  title: string,
  posterPath: string | null,
) =>
  invoke<void>('dismiss_recommendation', {
    tmdbId,
    title,
    posterPath,
  })

export const undismissRecommendation = (tmdbId: number) =>
  invoke<void>('undismiss_recommendation', { tmdbId })

export interface DismissedEntry {
  id: number
  title: string
  poster_path: string | null
  dismissed_at: number
}

export const listDismissed = () =>
  invoke<DismissedEntry[]>('list_dismissed')

// -------- Torrents --------

export const searchTorrentsByTmdb = (
  tmdbId: number,
  fallbackTitle: string,
  fallbackYear: number | null,
) =>
  invoke<TorrentSearchResult>('search_torrents_by_tmdb', {
    tmdbId,
    fallbackTitle,
    fallbackYear,
  })

export const searchTorrentsDirect = (query: string) =>
  invoke<TorrentSearchResult>('search_torrents_direct', { query })

/** Hit de TMDB anotado con el número de torrents disponibles. El
 * backend NO filtra por torrent_count > 0, así que este campo puede
 * ser 0: la peli aparecerá en el catálogo y la vista de Torrents
 * mostrará un mensaje adecuado si no hay resultados. */
export interface MovieHit extends Movie {
  torrent_count: number
}

export const searchMoviesTmdb = (query: string) =>
  invoke<MovieHit[]>('search_movies_tmdb', { query })

// -------- Series --------

export interface SeriesSeasonSummary {
  season_number: number
  episode_count: number
  air_date: string | null
  name: string | null
  poster_path: string | null
}

export interface SeriesDetails {
  id: number
  name: string
  original_name: string | null
  imdb_id: string | null
  original_language: string | null
  overview: string | null
  first_air_date: string | null
  poster_path: string | null
  backdrop_path: string | null
  seasons: SeriesSeasonSummary[]
  number_of_seasons: number
  status: string | null
}

export interface SeriesEpisode {
  season_number: number
  episode_number: number
  name: string | null
  overview: string | null
  air_date: string | null
  still_path: string | null
  runtime: number | null
}

export const getSeriesView = (tmdbId: number) =>
  invoke<SeriesDetails | null>('get_series_view', { tmdbId })

export const getSeriesSeason = (tmdbId: number, season: number) =>
  invoke<SeriesEpisode[]>('get_series_season', { tmdbId, season })

/** Búsqueda de torrents para un episodio (o pack de temporada, si
 * `episode = null`) de una serie. Backend construye variantes de
 * título y consulta providers series-aware (EZTV, Torznab tvsearch,
 * knaben/apibay con SxxEyy). */
export const searchTorrentsSeries = (
  tmdbId: number,
  season: number,
  episode: number | null,
) =>
  invoke<TorrentSearchResult>('search_torrents_series', {
    tmdbId,
    season,
    episode,
  })

/** Info por-fichero de un torrent (para picker manual cuando la
 * heurística S+E no matchee — packs con numeración de anime, etc.). */
export interface TorrentFileInfo {
  file_id: number
  name: string
  size: number
  season: number | null
  episode: number | null
  is_video: boolean
}

export const listTorrentFiles = (magnet: string) =>
  invoke<TorrentFileInfo[]>('list_torrent_files', { magnet })

export const openMagnet = (magnet: string) =>
  invoke<void>('open_magnet', { magnet })

// -------- Streaming --------

export const startStreamWithSub = (
  magnet: string,
  subPath: string | null,
  resumeSeconds: number | null = null,
  season: number | null = null,
  episode: number | null = null,
  fileHint: number | null = null,
) =>
  invoke<StreamInfo>('start_stream_with_sub', {
    magnet,
    subPath,
    resumeSeconds,
    season,
    episode,
    fileHint,
  })

/** Arranca el stream en modo player HTML: solo librqbit + HTTP server,
 * no spawnea VLC. La URL devuelta apunta a `/video` (raw file); si
 * `direct_playable=true` el player la usa tal cual, si no consume
 * `/hls/playlist.m3u8` (véase `hlsUrl`).
 *
 * `season`/`episode`: cuando el magnet es un season pack de una
 * serie, seleccionan el fichero del episodio dentro del torrent
 * parseando nombres (§4 audit series). Ambos juntos o ninguno.
 * `fileHint`: cuando el provider ya resolvió el índice del fichero
 * (Torrentio.fileIdx), se pasa aquí y skipeamos el parseo. Tiene
 * prioridad sobre season/episode. */
export const startStreamHtml = (
  magnet: string,
  season: number | null = null,
  episode: number | null = null,
  fileHint: number | null = null,
) => invoke<StreamInfo>('start_stream_html', { magnet, season, episode, fileHint })

/** `true` si ffmpeg + ffprobe están en PATH. */
export const ffmpegAvailable = () => invoke<boolean>('ffmpeg_available')

// ── Client capabilities (audit §4) ────────────────────────────
//
// El backend necesita saber qué códecs sabe decodificar el WebView
// para decidir DIRECT vs COPY vs TRANSCODE en el pipeline HLS. Antes
// era una whitelist estática (`["h264","hevc"]`); ahora los reportamos
// desde el frontend usando `HTMLVideoElement.canPlayType()`, que es
// el único juez fiable en producción (WKWebView macOS decodifica HEVC
// por hardware pero WebView2 Windows sólo si el user tiene "HEVC Video
// Extensions" de la MS Store).

export interface ClientCapabilities {
  /** Tags cortos: "h264", "hevc", "hevc10", "av1", "vp9",
   *  "aac", "mp3", "ac3", "eac3", "opus", "flac". */
  codecs: string[]
}

export const setClientCapabilities = (caps: ClientCapabilities) =>
  invoke<void>('set_client_capabilities', { caps })

/** MIME representativo por tag. Se usa `hvc1` (Main) y también
 *  `hev1` (algunos WebViews solo aceptan una de las dos formas).
 *  `avc1.640028` = H.264 High L4.0 — la referencia universal.
 *  `mp4a.40.2` = AAC LC. */
const CODEC_PROBES: Record<string, string[]> = {
  h264: ['video/mp4; codecs="avc1.640028"'],
  hevc: [
    'video/mp4; codecs="hvc1.1.6.L123.B0"',
    'video/mp4; codecs="hev1.1.6.L123.B0"',
  ],
  hevc10: [
    'video/mp4; codecs="hvc1.2.4.L123.B0"',
    'video/mp4; codecs="hev1.2.4.L123.B0"',
  ],
  av1: ['video/mp4; codecs="av01.0.05M.08"'],
  vp9: ['video/webm; codecs="vp09.00.10.08"'],
  aac: ['audio/mp4; codecs="mp4a.40.2"'],
  mp3: ['audio/mpeg'],
  ac3: ['audio/mp4; codecs="ac-3"'],
  eac3: ['audio/mp4; codecs="ec-3"'],
  opus: ['audio/webm; codecs="opus"'],
  flac: ['audio/flac'],
}

/** Corre `canPlayType` para cada tag conocido y devuelve los que
 *  el WebView acepta como `"probably"` o `"maybe"`. `maybe` cuenta
 *  para todo menos HEVC/AV1 (Chromium suele decir `maybe` cuando
 *  en realidad falla → strict `probably` en esos dos). */
export function detectClientCapabilities(): ClientCapabilities {
  if (typeof document === 'undefined') return { codecs: [] }
  const probe = document.createElement('video')
  const audio = document.createElement('audio')
  const strict = new Set(['hevc', 'hevc10', 'av1'])
  const supported: string[] = []
  for (const [tag, mimes] of Object.entries(CODEC_PROBES)) {
    const el = mimes[0].startsWith('audio/') ? audio : probe
    const ok = mimes.some((m) => {
      const r = el.canPlayType(m)
      return strict.has(tag) ? r === 'probably' : r !== ''
    })
    if (ok) supported.push(tag)
  }
  return { codecs: supported }
}

// Info que devuelve `/probe.json` del servidor local. Es lo que
// ffprobe reporta sobre el input: contenedor, duración y streams.
// El player HTML lo usa para saber la duración (video.duration es
// Infinity con streams chunked) y para poblar los menús de audio/subs.
export interface MediaInfo {
  duration_seconds: number | null
  container: string | null
  streams: MediaStream[]
  /** `true` si el frontend puede apuntar `<video src>` a `/video` raw
   * (WKWebView/WebView2 decodifican directamente). Cuando es `false`
   * el player pasa por ffmpeg vía `/hls/playlist.m3u8`. */
  direct_playable: boolean
}

export interface MediaStream {
  index: number
  kind: 'video' | 'audio' | 'subtitle' | 'other'
  codec: string
  language: string | null
  title: string | null
  width: number | null
  height: number | null
}

/** Consulta el endpoint `/probe.json` del stream local. `streamUrl` es
 * la URL que devuelve `start_stream_html` (que apunta a `/video`);
 * probe se sirve desde el mismo host/port.
 *
 * Errores posibles:
 *   - `ProbeStalledError`: backend devolvió 504 + JSON
 *     `{reason:"probe_stalled", bytes, elapsed_s}` — ffprobe se
 *     rindió tras 20 s sin cabecera. Firma clara de "swarm sin
 *     seeders": el player pinta mensaje y botón "Volver a torrents
 *     del título" en vez del genérico "comprueba ffmpeg".
 *   - `Error` genérico: 500 (ffprobe/ffmpeg roto), 4xx, red caída.
 *     El player lo mapea al mensaje "ffmpeg missing" heurístico
 *     que existía antes. */
export class ProbeStalledError extends Error {
  readonly reason = 'probe_stalled'
  readonly bytes: number
  readonly elapsedS: number
  constructor(bytes: number, elapsedS: number) {
    super(`probe_stalled: 0 B in ${elapsedS}s`)
    this.name = 'ProbeStalledError'
    this.bytes = bytes
    this.elapsedS = elapsedS
  }
}

export async function probeStream(streamUrl: string): Promise<MediaInfo> {
  const base = streamUrl.replace(/\/video$/, '')
  const r = await fetch(`${base}/probe.json`)
  if (r.status === 504) {
    // Rama estructurada `probe_stalled` del backend. El body es JSON
    // con `bytes` (siempre 0 hoy) + `elapsed_s`. Si falla el parse
    // (raro — el backend controla el body), caemos al Error genérico
    // para no ocultar información al log de la consola.
    try {
      const body = (await r.json()) as {
        reason?: string
        bytes?: number
        elapsed_s?: number
      }
      if (body.reason === 'probe_stalled') {
        throw new ProbeStalledError(body.bytes ?? 0, body.elapsed_s ?? 0)
      }
    } catch (e) {
      if (e instanceof ProbeStalledError) throw e
      throw new Error(`probe 504 (body no parseable): ${e}`, { cause: e })
    }
  }
  if (!r.ok) throw new Error(`probe ${r.status}`)
  return (await r.json()) as MediaInfo
}

/** URL del playlist HLS. Es un playlist VOD ESTÁTICO — función pura
 * de la duración de la peli (enumera todos los segmentos desde
 * arranque con `#EXT-X-ENDLIST`). El backend materializa los `.ts`
 * bajo demanda cuando el `<video>` los pide.
 *
 * Sin query params: la URL es estable durante toda la vida del
 * stream, así que WKWebView puede cachear libremente. El seek al
 * offset inicial de resume se hace en el frontend con
 * `v.currentTime = t` tras `loadedmetadata`, no en la URL.
 */
export function hlsUrl(streamUrl: string): string {
  const base = streamUrl.replace(/\/video$/, '')
  return `${base}/hls/playlist.m3u8`
}

/** POST al backend para cambiar la pista de audio activa. El backend
 * mata el ffmpeg job actual y purga los `.ts` de la pista anterior;
 * el caller es responsable de reiniciar hls.js (o el `<video>`
 * nativo) para que vuelva a pedir la playlist desde cero. `idx` es
 * el ÍNDICE dentro del sub-array de audio de `MediaInfo.streams`
 * (0-based, orden ffprobe).
 *
 * Devuelve 204 sin body; error como excepción del fetch. */
export async function setAudioTrack(streamUrl: string, idx: number): Promise<void> {
  const base = streamUrl.replace(/\/video$/, '')
  const r = await fetch(`${base}/hls/audio?idx=${idx}`, { method: 'POST' })
  if (!r.ok) throw new Error(`set audio track ${idx}: HTTP ${r.status}`)
}

/** Descarga el track de subtítulos embebido `idx` del contenedor
 * como WebVTT text. `idx` es el sub-índice de `MediaInfo.streams`
 * filtrado por `kind === 'subtitle'`. Solo funciona con subs de
 * TEXTO (SRT/ASS/SSA); los bitmap (PGS/DVBSUB) devuelven 415 y el
 * caller debería ocultar la pista del panel. */
export async function fetchEmbeddedSubtitle(
  streamUrl: string,
  idx: number,
): Promise<string> {
  const base = streamUrl.replace(/\/video$/, '')
  const r = await fetch(`${base}/subs/embedded/${idx}`)
  if (r.status === 415) throw new Error('unsupported')
  if (!r.ok) throw new Error(`fetch embedded sub ${idx}: HTTP ${r.status}`)
  return r.text()
}

export const streamStats = (id: number) =>
  invoke<StreamStats>('stream_stats', { id })

export const stopStream = (id: number) => invoke<void>('stop_stream', { id })

/** Estado de resume persistido para un magnet, si su infohash tiene
 * caché. Puede llegar en dos formas:
 *
 *   * `seconds` + `duration_seconds` — reportado por el player HTML
 *     mientras reproducía. Preferido: no necesita `runtime_minutes`
 *     para convertir a tiempo (funciona en modo direct y en
 *     búsquedas sin TMDB).
 *   * `fraction` — byte-based, escrito por el Drop del stream cuando
 *     se cierra (path VLC y compat con caché legacy). El caller lo
 *     multiplica por `runtime_minutes × 60` para sacar segundos.
 *
 * Ambos campos pueden coexistir; el frontend prefiere `seconds`. */
export interface Resume {
  fraction: number
  seconds: number | null
  duration_seconds: number | null
  updated_at: number
  /** Metadata de episodio si el resume es de una serie. `null` para
   * pelis o entradas legacy. */
  season?: number | null
  episode?: number | null
}

export const getResume = (
  magnet: string,
  season: number | null = null,
  episode: number | null = null,
) => invoke<Resume | null>('get_resume', { magnet, season, episode })

/** Reporta la posición absoluta del `<video>` al backend para que la
 * persista en `resume.json`. Se invoca cada ~15s durante la
 * reproducción y en el cleanup del player. Si `seconds/duration_seconds`
 * supera el 95%, el backend borra el resume (peli terminada).
 *
 * `season`/`episode`/`tmdbId` (opcionales): metadata que se guarda
 * con la entrada para habilitar "continuar viendo" y "siguiente
 * episodio" (§6 audit). */
export const reportPosition = (
  streamId: number,
  seconds: number,
  durationSeconds: number,
  season: number | null = null,
  episode: number | null = null,
  tmdbId: number | null = null,
) =>
  invoke<void>('report_position', {
    streamId,
    seconds,
    durationSeconds,
    season,
    episode,
    tmdbId,
  })

// -------- Subtitles --------

export const subtitlesAvailable = () =>
  invoke<boolean>('subtitles_available')

export const searchSubtitles = (
  streamId: number | null,
  imdbId: string | null,
  query: string | null,
  languages?: string,
  season: number | null = null,
  episode: number | null = null,
) =>
  invoke<Subtitle[]>('search_subtitles', {
    streamId,
    imdbId,
    query,
    season,
    episode,
    languages,
  })

export const downloadSubtitle = (sub: Subtitle) =>
  invoke<string>('download_subtitle', { sub })

/** Convierte un `.srt` local a WebVTT (string). El player HTML lo
 * usa como `<track>` vía blob URL — WKWebView/WebView2 no cargan
 * `.srt` nativamente. */
export const subtitleToVtt = (path: string) =>
  invoke<string>('subtitle_to_vtt', { path })

// -------- Ajustes: caché + preferencias --------

export interface CacheEntry {
  kind:
    | 'log_entries'
    | 'watchlist'
    | 'tmdb_recs'
    | 'search'
    | 'torrent_search'
    | 'streams'
    | 'tmdb_search'
    | 'tmdb_view'
    | 'tmdb_details'
  label: string
  path: string
  exists: boolean
  size_bytes: number
  modified_at: number
}

export interface Preferences {
  default_min_rating: number
  subtitle_languages: string
  /** TTL de la caché de streams en días (auto-prune al arrancar). */
  stream_cache_ttl_days: number
  /** Opacidad del "liquid glass" (0..=100). 0 = default translúcido,
   * 100 = superficies casi sólidas para máxima legibilidad. */
  glass_opacity: number
  /** Reproductor por defecto: `html` = player embebido (requiere
   * ffmpeg en PATH), `vlc` = ruta legacy con VLC como proceso
   * externo. El clic derecho sobre un torrent siempre ofrece
   * "Abrir en VLC" como escape hatch. */
  default_player: 'html' | 'vlc'
  /** Idioma de la UI (ISO 639-1). `null` = auto-detección al
   * arrancar vía `navigator.language`; tras la primera detección se
   * persiste aquí. Se usa además como primer idioma al buscar
   * subtítulos (el UI lang sale arriba en OpenSubtitles). */
  ui_language: string | null
}

export const cacheInfo = () => invoke<CacheEntry[]>('cache_info')
export const clearCache = (kind: CacheEntry['kind'] | 'all') =>
  invoke<void>('clear_cache', { kind })
export const getPreferences = () => invoke<Preferences>('get_preferences')
export const setPreferences = (prefs: Preferences) =>
  invoke<void>('set_preferences', { prefs })

// -------- About / logs --------

/** Info de app + capa de logging, expuesta para la sección
 *  "Acerca de" de Ajustes. `enabled=false` cuando el user forzó
 *  `VIDEODROME_LOG=0`. El fichero puede no existir todavía si la
 *  app arrancó hace segundos y aún no ha flusheado ninguna línea. */
export interface AppLogInfo {
  version: string
  enabled: boolean
  dir: string | null
  file: string | null
  /** `true` cuando el user forzó `VIDEODROME_LOG=/ruta/x.log`. En ese
   * modo el prune diario y la rotación están desactivados. */
  explicit_path: boolean
}

export const logInfo = () => invoke<AppLogInfo>('log_info')
export const openLogFolder = () => invoke<void>('open_log_folder')

// -------- Helpers --------

export function tmdbPoster(
  path: string | null,
  size: 'w342' | 'w500' | 'w780' = 'w500',
): string | null {
  if (!path) return null
  // Fallback de Cinemeta: viene una URL absoluta ya lista para usar
  // (imágenes de metahub, etc.). No prependemos el CDN de TMDB.
  if (/^https?:\/\//i.test(path)) return path
  return `https://image.tmdb.org/t/p/${size}${path}`
}

export function tmdbBackdrop(
  path: string | null,
  size: 'w780' | 'w1280' | 'original' = 'w1280',
): string | null {
  if (!path) return null
  if (/^https?:\/\//i.test(path)) return path
  return `https://image.tmdb.org/t/p/${size}${path}`
}

export function formatSize(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let n = bytes
  let i = 0
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024
    i++
  }
  return `${n.toFixed(n < 10 ? 2 : 1)} ${units[i]}`
}

/** Emoji bandera para un código ISO 639-1 de audio o marcador especial. */
export function audioFlag(audio: string): { flag: string; label: string } {
  const map: Record<string, { flag: string; label: string }> = {
    en: { flag: '🇬🇧', label: 'EN' },
    es: { flag: '🇪🇸', label: 'ES' },
    ru: { flag: '🇷🇺', label: 'RU' },
    fr: { flag: '🇫🇷', label: 'FR' },
    it: { flag: '🇮🇹', label: 'IT' },
    de: { flag: '🇩🇪', label: 'DE' },
    ja: { flag: '🇯🇵', label: 'JA' },
    ko: { flag: '🇰🇷', label: 'KO' },
    pt: { flag: '🇵🇹', label: 'PT' },
    zh: { flag: '🇨🇳', label: 'ZH' },
    multi: { flag: '🌐', label: 'MULTI' },
    orig: { flag: '🎬', label: 'ORIG' },
    dub: { flag: '💬', label: 'DUB' },
    unknown: { flag: '·', label: '?' },
  }
  return map[audio] ?? { flag: '·', label: audio.toUpperCase() }
}
