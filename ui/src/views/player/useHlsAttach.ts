import { useCallback, useEffect, useRef, useState, type RefObject } from 'react'
import Hls from 'hls.js'
import {
  hlsUrl,
  setAudioTrack,
  type MediaInfo,
  type StreamInfo,
} from '../../lib/api'
import { formatSpeed } from './utils'

/**
 * Hooks del pipeline de vídeo:
 *   * {@link useHlsAttach}: decide `videoSrc` (DIRECT vs HLS via
 *     `canGoDirect`), monta hls.js cuando falta soporte nativo,
 *     y maneja el fatal 503 `swarm_stalled` del backend.
 *   * {@link useAudioSwitch}: cambia la pista de audio actual
 *     (POST `/hls/audio` + purga + respawn ffmpeg), guarda
 *     seek pendiente para restaurar tras el re-mount de hls.js.
 *
 * Ambos comparten el patrón "el `useEffect` de hls.js observa
 * `activeAudioIdx`" — al cambiar de pista se dispara destroy+new
 * Hls con la misma URL, y ffmpeg respawnea con `-map 0:a:<idx>`
 * en la primera petición de segmento del nuevo cliente.
 */

/** Decide si el `<video>` puede apuntar directo al `/video` raw:
 *   * `directFailed = true` (fallback runtime) → NO.
 *   * `direct_playable = false` desde el backend → NO.
 *   * HEVC pero WebView2 sin extensión de Microsoft Store → NO
 *     (probe `hvc1.1.6.L123.B0` === 'probably' como strict check).
 *   * Resto → SÍ.
 */
export function canGoDirect(media: MediaInfo | null, directFailed: boolean): boolean {
  if (directFailed) return false
  if (!media?.direct_playable) return false
  const videoStream = media.streams.find((s) => s.kind === 'video')
  const codec = videoStream?.codec?.toLowerCase() ?? ''
  const isHevc = codec === 'hevc' || codec === 'h265' || codec === 'h.265'
  if (!isHevc) return true
  if (typeof document === 'undefined') return true
  const probe = document.createElement('video')
  return probe.canPlayType('video/mp4; codecs="hvc1.1.6.L123.B0"') === 'probably'
}

export interface UseHlsAttachArgs {
  videoRef: RefObject<HTMLVideoElement | null>
  stream: StreamInfo | null
  media: MediaInfo | null
  directFailed: boolean
  /** Índice de pista de audio activa — está en las deps del effect
   * para forzar destroy + new Hls cuando el user cambia de pista. */
  activeAudioIdx: number
  /** Ruta canónica de la lista de torrents del título — se fija
   * como `errorBackTo` cuando llega un fatal `swarm_stalled`. */
  torrentsRoute: string | null
  setError: (msg: string) => void
  setErrorBackTo: (route: string | null) => void
  t: (key: string, vars?: Record<string, string | number>) => string
}

export interface UseHlsAttachResult {
  videoSrc: string | null
  needsHls: boolean
  canGoDirect: boolean
}

export function useHlsAttach(args: UseHlsAttachArgs): UseHlsAttachResult {
  const {
    videoRef,
    stream,
    media,
    directFailed,
    activeAudioIdx,
    torrentsRoute,
    setError,
    setErrorBackTo,
    t,
  } = args

  const direct = canGoDirect(media, directFailed)
  const videoSrc = stream && media ? (direct ? stream.url : hlsUrl(stream.url)) : null
  const needsHls = !!(stream && media && !direct)

  useEffect(() => {
    const v = videoRef.current
    if (!v || !videoSrc) return
    const nativeHls = v.canPlayType('application/vnd.apple.mpegurl') !== ''
    if (needsHls && !nativeHls) {
      if (!Hls.isSupported()) {
        // Ni HLS nativo ni MSE — plataforma sin soporte de vídeo
        // decente. Cae al mensaje de error genérico; el user puede
        // volver atrás y cambiar a VLC en Ajustes.
        setError(t('player.hlsUnsupported'))
        return
      }
      const hls = new Hls({
        // VOD con segmentos bajo demanda con progress-sensitive
        // deadline en el backend (audit §3.a): hard 120s, o 15s sin
        // progreso → 503 swarm_stalled. hls.js debe esperar al
        // backend (una sola fuente de verdad) → subimos los
        // timeouts por encima del hard deadline para que un abort
        // por timeout de hls.js NUNCA gane la carrera al backend.
        fragLoadingTimeOut: 130_000,
        manifestLoadingTimeOut: 20_000,
        fragLoadingMaxRetry: 6,
      })
      hls.loadSource(videoSrc)
      hls.attachMedia(v)
      hls.on(Hls.Events.ERROR, (_evt, data) => {
        if (!data.fatal) return
        console.warn('[hls] fatal', data.type, data.details)
        const resp = data.response
        if (resp && resp.code === 503) {
          try {
            const raw =
              typeof resp.data === 'string'
                ? resp.data
                : new TextDecoder().decode(resp.data as ArrayBuffer)
            const stall = JSON.parse(raw) as {
              reason?: string
              downloaded_pct?: number
              speed_bps?: number
              peers?: number
            }
            if (stall.reason === 'swarm_stalled') {
              setError(
                t('player.swarmStalled', {
                  speed: formatSpeed(stall.speed_bps ?? 0),
                  peers: String(stall.peers ?? 0),
                  pct: (stall.downloaded_pct ?? 0).toFixed(1),
                }),
              )
              setErrorBackTo(torrentsRoute)
              return
            }
          } catch {
            /* fallthrough al mensaje genérico */
          }
        }
        setError(t('player.hlsFatal', { type: data.type, details: data.details }))
      })
      return () => {
        try {
          hls.destroy()
        } catch {
          /* destroy es best-effort */
        }
      }
    }
    // Path nativo (DIRECT MP4 o HLS en Safari): asignar src.
    if (v.src !== videoSrc) {
      v.src = videoSrc
    }
    // `activeAudioIdx` fuerza re-run al cambiar pista. `t` y
    // `torrentsRoute` son estables y solo se leen en el catch fatal.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [videoSrc, needsHls, activeAudioIdx])

  return { videoSrc, needsHls, canGoDirect: direct }
}

// ── Audio switch ────────────────────────────────────────────────

/** Ref del seek pendiente tras un cambio de pista. Se restaura
 * en `onLoadedMetadata` del nuevo Hls; ver comentario en el hook. */
export interface PostAudioSwitchSeek {
  time: number
  play: boolean
}

export interface UseAudioSwitchArgs {
  videoRef: RefObject<HTMLVideoElement | null>
  stream: StreamInfo | null
  /** Se llama con `true` durante el cambio para tapar con
   * `StremioLoader` mientras dura el respawn. */
  setAudioSwitching: (v: boolean) => void
  setBuffering: (v: boolean) => void
}

export interface UseAudioSwitchResult {
  activeAudioIdx: number
  setActiveAudioIdx: (v: number) => void
  postAudioSwitchSeekRef: RefObject<PostAudioSwitchSeek | null>
  switchAudioTrack: (newIdx: number) => Promise<void>
}

/** Estado + callback para cambio de pista de audio. La ref
 * `postAudioSwitchSeekRef` la consume `onLoadedMetadata` del
 * `<video>` en Player.tsx para restaurar posición y playback tras
 * el destroy+new Hls que dispara `activeAudioIdx`. */
export function useAudioSwitch(args: UseAudioSwitchArgs): UseAudioSwitchResult {
  const { videoRef, stream, setAudioSwitching, setBuffering } = args
  const [activeAudioIdx, setActiveAudioIdx] = useState(0)
  const postAudioSwitchSeekRef = useRef<PostAudioSwitchSeek | null>(null)

  const switchAudioTrack = useCallback(
    async (newIdx: number) => {
      if (!stream) return
      if (newIdx === activeAudioIdx) return
      const v = videoRef.current
      if (!v) return
      postAudioSwitchSeekRef.current = {
        time: v.currentTime,
        play: !v.paused,
      }
      setAudioSwitching(true)
      setBuffering(true)
      try {
        v.pause()
      } catch {
        /* pause sync error, ignore */
      }
      try {
        await setAudioTrack(stream.url, newIdx)
      } catch (e) {
        console.warn('setAudioTrack failed:', e)
        setAudioSwitching(false)
        postAudioSwitchSeekRef.current = null
        return
      }
      setActiveAudioIdx(newIdx)
      // El `useEffect` de hls.js en `useHlsAttach` observa
      // `activeAudioIdx` en sus deps y se re-run: destroy + new Hls
      // con la misma URL; ffmpeg respawnea con `-map 0:a:<idx>` en
      // la primera petición de segmento del hls.js nuevo.
    },
    [stream, activeAudioIdx, videoRef, setAudioSwitching, setBuffering],
  )

  return {
    activeAudioIdx,
    setActiveAudioIdx,
    postAudioSwitchSeekRef,
    switchAudioTrack,
  }
}
