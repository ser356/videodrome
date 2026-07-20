import { useEffect, useState, type RefObject } from 'react'
import {
  formatSize,
  getMovieView,
  probeStream,
  ProbeStalledError,
  startStreamHtml,
  stopStream,
  streamStats,
  tmdbBackdrop,
  type MediaInfo,
  type StreamInfo,
  type StreamStats,
} from '../../lib/api'
import { ffmpegInstallHint } from './utils'

/**
 * Estado y efectos del pipeline "torrent → stream → media" del Player.
 * Extraído de `Player.tsx` para bajar el tamaño del componente y
 * facilitar tests unitarios de casos aislados (ProbeStalled, direct
 * fallback, etc.).
 *
 * Cubre:
 *   1. Arranque del stream (mount) → `startStreamHtml`.
 *   2. Probe de ffmpeg → `probeStream` (con manejo específico de
 *      `ProbeStalledError` y mensajes de ffmpeg missing).
 *   3. Backdrop + poster + año TMDB (para `StremioLoader` y snapshot
 *      del store movie-level).
 *   4. Logo URL de Metahub por `imdb_id`.
 *   5. Poll de `stream_stats` cada 1s.
 *   6. Cleanup al desmontar: flush de posición + `stopStream`.
 *
 * ⚠ El cleanup del efecto de arranque llama al `reportPositionNow` que
 * el caller le pasa via ref — necesario porque `useResumePosition`
 * (donde vive el reporter) depende a su vez de estado de este hook
 * (`posterPathRaw` / `backdropPathRaw` / `yearFromView`). Pasando por
 * ref evitamos la circularidad sin cambiar semántica.
 */

/**
 * Sub-conjunto de `PlayerState` que necesita el hook. Se acepta
 * como estructura suelta (no como el tipo completo) para que el hook
 * no dependa del componente que lo usa.
 */
export interface StreamLifecycleState {
  magnet: string
  title: string
  imdbId: string | null
  tmdbId?: number | null
  isSeries?: boolean
  season?: number | null
  episode?: number | null
  fileHint?: number | null
}

export interface UseStreamLifecycleArgs {
  state: StreamLifecycleState | null
  /** Ruta a la lista de torrents del título — se fija como
   * `errorBackTo` cuando el error es swarm-muerto (probe stalled)
   * para que el botón Volver del `ErrorOverlay` lleve al usuario a
   * elegir otro release. `null` si no conocemos `tmdbId` (flujo
   * directo). */
  torrentsRoute: string | null
  /** Ref al reporter periódico de posición. Se lee en el cleanup del
   * mount effect para hacer un flush final antes de `stopStream`. Se
   * pasa por ref (no valor) para romper la dependencia circular con
   * `useResumePosition`, que consume `posterPathRaw`/etc. de este
   * mismo hook. */
  reportPositionRef: RefObject<() => Promise<void>>
  /** i18n. `t` se lee dentro del hook para pintar mensajes de error.
   * Estable dentro de la vida del componente (`useT()` re-renderiza
   * al cambiar locale, pero eso también re-monta este componente
   * lógicamente porque el user no puede cambiar de idioma dentro del
   * Player sin salir). */
  t: (key: string, vars?: Record<string, string | number>) => string
}

export interface UseStreamLifecycleResult {
  stream: StreamInfo | null
  media: MediaInfo | null
  error: string | null
  errorBackTo: string | null
  directFailed: boolean
  setError: (v: string | null) => void
  setErrorBackTo: (v: string | null) => void
  setDirectFailed: (v: boolean) => void
  backdropUrl: string | null
  logoUrl: string | null
  posterPathRaw: string | null
  backdropPathRaw: string | null
  yearFromView: number | null
  stats: StreamStats | null
}

export function useStreamLifecycle(
  args: UseStreamLifecycleArgs,
): UseStreamLifecycleResult {
  const { state, torrentsRoute, reportPositionRef, t } = args

  const [stream, setStream] = useState<StreamInfo | null>(null)
  const [media, setMedia] = useState<MediaInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [errorBackTo, setErrorBackTo] = useState<string | null>(null)
  const [directFailed, setDirectFailed] = useState(false)
  const [stats, setStats] = useState<StreamStats | null>(null)
  const [backdropUrl, setBackdropUrl] = useState<string | null>(null)
  const [posterPathRaw, setPosterPathRaw] = useState<string | null>(null)
  const [backdropPathRaw, setBackdropPathRaw] = useState<string | null>(null)
  const [yearFromView, setYearFromView] = useState<number | null>(null)
  const [logoUrl, setLogoUrl] = useState<string | null>(null)

  // Arranca el stream al montar; para al desmontar.
  useEffect(() => {
    if (!state?.magnet) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Gate no-magnet: setState síncrona única antes de retornar.
      setError(t('player.noMagnet'))
      return
    }
    let cancelled = false
    let localStream: StreamInfo | null = null
    ;(async () => {
      try {
        // §4 audit series: si el user viene del flujo de serie
        // pasamos S/E → el backend selecciona el fichero del
        // episodio dentro del pack (`select_file`). Si el provider
        // ya nos dio el índice (Torrentio.fileIdx), lo pasamos como
        // `fileHint` — bypasa `select_file` y es más preciso.
        const info = await startStreamHtml(
          state.magnet,
          state.isSeries ? (state.season ?? null) : null,
          state.isSeries ? (state.episode ?? null) : null,
          state.fileHint ?? null,
        )
        if (cancelled) {
          await stopStream(info.id).catch(() => {})
          return
        }
        localStream = info
        setStream(info)
      } catch (e) {
        setError(t('player.startError', { err: String(e) }))
      }
    })()
    return () => {
      cancelled = true
      if (localStream) {
        // Flush final de la posición ANTES del stopStream: el
        // backend resuelve `report_position` mirando el stream_id
        // en el mapa vivo; tras `stop_stream` el slot se libera y
        // no habría dónde escribir el infohash. Fire-and-forget:
        // el cleanup no puede ser async, pero encolar el await es
        // suficiente porque Tauri serializa las invocaciones IPC.
        //
        // Nota: guardamos la ref al reporter en una variable local
        // para satisfacer `react-hooks/exhaustive-deps` (que avisa
        // sobre `ref.current` leído en cleanup); el valor efectivo
        // es el mismo porque el sync ref ↔ callback vive fuera del
        // hook y siempre apunta al último `reportPositionNow`.
        const id = localStream.id
        // Leer `reportPositionRef.current` DENTRO del cleanup es
        // intencional: queremos el reporter MÁS RECIENTE al
        // desmontar, no el capturado al mount. Snapshotarlo fuera
        // (como sugiere la regla) romperia esa semántica.
        // eslint-disable-next-line react-hooks/exhaustive-deps -- ver comentario arriba.
        const reporter = reportPositionRef.current
        void (async () => {
          await reporter()
          await stopStream(id).catch(() => {})
        })()
      }
    }
    // Solo arrancamos una vez al montar. El magnet nunca cambia
    // durante la vida del componente (navegación crea un mount nuevo).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // ffprobe: sacamos duración + streams tras tener el stream.
  useEffect(() => {
    if (!stream) return
    let cancelled = false
    ;(async () => {
      try {
        const info = await probeStream(stream.url)
        if (!cancelled) setMedia(info)
      } catch (e) {
        // Probe puede fallar por: swarm sin seeders (backend firma
        // `probe_stalled` con 504+JSON → ProbeStalledError),
        // ffmpeg/ffprobe no instalado, timeout, CSP bloqueando
        // 127.0.0.1. Sin `media` el <video> nunca se monta
        // (videoSrc = null) → onError nunca dispara → spinner
        // infinito. Hay que decidirle al user.
        if (cancelled) return
        if (e instanceof ProbeStalledError) {
          // Firma clara de "este torrent no arranca": mensaje
          // específico + botón Volver → lista de torrents del
          // título. Antes esto se ocultaba bajo el mensaje
          // genérico "comprueba ffmpeg", que era engañoso — el
          // binario está OK, el problema es el swarm.
          //
          // Distinguimos dos motivos:
          //   * `no_progress` — bytes de librqbit no aumentan.
          //     Swarm muerto de verdad (no peers o todos ausentes).
          //   * `hard_deadline` — bajaba bytes pero NO los que
          //     ffprobe necesita. Típico MP4 con moov al final
          //     que la política de piece-picking no prioriza.
          if (e.stallReason === 'hard_deadline') {
            setError(
              t('player.probeStalledHardDeadline', {
                elapsed: String(e.elapsedS),
                downloaded: formatSize(e.downloadedBytes),
                peers: String(e.peers),
              }),
            )
          } else {
            setError(
              t('player.probeStalledNoProgress', {
                stalled: String(e.stalledS || e.elapsedS),
                downloaded: formatSize(e.downloadedBytes),
                peers: String(e.peers),
              }),
            )
          }
          setErrorBackTo(torrentsRoute)
          return
        }
        const msg = String(e)
        const looksLikeMissingFfmpeg = /ffmpeg/i.test(msg)
        setError(
          looksLikeMissingFfmpeg
            ? t('player.ffmpegMissing', { hint: ffmpegInstallHint(t) })
            : t('player.probeFailed', { err: msg }),
        )
      }
    })()
    return () => {
      cancelled = true
    }
    // `stream` es el disparador real; `t` y `torrentsRoute` los
    // usamos en el catch pero son estables durante la vida del
    // componente. Añadirlos al array re-dispararía el probe sin
    // motivo.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stream])

  // Backdrop de TMDB + snapshot poster/año para el store movie-level.
  useEffect(() => {
    const id = state?.tmdbId
    if (!id) return
    let cancelled = false
    ;(async () => {
      try {
        const view = await getMovieView(id)
        if (cancelled) return
        // Prioridad al backdrop (16:9, encaja con el player); si no
        // hay, caemos al poster (2:3) — se ve descentrado pero es
        // mejor que negro plano.
        const url =
          tmdbBackdrop(view?.backdrop_path ?? null, 'w1280') ??
          tmdbBackdrop(view?.poster_path ?? null, 'w780')
        setBackdropUrl(url)
        setPosterPathRaw(view?.poster_path ?? null)
        setBackdropPathRaw(view?.backdrop_path ?? null)
        const y = view?.release_date?.slice(0, 4)
        setYearFromView(y ? Number(y) || null : null)
      } catch {
        /* silencioso: sin backdrop el loader cae a fondo negro */
      }
    })()
    return () => {
      cancelled = true
    }
  }, [state?.tmdbId])

  // Logo art via Metahub. URL directa por imdb_id, sin API key: si
  // hay HD Movie Logo en Fanart.tv, Metahub lo sirve; si no, 404 y
  // el `<img onError>` del loader cae al `<h1>` de texto.
  useEffect(() => {
    const raw = state?.imdbId
    if (!raw) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono cuando no hay imdb.
      setLogoUrl(null)
      return
    }
    const id = raw.startsWith('tt') ? raw : `tt${raw}`
    setLogoUrl(`https://images.metahub.space/logo/medium/${id}/img`)
  }, [state?.imdbId])

  // Poll de `stream_stats` cada 1s mientras el stream esté vivo.
  useEffect(() => {
    if (!stream) return
    let cancelled = false
    const poll = async () => {
      try {
        const s = await streamStats(stream.id)
        if (!cancelled) setStats(s)
      } catch {
        // Silencioso: si el handle ya no está en el mapa (stop_stream
        // o crash del backend), simplemente dejamos de pintar HUD.
        if (!cancelled) setStats(null)
      }
    }
    void poll()
    const id = window.setInterval(() => {
      void poll()
    }, 1000)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [stream])

  return {
    stream,
    media,
    error,
    errorBackTo,
    directFailed,
    setError,
    setErrorBackTo,
    setDirectFailed,
    backdropUrl,
    logoUrl,
    posterPathRaw,
    backdropPathRaw,
    yearFromView,
    stats,
  }
}
