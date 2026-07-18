import { useCallback, useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { BackButton } from '../components/BackButton'
import { ContextMenu, type ContextMenuItem } from '../components/ContextMenu'
import { HotkeyBar } from '../components/HotkeyBar'
import { MovieDetailModal } from '../components/MovieDetailModal'
import { SearchBox } from '../components/SearchBox'
import { Toast } from '../components/Toast'
import { TopNav } from '../components/TopNav'
import {
  dismissRecommendation,
  getMovieView,
  getPreferences,
  getRecommendationsPage,
  isTauri,
  tmdbPoster,
  type Recommendation,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'
import { useT } from '../lib/i18n'

/**
 * Vista `View::Recs` de la TUI adaptada al look "cabina de proyección".
 *
 * Antes había un dropdown de filtros (rating mínimo + top N) que el user
 * ajustaba y pulsaba R para recargar. Ahora es scroll infinito: el
 * backend computa un pool grande (~200 items) la primera vez y sirve
 * páginas de 24 sobre él; al llegar al final de la lista un
 * `IntersectionObserver` dispara la siguiente. Sin ceiling artificial,
 * sin cognitive load — el "top" lo define el propio scroll.
 *
 * `min_rating` sigue existiendo pero como preferencia de sesión
 * (Ajustes → Rating mínimo por defecto). No tiene sentido exponerlo en
 * la vista si cambiarlo invalida todo el pool: es una decisión de
 * calidad de recomendaciones, no un dial de curación.
 */
export function Recommendations() {
  const nav = useNavigate()
  const t = useT()
  const [minRating, setMinRating] = useState<number | null>(null)
  const [items, setItems] = useState<Recommendation[] | null>(null)
  const [hasMore, setHasMore] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [sel, setSel] = useState(0)
  const [detail, setDetail] = useState<Recommendation | null>(null)
  const [dismissingId, setDismissingId] = useState<number | null>(null)
  const [menu, setMenu] = useState<{
    x: number
    y: number
    rec: Recommendation
  } | null>(null)
  const [flashMsg, setFlashMsg] = useState<string | null>(null)

  /** Tamaño de página del scroll infinito. 12 = 2 filas de 6 en
   * desktop; el backend LB-enriquece exactamente estos items en la
   * primera carga (~200-500ms con TMDB caché caliente). Batches
   * siguientes son también de 12 según scroll. */
  const PAGE_SIZE = 12

  const fetchPage = useCallback(
    async (offset: number, rating: number, force = false) => {
      const isFirst = offset === 0
      if (isFirst) {
        setLoading(true)
        setError(null)
      } else {
        setLoadingMore(true)
      }
      try {
        const page = await getRecommendationsPage(offset, PAGE_SIZE, rating, force)
        setItems((prev) => {
          if (isFirst || prev === null) return page.items
          // Dedup: si el user descartó pelis mientras cargábamos, el
          // backend puede devolver items que ya no están en `prev`;
          // los concatenamos sin repetir.
          const seen = new Set(prev.map((r) => r.movie.id))
          const merged = prev.slice()
          for (const r of page.items) {
            if (!seen.has(r.movie.id)) merged.push(r)
          }
          return merged
        })
        setHasMore(page.has_more)
        if (isFirst) setSel(0)
      } catch (e) {
        setError(String(e))
      } finally {
        setLoading(false)
        setLoadingMore(false)
      }
    },
    [],
  )

  // Boot: leer minRating de Preferences y disparar primera página.
  useEffect(() => {
    if (!isTauri()) {
      setError(t('series.tauriRequired'))
      return
    }
    let cancelled = false
    ;(async () => {
      let r = 4.0
      try {
        const p = await getPreferences()
        r = p.default_min_rating
      } catch {
        // preferencias corruptas / backend down: seguimos con default.
      }
      if (cancelled) return
      setMinRating(r)
      await fetchPage(0, r, false)
    })()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const refresh = () => {
    if (minRating == null) return
    void fetchPage(0, minRating, true)
  }

  // IntersectionObserver sobre un sentinel al final del grid. Cuando
  // el sentinel entra en viewport y todavía queda pool, disparamos la
  // siguiente página. El `rootMargin` de 400px pre-carga un poco antes
  // de que el user llegue al final visualmente — evita el "salto" a
  // spinner en scroll rápido.
  const sentinelRef = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    const el = sentinelRef.current
    if (!el || minRating == null) return
    const obs = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (
            entry.isIntersecting &&
            hasMore &&
            !loading &&
            !loadingMore &&
            items !== null
          ) {
            void fetchPage(items.length, minRating, false)
          }
        }
      },
      { rootMargin: '400px' },
    )
    obs.observe(el)
    return () => obs.disconnect()
  }, [hasMore, loading, loadingMore, items, minRating, fetchPage])

  const openTorrents = (rec: Recommendation) => {
    const y = rec.movie.release_date?.slice(0, 4)
    nav(
      `/torrents/tmdb/${rec.movie.id}?title=${encodeURIComponent(rec.movie.title)}${
        y ? `&year=${y}` : ''
      }`,
    )
  }

  // Hover-preload de detalles: al pasar el ratón sobre una card
  // disparamos `getMovieView` en background. El backend cachea la
  // respuesta 7d en disco, así que cuando el user haga click el modal
  // se abre con la sinopsis lista sin espera. Set anti-duplicados
  // por id — evita machacar TMDB si el user pasea el cursor por el
  // grid varias veces.
  const preloadedRef = useRef<Set<number>>(new Set())
  const preloadDetail = useCallback((tmdbId: number) => {
    if (preloadedRef.current.has(tmdbId)) return
    preloadedRef.current.add(tmdbId)
    void getMovieView(tmdbId).catch(() => {
      // Fallo → permitimos reintentar en el próximo hover.
      preloadedRef.current.delete(tmdbId)
    })
  }, [])

  /**
   * "No sugerir" reactivo con fade-out + eliminación local.
   *
   * Antes el backend re-computaba la lista entera para meter un
   * reemplazo al final; con el pool cacheado y scroll infinito ya no
   * hace falta — el user simplemente ve una peli menos y las
   * siguientes páginas ya llegarán filtradas.
   */
  const dismissCurrent = async (rec: Recommendation) => {
    if (dismissingId !== null) return
    setDismissingId(rec.movie.id)

    // Fire-and-forget al backend. Si falla, avisamos pero no
    // deshacemos el fade — el usuario ya vio el efecto.
    void dismissRecommendation(
      rec.movie.id,
      rec.movie.title,
      rec.movie.poster_path,
    ).catch((e) => {
      setFlashMsg(t('recs.dismissError', { err: String(e) }))
    })

    // Fade-out (220ms) → eliminar localmente.
    await new Promise((r) => setTimeout(r, 220))
    setDismissingId(null)
    setItems((prev) =>
      prev ? prev.filter((r) => r.movie.id !== rec.movie.id) : prev,
    )
    setSel((i) => Math.max(0, Math.min(i, (items?.length ?? 1) - 2)))
    setFlashMsg(
      t('recs.dismissedFlash', { title: rec.movie.title }),
    )
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

  // Preload detail para el item seleccionado con teclado (j/k, ↑/↓).
  // Debounce 150ms para no saturar TMDB con scroll rápido.
  useEffect(() => {
    if (!items || items[sel] == null) return
    const id = items[sel].movie.id
    const timer = window.setTimeout(() => preloadDetail(id), 150)
    return () => window.clearTimeout(timer)
  }, [sel, items, preloadDetail])

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: '', run: () => move(1) },
    { key: 'ArrowDown', hint: '', run: () => move(1) },
    { key: 'k', hint: t('hotkey.move'), run: () => move(-1) },
    { key: 'ArrowUp', hint: '', run: () => move(-1) },
    { key: 'ArrowRight', hint: '', run: () => move(1) },
    { key: 'ArrowLeft', hint: '', run: () => move(-1) },
    {
      key: 'Enter',
      hint: t('recs.detail'),
      run: () => items && items[sel] && setDetail(items[sel]),
    },
    {
      key: 't',
      hint: t('hotkey.torrents'),
      run: () => items && items[sel] && openTorrents(items[sel]),
    },
    { key: 'r', hint: t('recs.reload'), run: refresh },
    { key: 'Escape', hint: '', run: () => nav('/') },
  ]
  useHotkeys(hotkeys, [items, sel, minRating], {
    enabled: detail === null && menu === null,
  })

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        <BackButton onClick={() => nav('/')} />
        <SearchBox />
      </TopNav>

      <main className="mx-auto w-full max-w-[1400px] flex-1 px-8 py-8">
        <h1 className="mb-8 text-[22px] font-semibold text-ink">{t('recs.title')}</h1>

        {error && (
          <div className="rounded-md border border-danger/40 bg-danger/10 p-4 text-[14px] text-danger">
            {error}
          </div>
        )}

        {!error && !items && loading && <PosterSkeletonGrid />}

        {items && items.length === 0 && !loading && (
          <div className="rounded-lg border border-hairline bg-surface p-10 text-center">
            <p className="text-[16px] text-ink">{t('recs.emptyTitle')}</p>
            <p className="mt-1 text-[13px] text-muted">
              {t('recs.emptyHint')}
            </p>
          </div>
        )}

        {items && items.length > 0 && (
          <>
            <ul className="grid grid-cols-2 gap-x-6 gap-y-10 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6">
              {items.map((rec, i) => (
                <MovieCard
                  key={rec.movie.id}
                  rec={rec}
                  active={i === sel}
                  dismissing={rec.movie.id === dismissingId}
                  onClick={() => setDetail(rec)}
                  onMouseEnter={() => {
                    setSel(i)
                    preloadDetail(rec.movie.id)
                  }}
                  onContextMenu={(x, y) => {
                    setSel(i)
                    setMenu({ x, y, rec })
                  }}
                />
              ))}
            </ul>

            {/* Sentinel para el IntersectionObserver + spinner de la
                siguiente página. Solo se pinta si aún queda pool o si
                estamos cargando; cuando `has_more = false` desaparece
                (evita infinite triggering). */}
            {(hasMore || loadingMore) && (
              <div
                ref={sentinelRef}
                className="flex h-24 items-center justify-center"
              >
                {loadingMore && (
                  <div className="h-8 w-8 animate-spin rounded-full border-2 border-accent border-t-transparent" />
                )}
              </div>
            )}

            {!hasMore && !loadingMore && (
              <p className="mt-10 text-center text-[12px] text-dim">
                {t('recs.endOfList', { n: items.length })}
              </p>
            )}
          </>
        )}
      </main>

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
                label: t('recs.menu.detail'),
                hint: '↵',
                onClick: () => setDetail(rec),
              },
              {
                label: t('recs.menu.torrents'),
                hint: 't',
                onClick: () => openTorrents(rec),
              },
              {
                label: t('home.dismiss'),
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
              alt={movie.title}
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
