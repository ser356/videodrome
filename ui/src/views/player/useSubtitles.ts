import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  downloadSubtitle,
  fetchEmbeddedSubtitle,
  getPreferences,
  getResume,
  searchSubtitles,
  subtitleToVtt,
  type MediaStream,
  type StreamInfo,
  type Subtitle,
} from '../../lib/api'
import { getLocale, mergeSubtitleLangs } from '../../lib/i18n'

/**
 * Pipeline completo de subtítulos del Player:
 *   * Auto-fetch del catálogo OpenSubtitles cuando el stream está
 *     listo, con el UI locale como primer idioma preferido.
 *   * Hidratación de `activeSub` desde el store movie-level
 *     (`getResume().last_sub`) — respeta el flujo legacy que
 *     pasa `subPath` en `state`.
 *   * Descarga VTT (openSubs vía backend, o embedded vía endpoint
 *     `/subs/embedded/<idx>.vtt`) y crea Blob URL estable.
 *   * Aplica shift de cues por `subOffset` / `subSpeed`
 *     in-place sobre `textTracks[0].cues` (sin tocar el blob:
 *     reasignar `<track src>` a mitad de carga rompe WKWebView).
 *   * Fuerza `mode='showing'` en el TextTrack tras cada
 *     `loadedmetadata` / `loadeddata` — WKWebView y Safari resetean
 *     los tracks a `disabled` en cada re-load del `<video>`.
 *   * Drag & drop de subs locales (`.srt` / `.vtt` / `.ass` / `.ssa`)
 *     vía `getCurrentWebview().onDragDropEvent` (los HTML5 handlers
 *     no reciben el path del fichero en Tauri).
 *
 * Extraído de `Player.tsx` para bajar el tamaño del componente y
 * aislar el pipeline; su lógica es sustancial y muy testeable en
 * aislamiento.
 */

/** Sub activo: unión discriminada entre "descargado de
 * OpenSubtitles" (fichero local que el backend convierte a VTT)
 * y "extraído del contenedor" (ffmpeg extrae la pista `idx` del
 * torrent como VTT en un endpoint one-shot). Los dos casos se
 * colapsan en el mismo blob VTT en el `<video>`. */
export type ActiveSub =
  | { source: 'openSubs'; path: string; release: string; language: string }
  | { source: 'embedded'; idx: number; release: string; language: string }

/** Sub-conjunto de `PlayerState` que necesita el hook. */
export interface SubtitlesState {
  magnet: string
  title: string
  imdbId: string | null
  tmdbId?: number | null
  subPath: string | null
  subRelease: string | null
  isSeries?: boolean
  season?: number | null
  episode?: number | null
}

export interface UseSubtitlesArgs {
  stream: StreamInfo | null
  state: SubtitlesState | null
  videoRef: RefObject<HTMLVideoElement | null>
  /** Ref compartida con `useResumePosition` para persistir la pista
   * de subs en el store movie-level sin re-crear el reporter en
   * cada cambio. El hook la sincroniza vía effect con el `activeSub`
   * actual. Se pasa por ref para romper cualquier orden de creación
   * entre este hook y `useResumePosition`. */
  activeSubRef: RefObject<ActiveSub | null>
  /** i18n. Solo se lee en el handler de drag&drop para el mensaje
   * de "fichero inválido". */
  t: (key: string, vars?: Record<string, string | number>) => string
}

export interface UseSubtitlesResult {
  activeSub: ActiveSub | null
  setActiveSub: Dispatch<SetStateAction<ActiveSub | null>>
  subsList: Subtitle[] | null
  subsLoading: boolean
  subsPanelOpen: boolean
  setSubsPanelOpen: Dispatch<SetStateAction<boolean>>
  subDownloading: number | null
  vttUrl: string | null
  subOffset: number
  setSubOffset: Dispatch<SetStateAction<number>>
  subSpeed: number
  setSubSpeed: Dispatch<SetStateAction<number>>
  syncHud: string | null
  showSyncHud: (text: string) => void
  dragActive: boolean
  dragFlash: boolean
  dragError: string | null
  pickSub: (sub: Subtitle) => Promise<void>
  pickEmbeddedSub: (streamInfo: MediaStream, subIdx: number) => void
  clearSub: () => void
}

export function useSubtitles(args: UseSubtitlesArgs): UseSubtitlesResult {
  const { stream, state, videoRef, activeSubRef, t } = args

  const [subsList, setSubsList] = useState<Subtitle[] | null>(null)
  const [subsLoading, setSubsLoading] = useState(false)
  const [subsPanelOpen, setSubsPanelOpen] = useState(false)
  const [subDownloading, setSubDownloading] = useState<number | null>(null)

  const [activeSub, setActiveSub] = useState<ActiveSub | null>(
    state?.subPath
      ? {
          source: 'openSubs',
          path: state.subPath,
          release: state.subRelease ?? 'Subs',
          language: 'es',
        }
      : null,
  )

  // Sincroniza el ref compartido con `useResumePosition`.
  useEffect(() => {
    activeSubRef.current = activeSub
  }, [activeSub, activeSubRef])

  // Hidratación desde `movie_progress.json` (getResume) al montar.
  // No pisa la elección explícita del user (flujo legacy con subPath).
  useEffect(() => {
    if (state?.subPath) return
    if (!state?.magnet) return
    let cancelled = false
    ;(async () => {
      try {
        const r = await getResume(
          state.magnet,
          state.isSeries ? (state.season ?? null) : null,
          state.isSeries ? (state.episode ?? null) : null,
          state.tmdbId ?? null,
        )
        if (cancelled || !r?.last_sub) return
        setActiveSub(r.last_sub)
      } catch {
        /* silencioso: sin sub previo es el estado normal */
      }
    })()
    return () => {
      cancelled = true
    }
    // Solo al mount — dependemos de valores que no cambian mid-vida.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Auto-fetch del catálogo tras tener stream. UI locale como primer
  // idioma (§ audit i18n).
  useEffect(() => {
    if (!stream) return
    let cancelled = false
    // eslint-disable-next-line react-hooks/set-state-in-effect -- Loading flag síncrono; el resto se resuelve async.
    setSubsLoading(true)
    ;(async () => {
      let langs: string = getLocale()
      try {
        const prefs = await getPreferences()
        langs = mergeSubtitleLangs(getLocale(), prefs.subtitle_languages)
      } catch {
        /* best-effort */
      }
      if (cancelled) return
      try {
        const subs = await searchSubtitles(
          stream.id,
          state?.imdbId ?? null,
          state?.title ?? null,
          langs,
          state?.isSeries ? (state.season ?? null) : null,
          state?.isSeries ? (state.episode ?? null) : null,
        )
        if (!cancelled) setSubsList(subs)
      } catch (e) {
        console.warn('searchSubtitles failed:', e)
        if (!cancelled) setSubsList([])
      } finally {
        if (!cancelled) setSubsLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [
    stream,
    state?.imdbId,
    state?.title,
    state?.isSeries,
    state?.season,
    state?.episode,
  ])

  // VTT raw + Blob URL estable.
  const [rawVtt, setRawVtt] = useState<string | null>(null)
  useEffect(() => {
    if (!activeSub) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono cuando no hay sub activo.
      setRawVtt(null)
      return
    }
    let cancelled = false
    ;(async () => {
      try {
        const vtt =
          activeSub.source === 'openSubs'
            ? await subtitleToVtt(activeSub.path)
            : await fetchEmbeddedSubtitle(stream?.url ?? '', activeSub.idx)
        if (!cancelled) setRawVtt(vtt)
      } catch (e) {
        console.warn('vtt fetch failed:', e)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [activeSub, stream])

  const [vttUrl, setVttUrl] = useState<string | null>(null)
  useEffect(() => {
    if (!rawVtt) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono cuando no hay vtt.
      setVttUrl(null)
      return
    }
    const blob = new Blob([rawVtt], { type: 'text/vtt' })
    const url = URL.createObjectURL(blob)
    setVttUrl(url)
    return () => URL.revokeObjectURL(url)
  }, [rawVtt])

  // Timestamps ORIGINALES por cue — necesarios para re-shift no-acumulativo.
  const cueOriginalTimesRef = useRef<Map<number, [number, number]>>(new Map())
  useEffect(() => {
    cueOriginalTimesRef.current = new Map()
  }, [rawVtt])

  const [subOffset, setSubOffset] = useState(0)
  const [subSpeed, setSubSpeed] = useState(1)
  const [syncHud, setSyncHud] = useState<string | null>(null)
  const syncHudTimerRef = useRef<number | null>(null)
  const showSyncHud = useCallback((text: string) => {
    setSyncHud(text)
    if (syncHudTimerRef.current) window.clearTimeout(syncHudTimerRef.current)
    syncHudTimerRef.current = window.setTimeout(() => setSyncHud(null), 1500)
  }, [])

  // Reset del sync al cambiar de sub.
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono de offset/speed al cambiar de sub.
    setSubOffset(0)
    setSubSpeed(1)
  }, [activeSub])

  // Shift de cues in-place (fórmula: cue_final = cue_original * subSpeed + subOffset).
  useEffect(() => {
    const v = videoRef.current
    if (!v || !vttUrl) return
    const applyShift = () => {
      const tracks = v.textTracks
      if (tracks.length === 0) return
      const cues = tracks[0].cues
      if (!cues) return
      for (let i = 0; i < cues.length; i++) {
        const cue = cues[i]
        let original = cueOriginalTimesRef.current.get(i)
        if (!original) {
          original = [cue.startTime, cue.endTime]
          cueOriginalTimesRef.current.set(i, original)
        }
        const [origStart, origEnd] = original
        cue.startTime = Math.max(0, origStart * subSpeed + subOffset)
        cue.endTime = Math.max(0, origEnd * subSpeed + subOffset)
      }
    }
    applyShift()
    const trackEl = v.querySelector('track')
    trackEl?.addEventListener('load', applyShift)
    return () => {
      trackEl?.removeEventListener('load', applyShift)
    }
  }, [vttUrl, subOffset, subSpeed, videoRef])

  // Fuerza `mode='showing'` en el TextTrack tras cada re-load del `<video>`.
  useEffect(() => {
    const v = videoRef.current
    if (!v || !vttUrl) return
    const applyMode = () => {
      const tracks = v.textTracks
      for (let i = 0; i < tracks.length; i++) {
        tracks[i].mode = i === 0 ? 'showing' : 'disabled'
      }
    }
    const raf = requestAnimationFrame(applyMode)
    v.addEventListener('loadedmetadata', applyMode)
    v.addEventListener('loadeddata', applyMode)
    return () => {
      cancelAnimationFrame(raf)
      v.removeEventListener('loadedmetadata', applyMode)
      v.removeEventListener('loadeddata', applyMode)
    }
  }, [vttUrl, videoRef])

  const pickSub = useCallback(
    async (sub: Subtitle) => {
      if (subDownloading !== null) return
      setSubDownloading(sub.file_id)
      try {
        const path = await downloadSubtitle(sub)
        setActiveSub({
          source: 'openSubs',
          path,
          release: sub.release || sub.file_name || 'Sub',
          language: sub.language,
        })
        setSubsPanelOpen(false)
      } catch (e) {
        console.warn('downloadSubtitle failed:', e)
      } finally {
        setSubDownloading(null)
      }
    },
    [subDownloading],
  )

  const pickEmbeddedSub = useCallback(
    (streamInfo: MediaStream, subIdx: number) => {
      setActiveSub({
        source: 'embedded',
        idx: subIdx,
        release: streamInfo.title || `Track #${subIdx + 1}`,
        language: streamInfo.language || 'und',
      })
      setSubsPanelOpen(false)
    },
    [],
  )

  const clearSub = useCallback(() => {
    setActiveSub(null)
  }, [])

  // ---- Drag & drop de subtítulos locales ----
  const [dragActive, setDragActive] = useState(false)
  const [dragFlash, setDragFlash] = useState(false)
  const [dragError, setDragError] = useState<string | null>(null)
  const dragErrorTimerRef = useRef<number | null>(null)
  useEffect(() => {
    let unlisten: (() => void) | null = null
    let cancelled = false
    ;(async () => {
      try {
        const webview = getCurrentWebview()
        const off = await webview.onDragDropEvent((event) => {
          const payload = event.payload
          if (payload.type === 'enter') {
            setDragActive(true)
          } else if (payload.type === 'over') {
            // Tauri 2: el evento `over` NO trae `paths` — solo
            // `position`. Mantenemos el overlay activo tal cual;
            // el path real lo consumimos en `drop`.
            setDragActive(true)
          } else if (payload.type === 'leave') {
            setDragActive(false)
          } else if (payload.type === 'drop') {
            setDragActive(false)
            const paths = payload.paths as string[]
            const sub = paths.find((p) => /\.(srt|vtt|ass|ssa)$/i.test(p))
            if (!sub) {
              setDragError(t('player.subDropInvalid'))
              if (dragErrorTimerRef.current)
                window.clearTimeout(dragErrorTimerRef.current)
              dragErrorTimerRef.current = window.setTimeout(
                () => setDragError(null),
                2500,
              )
              return
            }
            const base = sub.split(/[\\/]/).pop() ?? sub
            const release = base.replace(/\.[^.]+$/, '')
            const langMatch = base.match(/[._-]([a-z]{2,3})[._-]/i)
            const language =
              (langMatch?.[1] ?? activeSubRef.current?.language ?? 'es').toLowerCase()
            setActiveSub({
              source: 'openSubs',
              path: sub,
              release,
              language,
            })
            setDragFlash(true)
            window.setTimeout(() => setDragFlash(false), 800)
          }
        })
        if (cancelled) off()
        else unlisten = off
      } catch (e) {
        console.warn('onDragDropEvent listener setup failed:', e)
      }
    })()
    return () => {
      cancelled = true
      unlisten?.()
      if (dragErrorTimerRef.current)
        window.clearTimeout(dragErrorTimerRef.current)
    }
    // Deps: t (i18n) para re-suscribir al cambiar locale. `activeSubRef`
    // se lee via ref, no depende.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t])

  return {
    activeSub,
    setActiveSub,
    subsList,
    subsLoading,
    subsPanelOpen,
    setSubsPanelOpen,
    subDownloading,
    vttUrl,
    subOffset,
    setSubOffset,
    subSpeed,
    setSubSpeed,
    syncHud,
    showSyncHud,
    dragActive,
    dragFlash,
    dragError,
    pickSub,
    pickEmbeddedSub,
    clearSub,
  }
}
