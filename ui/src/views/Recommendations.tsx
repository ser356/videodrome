import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { BackButton } from '../components/BackButton'
import { FiltersDropdown } from '../components/FiltersDropdown'
import { HotkeyBar } from '../components/HotkeyBar'
import { MovieDetailModal } from '../components/MovieDetailModal'
import { SearchBox } from '../components/SearchBox'
import { Toast } from '../components/Toast'
import { TopNav } from '../components/TopNav'
import {
  getPreferences,
  getRecommendations,
  isTauri,
  tmdbPoster,
  type Recommendation,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Vista `View::Recs` de la TUI, adaptada al look "cabina de proyección"
 * definido en DESIGN.md.
 *
 * - Grid uniforme de posters. Bajo cada card: solo título + año.
 *   Nada de rating dot verde ni contadores decorativos.
 * - Filtros como una query mono editable ("rating ≥ 4.0 · top 20"),
 *   reforzando el vocabulario CLI. Hotkeys +/- y [/] siguen operando.
 * - Título de vista "Cartelera" (voz de curator, no consumer-generic).
 */
export function Recommendations() {
  const nav = useNavigate()
  const [count, setCount] = useState(20)
  const [minRating, setMinRating] = useState(4.0)
  const [items, setItems] = useState<Recommendation[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [stale, setStale] = useState(false)
  const [sel, setSel] = useState(0)
  const [detail, setDetail] = useState<Recommendation | null>(null)

  const fetchRecs = useCallback(() => {
    if (!isTauri()) {
      setError('Esta vista requiere la app de escritorio (Tauri).')
      return
    }
    setLoading(true)
    setError(null)
    setStale(false)
    getRecommendations(count, minRating)
      .then((list) => {
        setItems(list)
        setSel(0)
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false))
  }, [count, minRating])

  useEffect(() => {
    // Al montar, leemos las preferencias guardadas y disparamos la
    // primera búsqueda con esos valores. Si el user no ha tocado nunca
    // ajustes, `getPreferences` devuelve los defaults sensatos (4.0 /
    // 20). No pasamos por `fetchRecs` porque su closure aún vería los
    // valores hardcoded del `useState` antes de que React aplique los
    // setters — llamamos directo a `getRecommendations` con los valores
    // frescos.
    if (!isTauri()) {
      setError('Esta vista requiere la app de escritorio (Tauri).')
      return
    }
    let cancelled = false
    ;(async () => {
      let c = count
      let r = minRating
      try {
        const p = await getPreferences()
        c = p.default_count
        r = p.default_min_rating
      } catch {
        // preferencias corruptas o backend down: seguimos con defaults
      }
      if (cancelled) return
      setCount(c)
      setMinRating(r)
      setLoading(true)
      setError(null)
      setStale(false)
      try {
        const list = await getRecommendations(c, r)
        if (cancelled) return
        setItems(list)
        setSel(0)
      } catch (e) {
        if (!cancelled) setError(String(e))
      } finally {
        if (!cancelled) setLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const openTorrents = (rec: Recommendation) => {
    const y = rec.movie.release_date?.slice(0, 4)
    nav(
      `/torrents/tmdb/${rec.movie.id}?title=${encodeURIComponent(rec.movie.title)}${
        y ? `&year=${y}` : ''
      }`,
    )
  }

  const n = items?.length ?? 0
  const move = (delta: number) => {
    if (n === 0) return
    setSel((i) => (i + delta + n) % n)
  }

  const bumpRating = (d: number) => {
    setMinRating((r) => Math.min(5, Math.max(0.5, +(r + d).toFixed(1))))
    setStale(true)
  }
  const bumpCount = (d: number) => {
    setCount((c) => Math.max(5, c + d))
    setStale(true)
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: '', run: () => move(1) },
    { key: 'ArrowDown', hint: '', run: () => move(1) },
    { key: 'k', hint: 'Mover', run: () => move(-1) },
    { key: 'ArrowUp', hint: '', run: () => move(-1) },
    { key: 'ArrowRight', hint: '', run: () => move(1) },
    { key: 'ArrowLeft', hint: '', run: () => move(-1) },
    {
      key: 'Enter',
      hint: 'Detalle',
      run: () => items && items[sel] && setDetail(items[sel]),
    },
    {
      key: 't',
      hint: 'Torrents',
      run: () => items && items[sel] && openTorrents(items[sel]),
    },
    { key: 'r', hint: 'Recargar', run: () => fetchRecs() },
    { key: '+', hint: '', run: () => bumpRating(0.5) },
    { key: '-', hint: 'Rating', run: () => bumpRating(-0.5) },
    { key: ']', hint: '', run: () => bumpCount(5) },
    { key: '[', hint: 'Top', run: () => bumpCount(-5) },
    { key: 'Escape', hint: '', run: () => nav('/') },
  ]
  useHotkeys(hotkeys, [items, sel, count, minRating, fetchRecs], {
    enabled: detail === null,
  })

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        <BackButton onClick={() => nav('/')} />
        <SearchBox />
        <FiltersDropdown
          minRating={minRating}
          count={count}
          dirty={stale}
          onChange={(nr, nc) => {
            setMinRating(nr)
            setCount(nc)
            setStale(true)
          }}
        />
      </TopNav>

      <main className="mx-auto w-full max-w-[1400px] flex-1 px-8 py-8">
        <h1 className="mb-8 text-[22px] font-semibold text-ink">Cartelera</h1>

        {error && (
          <div className="rounded-md border border-danger/40 bg-danger/10 p-4 text-[14px] text-danger">
            {error}
          </div>
        )}

        {!error && !items && loading && <PosterSkeletonGrid />}

        {items && items.length === 0 && !loading && (
          <div className="rounded-lg border border-hairline bg-surface p-10 text-center">
            <p className="text-[16px] text-ink">Sin resultados.</p>
            <p className="mt-1 text-[13px] text-muted">
              Baja el rating mínimo o comprueba tu historial en Letterboxd.
            </p>
          </div>
        )}

        {items && items.length > 0 && (
          <ul className="grid grid-cols-2 gap-x-6 gap-y-10 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6">
            {items.map((rec, i) => (
              <MovieCard
                key={rec.movie.id}
                rec={rec}
                active={i === sel}
                onClick={() => setDetail(rec)}
                onMouseEnter={() => setSel(i)}
              />
            ))}
          </ul>
        )}
      </main>

      <Toast visible={stale && !loading}>
        <span className="text-muted">Hay cambios pendientes.</span>
        <span>
          Pulsa <span className="font-semibold text-ink">R</span> para
          recargar.
        </span>
      </Toast>

      <HotkeyBar hotkeys={hotkeys.filter((h) => h.hint)} />

      {detail && (
        <MovieDetailModal
          rec={detail}
          onClose={() => setDetail(null)}
          onOpenTorrents={(r) => {
            setDetail(null)
            openTorrents(r)
          }}
        />
      )}
    </div>
  )
}

// ─── MovieCard: solo poster + título + año ───

function MovieCard({
  rec,
  active,
  onClick,
  onMouseEnter,
}: {
  rec: Recommendation
  active: boolean
  onClick: () => void
  onMouseEnter: () => void
}) {
  const { movie } = rec
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
              className="h-full w-full object-cover"
              onError={(e) => {
                e.currentTarget.style.display = 'none'
              }}
            />
          ) : (
            <div className="flex h-full w-full items-center justify-center px-3 text-center text-[12px] text-dim">
              {movie.title}
            </div>
          )}
        </div>
        <div className="mt-3 flex items-baseline justify-between gap-2">
          <p className="truncate text-[13px] text-body">{movie.title}</p>
          <span className="shrink-0 text-[11px] text-muted">
            {year}
          </span>
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
