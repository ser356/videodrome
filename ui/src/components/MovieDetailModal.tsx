import { useEffect, useState } from 'react'
import { CaretRight, Star, X } from '@phosphor-icons/react'
import {
  getMovieView,
  tmdbBackdrop,
  tmdbPoster,
  type MovieView,
  type Recommendation,
} from '../lib/api'
import { useHotkeys } from '../lib/hotkeys'
import { useT } from '../lib/i18n'

/**
 * Modal de detalle estilo Stremio: backdrop grande arriba con fade al
 * fondo, poster miniatura + metadata a la izquierda, sinopsis a la
 * derecha. CTA principal: "Ver torrents".
 *
 * Hotkeys locales: `Enter` → torrents, `Esc` → cerrar. Cuando este modal
 * está abierto, los hotkeys de la vista padre deben deshabilitarse
 * pasando `enabled: false` a su `useHotkeys` para evitar dobles bindings.
 */
export function MovieDetailModal({
  rec,
  onClose,
  onOpenTorrents,
}: {
  rec: Recommendation
  onClose: () => void
  onOpenTorrents: (rec: Recommendation) => void
}) {
  const t = useT()
  const [view, setView] = useState<MovieView | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setView(null)
    getMovieView(rec.movie.id)
      .then((v) => {
        if (!cancelled) setView(v)
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [rec.movie.id])

  // Nota: antes se precargaba backdrop + poster antes de mostrar el
  // modal para evitar el "pop" de imágenes; en la práctica añadía
  // 500ms-3s de dot-loader antes de ver nada. Ahora mostramos el
  // modal al instante con lo que `rec.movie` ya trae (título, poster,
  // año, rating) y las imágenes se van rellenando conforme cargan.
  // La sinopsis / géneros / backdrop llegan cuando `getMovieView`
  // resuelve — se cachean en disco 7d, así que el segundo click a la
  // misma peli es literalmente instantáneo.

  useHotkeys(
    [
      { key: 'Escape', hint: '', run: onClose },
      { key: 'Enter', hint: '', run: () => onOpenTorrents(rec) },
    ],
    [rec, onClose, onOpenTorrents],
  )

  const title = view?.title ?? rec.movie.title
  const year =
    (view?.release_date ?? rec.movie.release_date)?.slice(0, 4) ?? ''
  const posterSrc = tmdbPoster(view?.poster_path ?? rec.movie.poster_path)
  const backdropSrc = tmdbBackdrop(view?.backdrop_path ?? null)
  const runtime = view?.runtime
  const genres = view?.genres ?? []
  const rating = view?.vote_average ?? rec.movie.vote_average
  const overview = view?.overview
  const lbRating = rec.lb_rating

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="glass-strong animate-modal-in relative flex max-h-[90vh] w-full max-w-[900px] flex-col overflow-hidden rounded-2xl border border-hairline"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        >
        {/* Backdrop header */}
        <div className="relative h-[260px] w-full shrink-0 overflow-hidden bg-surface-hi">
          {backdropSrc && (
            <img
              src={backdropSrc}
              alt=""
              className="h-full w-full object-cover"
              onError={(e) => {
                e.currentTarget.style.display = 'none'
              }}
            />
          )}
          <div className="absolute inset-0 bg-gradient-to-t from-canvas via-canvas/60 to-transparent" />
          <button
            onClick={onClose}
            className="focus-ring glass absolute right-3 top-3 flex h-8 w-8 items-center justify-center rounded-full text-body hover:text-ink"
            aria-label={t('common.close')}
            title={`${t('common.close')} (Esc)`}
          >
            <X size={16} weight="bold" />
          </button>
        </div>

        {/* Body */}
        <div className="relative -mt-24 flex gap-6 overflow-y-auto px-8 pb-8">
          {/* Poster mini */}
          <div className="shrink-0">
            <div className="aspect-[2/3] w-[160px] overflow-hidden rounded-poster bg-surface-hi shadow-2xl">
              {posterSrc && (
                <img
                  src={posterSrc}
                  alt={title}
                  className="h-full w-full object-cover"
                  onError={(e) => {
                    e.currentTarget.style.display = 'none'
                  }}
                />
              )}
            </div>
          </div>

          {/* Meta + overview */}
          <div className="min-w-0 flex-1 pt-24">
            <h2 className="text-[26px] font-semibold leading-tight text-ink">
              {title}
            </h2>

            <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted">
              {year && <span>{year}</span>}
              {runtime != null && (
                <>
                  <span aria-hidden>·</span>
                  <span>{formatRuntime(runtime)}</span>
                </>
              )}
              {rating > 0 && (
                <>
                  <span aria-hidden>·</span>
                  <span className="inline-flex items-center gap-1">
                    <Star size={12} weight="fill" className="text-accent" />
                    {rating.toFixed(1)}
                  </span>
                </>
              )}
              {lbRating != null && (
                <>
                  <span aria-hidden>·</span>
                  <span>LB {lbRating.toFixed(2)}</span>
                </>
              )}
            </div>

            {genres.length > 0 && (
              <div className="mt-3 flex flex-wrap gap-1.5">
                {genres.map((g) => (
                  <span
                    key={g}
                    className="rounded-full border border-hairline px-2 py-0.5 text-[11px] text-body"
                  >
                    {g}
                  </span>
                ))}
              </div>
            )}

            {view?.tagline && (
              <p className="mt-4 text-[14px] italic text-muted">
                {view.tagline}
              </p>
            )}

            {loading && !overview ? (
              // Skeleton mientras espera TMDB. 3 barras para dar
              // sensación de "hay texto viniendo" en vez del vacío
              // que confunde con "sin sinopsis".
              <div className="mt-4 space-y-2" aria-hidden>
                <div className="h-3 w-full animate-pulse rounded bg-surface" />
                <div className="h-3 w-11/12 animate-pulse rounded bg-surface" />
                <div className="h-3 w-3/4 animate-pulse rounded bg-surface" />
              </div>
            ) : (
              <p className="mt-4 whitespace-pre-line text-[14px] leading-relaxed text-body">
                {overview ?? t('movieDetail.noOverview')}
              </p>
            )}

            <div className="mt-6 flex items-center gap-3">
              <button
                onClick={() => onOpenTorrents(rec)}
                className="focus-ring inline-flex items-center gap-2 rounded-full bg-ink px-5 py-2.5 text-[13px] font-semibold text-canvas transition-transform hover:scale-[1.02]"
              >
                {t('movieDetail.viewTorrents')}
                <CaretRight size={14} weight="bold" />
              </button>
              <button
                onClick={onClose}
                className="focus-ring rounded-full px-4 py-2.5 text-[13px] text-muted hover:text-ink"
              >
                {t('common.close')}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function formatRuntime(mins: number): string {
  const h = Math.floor(mins / 60)
  const m = mins % 60
  if (h === 0) return `${m} min`
  if (m === 0) return `${h} h`
  return `${h} h ${m} min`
}
