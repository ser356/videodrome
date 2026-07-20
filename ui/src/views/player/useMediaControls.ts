import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'

/**
 * Estado y controles de reproducción del `<video>` HTML:
 *
 *   * Estado primitivo del player: paused, currentTime, volume,
 *     muted, buffering, isFullscreen, controlsVisible, seeking,
 *     hasStartedPlayback, stalledLong.
 *   * Refs volátiles: `currentTimeRef` (hotkeys leen de ella para
 *     no salir de un valor obsoleto), `isFullscreenRef`
 *     (para el handler de Esc que decide entre "salir de FS" y
 *     "volver atrás").
 *   * Autohide de controles: `bumpControls` resetea el timer de
 *     2.5s cuando hay actividad y solo oculta si `paused = false`.
 *   * Poll de fullscreen (1s) — WKWebView no expone evento nativo
 *     así que reflejamos el estado real de la ventana Tauri.
 *   * `stalledLong` — flag de 2s de latencia sobre
 *     `seeking || buffering` que separa el spinner ligero del
 *     StremioLoader completo (§3.b audit).
 *   * Sync de `v.volume` / `v.muted` con el estado React.
 *   * `seekTo` (pausa, seek, marca seeking), `seekBy` (leyendo la
 *     ref para hotkeys), `togglePlay`, `toggleFullscreen`,
 *     `onTimeUpdate`.
 *
 * NO cubre:
 *   * Attach de hls.js (`videoSrc` + effect) — depende de
 *     `activeAudioIdx` que vive fuera.
 *   * Handler `handleBack` — necesita `stream` + `reportPositionNow`
 *     + `nav` + `errorBackTo`, más idiomático mantenerlo en Player.tsx.
 *   * Los eventos `onPlay/onPause/onWaiting/onCanPlay/onSeeking/
 *     onSeeked/onPlaying/onError/onLoadedMetadata` del `<video>` —
 *     viven en el JSX; el hook expone los setters que necesitan.
 */

const CONTROLS_HIDE_MS = 2500

export interface UseMediaControlsArgs {
  videoRef: RefObject<HTMLVideoElement | null>
  /** Segundo inicial (resume). Se usa como valor inicial de
   * `currentTime` — permite mostrar el HUD en la posición correcta
   * antes de que `loadedmetadata` dispare el seek real. */
  initialSeconds: number
}

export interface UseMediaControlsResult {
  // Estado primitivo del player.
  paused: boolean
  setPaused: Dispatch<SetStateAction<boolean>>
  currentTime: number
  setCurrentTime: Dispatch<SetStateAction<number>>
  volume: number
  setVolume: Dispatch<SetStateAction<number>>
  muted: boolean
  setMuted: Dispatch<SetStateAction<boolean>>
  buffering: boolean
  setBuffering: Dispatch<SetStateAction<boolean>>
  isFullscreen: boolean
  setIsFullscreen: Dispatch<SetStateAction<boolean>>
  controlsVisible: boolean
  seeking: boolean
  setSeeking: Dispatch<SetStateAction<boolean>>
  hasStartedPlayback: boolean
  setHasStartedPlayback: Dispatch<SetStateAction<boolean>>
  stalledLong: boolean

  // Refs volátiles.
  currentTimeRef: RefObject<number>
  isFullscreenRef: RefObject<boolean>

  // Callbacks.
  seekTo: (absoluteSeconds: number) => void
  seekBy: (delta: number) => void
  togglePlay: () => void
  toggleFullscreen: () => Promise<void>
  onTimeUpdate: () => void
  bumpControls: () => void
}

export function useMediaControls(
  args: UseMediaControlsArgs,
): UseMediaControlsResult {
  const { videoRef, initialSeconds } = args

  const [paused, setPaused] = useState(true)
  const [currentTime, setCurrentTime] = useState(initialSeconds)
  const [volume, setVolume] = useState(1)
  const [muted, setMuted] = useState(false)
  const [buffering, setBuffering] = useState(true)
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [controlsVisible, setControlsVisible] = useState(true)
  const [seeking, setSeeking] = useState(false)
  const [hasStartedPlayback, setHasStartedPlayback] = useState(false)

  // `stalledLong` — 2s de latencia sobre `seeking || buffering` para
  // separar spinner ligero (rebuffer corto) de StremioLoader completo
  // (parada larga que merece explicación con backdrop+stats). Timer
  // arranca al entrar en stalling; se cancela + resetea al salir.
  const [stalledLong, setStalledLong] = useState(false)
  const stalledTimerRef = useRef<number | null>(null)
  useEffect(() => {
    const stalling = seeking || buffering
    if (!stalling) {
      if (stalledTimerRef.current) {
        window.clearTimeout(stalledTimerRef.current)
        stalledTimerRef.current = null
      }
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono al salir de estado stalling.
      setStalledLong(false)
      return
    }
    if (stalledTimerRef.current || stalledLong) return
    stalledTimerRef.current = window.setTimeout(() => {
      setStalledLong(true)
      stalledTimerRef.current = null
    }, 2000)
  }, [seeking, buffering, stalledLong])

  // Refs volátiles — leídas por handlers que NO deben re-suscribirse.
  const currentTimeRef = useRef(currentTime)
  const isFullscreenRef = useRef(isFullscreen)
  useEffect(() => {
    currentTimeRef.current = currentTime
  }, [currentTime])
  useEffect(() => {
    isFullscreenRef.current = isFullscreen
  }, [isFullscreen])

  // Sync volume/muted con el `<video>`.
  useEffect(() => {
    const v = videoRef.current
    if (!v) return
    v.volume = volume
    v.muted = muted
  }, [volume, muted, videoRef])

  // Autohide de controles.
  const hideTimerRef = useRef<number | null>(null)
  const bumpControls = useCallback(() => {
    setControlsVisible(true)
    if (hideTimerRef.current) window.clearTimeout(hideTimerRef.current)
    if (!paused) {
      hideTimerRef.current = window.setTimeout(() => {
        setControlsVisible(false)
      }, CONTROLS_HIDE_MS)
    }
  }, [paused])

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- bumpControls llama setControlsVisible síncrono al montar.
    bumpControls()
    return () => {
      if (hideTimerRef.current) window.clearTimeout(hideTimerRef.current)
    }
  }, [bumpControls])

  // Poll de fullscreen (1s) — WKWebView no dispara evento nativo.
  useEffect(() => {
    const w = getCurrentWindow()
    const check = () => {
      void w.isFullscreen().then(setIsFullscreen).catch(() => {})
    }
    check()
    const id = window.setInterval(check, 1000)
    return () => window.clearInterval(id)
  }, [])

  const seekTo = useCallback(
    (absoluteSeconds: number) => {
      const v = videoRef.current
      if (!v) return
      const target = Math.max(0, absoluteSeconds)
      setSeeking(true)
      setBuffering(true)
      setCurrentTime(target)
      try {
        v.pause()
      } catch {
        /* algún browser tira sync si no hay src todavía */
      }
      v.currentTime = target
    },
    [videoRef],
  )

  const seekBy = useCallback(
    (delta: number) => {
      seekTo(currentTimeRef.current + delta)
    },
    [seekTo],
  )

  const togglePlay = useCallback(() => {
    const v = videoRef.current
    if (!v) return
    if (v.paused) void v.play()
    else v.pause()
  }, [videoRef])

  const toggleFullscreen = useCallback(async () => {
    // Fullscreen a nivel de ventana Tauri (macOS: Split View / Space
    // dedicado; Windows/Linux: borderless fullscreen). WKWebView no
    // implementa `Element.requestFullscreen()` sobre `<div>`, así
    // que el path del DOM daría `undefined is not a function`.
    const w = getCurrentWindow()
    try {
      const current = await w.isFullscreen()
      await w.setFullscreen(!current)
      setIsFullscreen(!current)
    } catch (e) {
      console.warn('toggleFullscreen falló:', e)
    }
  }, [])

  const onTimeUpdate = useCallback(() => {
    const v = videoRef.current
    if (!v) return
    setCurrentTime(v.currentTime)
  }, [videoRef])

  return {
    paused,
    setPaused,
    currentTime,
    setCurrentTime,
    volume,
    setVolume,
    muted,
    setMuted,
    buffering,
    setBuffering,
    isFullscreen,
    setIsFullscreen,
    controlsVisible,
    seeking,
    setSeeking,
    hasStartedPlayback,
    setHasStartedPlayback,
    stalledLong,
    currentTimeRef,
    isFullscreenRef,
    seekTo,
    seekBy,
    togglePlay,
    toggleFullscreen,
    onTimeUpdate,
    bumpControls,
  }
}
