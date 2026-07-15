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
}

export interface TorrentSearchResult {
  title: string
  imdb_id: string | null
  original_language: string | null
  year: number | null
  results: Torrent[]
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
  file_name: string | null
}

// -------- Session --------

export const hasSession = () => invoke<boolean>('has_session')
export const getUsername = () => invoke<string>('get_username')
export const login = (username: string, password: string) =>
  invoke<string>('login', { username, password })
export const logout = () => invoke<void>('logout')

// -------- Recommendations --------

export const getRecommendations = (count: number, minRating: number) =>
  invoke<Recommendation[]>('get_recommendations', { count, minRating })

export const getMovieView = (tmdbId: number) =>
  invoke<MovieView | null>('get_movie_view', { tmdbId })

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

/** Hit de TMDB anotado con el número de torrents disponibles. Los hits
 * sin torrents se filtran en el backend, así que aquí todos tienen
 * `torrent_count >= 1`. */
export interface MovieHit extends Movie {
  torrent_count: number
}

export const searchMoviesTmdb = (query: string) =>
  invoke<MovieHit[]>('search_movies_tmdb', { query })

export const openMagnet = (magnet: string) =>
  invoke<void>('open_magnet', { magnet })

// -------- Streaming --------

export const startStreamWithSub = (magnet: string, subPath: string | null) =>
  invoke<StreamInfo>('start_stream_with_sub', { magnet, subPath })

export const streamStats = (id: number) =>
  invoke<StreamStats>('stream_stats', { id })

export const stopStream = (id: number) => invoke<void>('stop_stream', { id })

// -------- Subtitles --------

export const subtitlesAvailable = () =>
  invoke<boolean>('subtitles_available')

export const searchSubtitles = (
  imdbId: string | null,
  query: string | null,
  languages?: string,
) =>
  invoke<Subtitle[]>('search_subtitles', {
    imdbId,
    query,
    languages,
  })

export const downloadSubtitle = (sub: Subtitle) =>
  invoke<string>('download_subtitle', { sub, streamId: null })

// -------- Ajustes: caché + preferencias --------

export interface CacheEntry {
  kind: 'log_entries' | 'watchlist' | 'tmdb_recs' | 'search'
  label: string
  path: string
  exists: boolean
  size_bytes: number
  modified_at: number
}

export interface Preferences {
  default_min_rating: number
  default_count: number
  subtitle_languages: string
}

export const cacheInfo = () => invoke<CacheEntry[]>('cache_info')
export const clearCache = (kind: CacheEntry['kind'] | 'all') =>
  invoke<void>('clear_cache', { kind })
export const getPreferences = () => invoke<Preferences>('get_preferences')
export const setPreferences = (prefs: Preferences) =>
  invoke<void>('set_preferences', { prefs })

// -------- Helpers --------

export function tmdbPoster(
  path: string | null,
  size: 'w342' | 'w500' | 'w780' = 'w500',
): string | null {
  return path ? `https://image.tmdb.org/t/p/${size}${path}` : null
}

export function tmdbBackdrop(
  path: string | null,
  size: 'w780' | 'w1280' | 'original' = 'w1280',
): string | null {
  return path ? `https://image.tmdb.org/t/p/${size}${path}` : null
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
