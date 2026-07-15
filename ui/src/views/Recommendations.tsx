import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { BackButton } from '../components/BackButton'
import { ContextMenu, type ContextMenuItem } from '../components/ContextMenu'
import { FiltersDropdown } from '../components/FiltersDropdown'
import { HotkeyBar } from '../components/HotkeyBar'
import { MovieDetailModal } from '../components/MovieDetailModal'
import { SearchBox } from '../components/SearchBox'
import { Toast } from '../components/Toast'
import { TopNav } from '../components/TopNav'
import {
  dismissRecommendation,
  getPreferences,
  getRecommendations,
  isTauri,
  tmdbPoster,
  type Recommendation,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Vista `View::Recs` de la TUI, adaptada al look "cabina de proyección".
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
  // ID temporalmente marcado con la animación de fade-out mientras el
  // backend refresca la lista. Solo hay uno a la vez (el user hace
  // dismiss sobre una card).
  const [dismissingId, setDismissingId] = useState<number | null>(null)
  // Menú contextual (click derecho) sobre una card.
  const [menu, setMenu] = useState<{
    x: number
    y: number
    rec: Recommendation
  } | null>(null)
  // Toast efímero para confirmar el "no sugerir" con opción de
  // restaurar desde Ajustes. Sin este feedback el user queda con la
  // duda de si el descarte se guardó de verdad.
  const [flashMsg, setFlashMsg] = useState<string | null>(null)

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

  /**
   * "No sugerir" reactivo con dos fases desacopladas para que la card
   * desaparezca YA aunque el backend tarde varios segundos en volver
   * (la recomputación pega a TMDB + Letterboxd si la caché no ayuda):
   *
   *  1. Animación de fade-out (220 ms) → tras ella, quitamos la card
   *     localmente. Aquí el user ya ve el efecto inmediato.
   *  2. En paralelo, backend recalcula la lista con el reemplazo al
   *     final. Cuando responde, sustituimos `items` completo → el
   *     nuevo poster entra con `animate-card-in` (key nueva).
   */
  const dismissCurrent = async (rec: Recommendation) => {
    if (dismissingId !== null) return
    setDismissingId(rec.movie.id)

    // Fire-and-forget: el backend refresca en background. No bloquea
    // la eliminación visual.
    const refresh = dismissRecommendation(
      rec.movie.id,
      rec.movie.title,
      rec.movie.poster_path,
      count,
      minRating,
    ).catch((e) => {
      setFlashMsg(`Error al descartar: ${String(e)}`)
      return null
    })

    // Fase 1: espera solo la animación, luego elimina localmente.
    await new Promise((r) => setTimeout(r, 220))
    setDismissingId(null)
    setItems((prev) =>
      prev ? prev.filter((r) => r.movie.id !== rec.movie.id) : prev,
    )
    setSel((i) => Math.max(0, Math.min(i, (items?.length ?? 1) - 2)))
    setFlashMsg(
      `Descartada: ${rec.movie.title}. Restaurar desde Ajustes.`,
    )

    // Fase 2: cuando el backend termine, sustituimos la lista completa
    // para meter el reemplazo backfill al final. Si no hay respuesta
    // (error), nos quedamos con la lista de `count - 1` — el user
    // puede pulsar R para recargar.
    const freshList = await refresh
    if (freshList) setItems(freshList)
  }

  // Auto-hide del toast de dismiss.
  useEffect(() => {
    if (!flashMsg) return
    const t = setTimeout(() => setFlashMsg(null), 3200)
    return () => clearTimeout(t)
  }, [flashMsg])

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
    enabled: detail === null && menu === null,
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
                dismissing={rec.movie.id === dismissingId}
                onClick={() => setDetail(rec)}
                onMouseEnter={() => setSel(i)}
                onContextMenu={(x, y) => {
                  setSel(i)
                  setMenu({ x, y, rec })
                }}
              />
            ))}
          </ul>
        )}
      </main>

      <Toast visible={stale && !loading && flashMsg === null}>
        <span className="text-muted">Hay cambios pendientes.</span>
        <span>
          Pulsa <span className="font-semibold text-ink">R</span> para
          recargar.
        </span>
      </Toast>

      <Toast visible={flashMsg !== null}>
        <span className="text-body">{flashMsg}</span>
      </Toast>

      <HotkeyBar hotkeys={hotkeys.filter((h) => h.hint)} />

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={((): ContextMenuItem[] => {
            const rec = menu.rec
            return [
              {
                label: 'Ver detalle',
                hint: '↵',
                onClick: () => setDetail(rec),
              },
              {
                label: 'Ver torrents',
                hint: 't',
                onClick: () => openTorrents(rec),
              },
              {
                label: 'No sugerir',
                destructive: true,
                onClick: () => {
                  void dismissCurrent(rec)
                },
              },
            ]
          })()}
        />
      )}

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
  dismissing,
  onClick,
  onMouseEnter,
  onContextMenu,
}: {
  rec: Recommendation
  active: boolean
  dismissing: boolean
  onClick: () => void
  onMouseEnter: () => void
  onContextMenu: (x: number, y: number) => void
}) {
  const { movie } = rec
  const year = movie.release_date?.slice(0, 4) ?? ''
  const src = tmdbPoster(movie.poster_path)

  return (
    <li className={dismissing ? 'animate-dismiss' : 'animate-card-in'}>
      <button
        onClick={onClick}
        onMouseEnter={onMouseEnter}
        onContextMenu={(e) => {
          e.preventDefault()
          onContextMenu(e.clientX, e.clientY)
        }}
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
