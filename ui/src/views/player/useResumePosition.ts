import { useCallback, useEffect } from 'react'
import { reportPosition, type LastSubDto } from '../../lib/api'
import { type StreamInfo } from '../../lib/api'

interface ResumeOptions {
  stream: StreamInfo | null
  duration: number | null
  streamIdRef: React.MutableRefObject<number | null>
  currentTimeRef: React.MutableRefObject<number>
  durationRef: React.MutableRefObject<number | null>
  isSeries: boolean | undefined
  season: number | null | undefined
  episode: number | null | undefined
  tmdbId: number | null | undefined
  /** Metadata para el store "seguir viendo". Se snapshotea aquí para
   * que Home pueda pintar la card sin volver a llamar a TMDB. */
  title: string | undefined
  imdbId: string | null | undefined
  posterPath: string | null | undefined
  backdropPath: string | null | undefined
  year: number | null | undefined
  magnet: string | null | undefined
  /** Ref al `ActiveSub` actual — leemos siempre el valor último sin
   * re-crear el callback en cada cambio de sub. */
  activeSubRef: React.MutableRefObject<LastSubDto | null>
}

export function useResumePosition({
  stream,
  duration,
  streamIdRef,
  currentTimeRef,
  durationRef,
  isSeries,
  season,
  episode,
  tmdbId,
  title,
  imdbId,
  posterPath,
  backdropPath,
  year,
  magnet,
  activeSubRef,
}: ResumeOptions): { reportPositionNow: () => Promise<void> } {
  const reportPositionNow = useCallback(async () => {
    const id = streamIdRef.current
    const t = currentTimeRef.current
    const d = durationRef.current
    if (id == null || d == null || d <= 0) return
    try {
      const s = isSeries ? (season ?? null) : null
      const e = isSeries ? (episode ?? null) : null
      // tmdbId aplica a peli Y a serie — el store por-peli lo usa
      // en ambos casos (con season=null,episode=null para movies).
      // El bug anterior condicionaba `tid` a `isSeries`, así que las
      // pelis nunca escribían al store movie-level y "seguir viendo"
      // salía vacío para ellas.
      const tid = tmdbId ?? null
      const activeSub = activeSubRef.current
      // Convertimos el ActiveSub a payload backend. `null` (sin
      // subs) manda `{source:'none'}` para que el backend borre el
      // last_sub del store en vez de dejar el que había — así el
      // user que desactiva subs y sale ve el player limpio al
      // reentrar.
      const subPayload =
        activeSub == null
          ? ({ source: 'none' } as const)
          : activeSub
      await reportPosition(
        id,
        t,
        d,
        s,
        e,
        tid,
        title ?? null,
        imdbId ?? null,
        posterPath ?? null,
        backdropPath ?? null,
        year ?? null,
        magnet ?? null,
        subPayload,
      )
    } catch {
      /* best-effort */
    }
  }, [
    streamIdRef,
    currentTimeRef,
    durationRef,
    activeSubRef,
    isSeries,
    season,
    episode,
    tmdbId,
    title,
    imdbId,
    posterPath,
    backdropPath,
    year,
    magnet,
  ])

  useEffect(() => {
    if (!stream || duration == null || duration <= 0) return
    const id = window.setInterval(() => {
      void reportPositionNow()
    }, 15000)
    return () => window.clearInterval(id)
  }, [stream, duration, reportPositionNow])

  return { reportPositionNow }
}
