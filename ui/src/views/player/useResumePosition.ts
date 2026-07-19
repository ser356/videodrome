import { useCallback, useEffect } from 'react'
import { reportPosition } from '../../lib/api'
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
}: ResumeOptions): { reportPositionNow: () => Promise<void> } {
  const reportPositionNow = useCallback(async () => {
    const id = streamIdRef.current
    const t = currentTimeRef.current
    const d = durationRef.current
    if (id == null || d == null || d <= 0) return
    try {
      const s = isSeries ? (season ?? null) : null
      const e = isSeries ? (episode ?? null) : null
      const tid = isSeries ? (tmdbId ?? null) : null
      await reportPosition(id, t, d, s, e, tid)
    } catch {
      /* best-effort */
    }
  }, [streamIdRef, currentTimeRef, durationRef, isSeries, season, episode, tmdbId])

  useEffect(() => {
    if (!stream || duration == null || duration <= 0) return
    const id = window.setInterval(() => {
      void reportPositionNow()
    }, 15000)
    return () => window.clearInterval(id)
  }, [stream, duration, reportPositionNow])

  return { reportPositionNow }
}
