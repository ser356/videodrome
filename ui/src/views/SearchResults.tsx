import { useCallback, useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { HotkeyBar } from '../components/HotkeyBar'
import { TopNav } from '../components/TopNav'
import {
  isTauri,
  searchMoviesTmdb,
  tmdbPoster,
  type MovieHit,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'
import { useT } from '../lib/i18n'

/**
 * Pantalla intermedia entre `Search` y `Torrents`. Muestra los posibles
 * hits de TMDB para la query. El user pincha (o Enter) sobre la carátula
 * de la película correcta y saltamos a `/torrents/tmdb/:id`, que ya sabe
 * enriquecer la búsqueda con imdb/idioma/año.
 *
 * Motivación: una query imprecisa ("hollow man") antes iba directa a los
 * providers y devolvía basura mezclada de otras pelis. Ahora TMDB actúa
 * como disambiguator visual.
 */
export function SearchResults() {
  const nav = useNavigate()
  const [params] = useSearchParams()
  const q = params.get('q') ?? ''
  const t = useT()

  const [items, setItems] = useState<MovieHit[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [sel, setSel] = useState(0)

  const runSearch = useCallback(() => {
    if (!isTauri()) {
      setError('Esta vista requiere la app de escritorio (Tauri).')
      return
    }
    if (!q.trim()) {
      setItems([])
      return
    }
    setLoading(true)
    setError(null)
    setItems(null)
    setSel(0)
    searchMoviesTmdb(q)
      .then(setItems)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false))
  }, [q])

  useEffect(() => {
    runSearch()
  }, [runSearch])

  const openTorrents = (m: MovieHit) => {
    const y = m.release_date?.slice(0, 4)
    // Serie: la ruta correcta es /series/:id (SeriesDetail), donde
    // el user elige temporada/episodio antes de ver torrents.
    if (m.kind === 'series' && m.id) {
      nav(`/series/${m.id}?title=${encodeURIComponent(m.title)}`)
      return
    }
    // Fallback de Cinemeta: no hay TMDB id → no podemos usar la ruta
    // `/torrents/tmdb/:id` (dispararía `get_movie_details(0)`). Vamos a
    // búsqueda directa por título+año, que no depende de TMDB.
    if (!m.id) {
      const q = y ? `${m.title} ${y}` : m.title
      nav(`/torrents/search?q=${encodeURIComponent(q)}`)
      return
    }
    nav(
      `/torrents/tmdb/${m.id}?title=${encodeURIComponent(m.title)}${
        y ? `&year=${y}` : ''
      }`,
    )
  }

  const n = items?.length ?? 0
  const move = (delta: number) => {
    if (n === 0) return
    setSel((i) => (i + delta + n) % n)
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: '', run: () => move(1) },
    { key: 'ArrowDown', hint: '', run: () => move(1) },
    { key: 'k', hint: t('hotkey.move'), run: () => move(-1) },
    { key: 'ArrowUp', hint: '', run: () => move(-1) },
    { key: 'ArrowRight', hint: '', run: () => move(1) },
    { key: 'ArrowLeft', hint: '', run: () => move(-1) },
    {
      key: 'Enter',
      hint: t('hotkey.torrents'),
      run: () => items && items[sel] && openTorrents(items[sel]),
    },
    { key: 'Escape', hint: t('common.back'), run: () => nav('/search') },
  ]
  useHotkeys(hotkeys, [items, sel])

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        <button
          onClick={() => nav('/search')}
          className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-body hover:border-border-strong"
        >
          {t('common.back')}
        </button>
      </TopNav>

      <main className="mx-auto w-full max-w-[1400px] flex-1 px-8 py-8">
        <div className="mb-6 flex items-baseline justify-between">
          <h1 className="text-[22px] font-semibold text-ink">
            {t('searchResults.title')}{' '}
            <span className="text-muted">· {q}</span>
          </h1>
          <p className="text-[12px] tabular-nums text-dim">
            {loading
              ? t('searchResults.searching')
              : items
                ? t('searchResults.matches', { n: items.length })
                : ''}
          </p>
        </div>

        {error && (
          <div className="rounded-md border border-danger/40 bg-danger/10 p-4 text-[14px] text-danger">
            {error}
          </div>
        )}

        {!error && !items && loading && <PosterSkeletonGrid />}

        {items && items.length === 0 && !loading && (
          <div className="rounded-lg border border-hairline bg-surface p-10 text-center">
            <p className="text-[16px] text-ink">{t('searchResults.emptyTitle')}</p>
            <p className="mt-1 text-[13px] text-muted">
              {t('searchResults.emptyHint')}
            </p>
          </div>
        )}

        {items && items.length > 0 && (
          <ul className="grid grid-cols-2 gap-x-6 gap-y-10 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6">
            {items.map((m, i) => (
              <MovieCard
                key={m.id}
                movie={m}
                active={i === sel}
                onClick={() => openTorrents(m)}
                onMouseEnter={() => setSel(i)}
              />
            ))}
          </ul>
        )}
      </main>

      <HotkeyBar hotkeys={hotkeys.filter((h) => h.hint)} />
    </div>
  )
}

function MovieCard({
  movie,
  active,
  onClick,
  onMouseEnter,
}: {
  movie: MovieHit
  active: boolean
  onClick: () => void
  onMouseEnter: () => void
}) {
  const t = useT()
  const year = movie.release_date?.slice(0, 4) ?? ''
  const src = tmdbPoster(movie.poster_path)

  return (
    <li>
      <button
        onClick={onClick}
        onMouseEnter={onMouseEnter}
        className="focus-ring group block w-full rounded-poster text-left"
      >
        <div
          className={`poster-hover relative aspect-[2/3] w-full overflow-hidden rounded-poster bg-surface-hi transition-shadow ${
            active
              ? 'outline outline-1 outline-white/40 outline-offset-2'
              : ''
          }`}
        >
          {src ? (
            <img
              src={src}
              alt={`Poster de ${movie.title}`}
              loading="lazy"
              draggable={false}
              className="pointer-events-none h-full w-full select-none object-cover"
              onError={(e) => {
                e.currentTarget.style.display = 'none'
              }}
            />
          ) : (
            <div className="pointer-events-none flex h-full w-full items-center justify-center px-3 text-center text-[12px] text-dim">
              {movie.title}
            </div>
          )}
          <span className="absolute right-2 top-2 rounded-full border border-accent/40 bg-canvas/80 px-2 py-0.5 text-[10px] font-semibold text-accent backdrop-blur-sm">
            {movie.kind === 'series' ? t('searchResults.badgeSeries') : `▾ ${movie.torrent_count}`}
          </span>
        </div>
        <div className="mt-3 flex items-baseline justify-between gap-2">
          <p className="truncate text-[13px] text-body">{movie.title}</p>
          <span className="shrink-0 text-[11px] text-muted">{year}</span>
        </div>
      </button>
    </li>
  )
}

function PosterSkeletonGrid() {
  return (
    <ul className="grid grid-cols-2 gap-x-6 gap-y-10 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6">
      {Array.from({ length: 12 }).map((_, i) => (
        <li key={i}>
          <div className="aspect-[2/3] w-full animate-pulse rounded-poster bg-surface-hi" />
          <div className="mt-3 h-3 w-3/4 animate-pulse rounded-sm bg-surface-hi" />
        </li>
      ))}
    </ul>
  )
}
