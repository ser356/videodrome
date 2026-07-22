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

  // HUD de volumen (feedback visual estilo VLC/Stremio cuando el
  // user toca ArrowUp/Down o `m` mute). `null` cuando no está
  // visible. `bumpVolumeHud` lo activa por 1.2s con auto-clear.
  volumeHud: VolumeHudValue | null
  bumpVolumeHud: () => void

  // Callbacks.
  seekTo: (absoluteSeconds: number) => void
  seekBy: (delta: number) => void
  togglePlay: () => void
  toggleFullscreen: () => Promise<void>
  onTimeUpdate: () => void
  bumpControls: () => void
}

/** Payload del HUD flotante de volumen. */
export interface VolumeHudValue {
  volume: number
  muted: boolean
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
  //
  // Rango del slider extendido a 0..=2.0 (VLC / Stremio permiten
  // "amplificar" el audio hasta 200% para pelis con mezcla muy baja).
  // El `HTMLMediaElement.volume` está capado por spec a [0, 1]; para
  // valores >1 enrutamos el audio del `<video>` a través de un
  // `AudioContext` con un `GainNode`:
  //
  //   * volume ≤ 1.0 → path nativo (`v.volume = volume`, gain = 1).
  //   * volume > 1.0 → `v.volume = 1`, `gain.gain = volume`.
  //
  // El grafo Web Audio se cablea perezosamente la PRIMERA vez que el
  // user sube de 1.0 — así los streams que se conforman con 0-100%
  // no pagan overhead ni riesgo de CORS-taint. `MediaElementSource`
  // solo puede crearse una vez por element; guardamos el nodo en un
  // ref idempotente. AudioContext arranca "suspended" hasta un user
  // gesture (autoplay policy); mover el slider ES un gesture, así
  // que el `.resume()` posterior funciona.
  const audioCtxRef = useRef<AudioContext | null>(null)
  const gainNodeRef = useRef<GainNode | null>(null)
  const audioGraphFailedRef = useRef(false)

  const ensureAudioGraph = useCallback(() => {
    if (audioCtxRef.current || audioGraphFailedRef.current) return
    const v = videoRef.current
    if (!v) return
    try {
      const Ctx =
        window.AudioContext ??
        (window as unknown as { webkitAudioContext?: typeof AudioContext })
          .webkitAudioContext
      if (!Ctx) {
        audioGraphFailedRef.current = true
        return
      }
      // El swap nativo → Web Audio es transparente EN LOUDNESS si
      // inicializamos el gain al mismo nivel que tenía `v.volume`
      // justo antes del swap: `createMediaElementSource` desconecta
      // la salida nativa del element, así que a partir de aquí todo
      // el audio pasa por gain·v.volume. Fijamos `v.volume = 1` y
      // dejamos que el gain haga todo el control — así el punto de
      // partida audible coincide con el estado previo (sin click ni
      // silencio de arranque).
      //
      // Intentos previos con `gain = 0` + ramp fallaban: si el ctx
      // nace suspended (autoplay policy), el clock no avanza y el
      // ramp no se aplica hasta que `resume()` resuelve async → el
      // user oía silencio en la primera pulsación de ArrowUp.
      const prevVolume = v.volume
      const ctx: AudioContext = new Ctx()
      const src = ctx.createMediaElementSource(v)
      const gain = ctx.createGain()
      gain.gain.setValueAtTime(prevVolume, ctx.currentTime)
      src.connect(gain).connect(ctx.destination)
      audioCtxRef.current = ctx
      gainNodeRef.current = gain
      v.volume = 1
    } catch (err) {
      // `createMediaElementSource` tira `InvalidStateError` si ya se
      // llamó antes sobre este mismo elemento (p.ej. hot-reload en
      // dev). Marcamos como fallido para no reintentar en cada
      // slider tick — el boost queda desactivado esta sesión, resto
      // del player intacto.
      console.warn('[audio] boost graph init failed:', err)
      audioGraphFailedRef.current = true
    }
  }, [videoRef])

  useEffect(() => {
    const v = videoRef.current
    if (!v) return
    // Cuando el grafo Web Audio está activo, `HTMLMediaElement.muted`
    // NO garantiza silencio en WKWebView (el audio va por el
    // MediaElementSource → GainNode → destination; muted del element
    // no siempre se propaga). Cortamos también con el gain para
    // silencio inmediato — sin esto el mute se sentía "con delay"
    // porque el audio seguía saliendo por el pipeline de Web Audio.
    //
    // Path activo: gain controla TODO el nivel (0..2.0). Path nativo:
    // `v.volume` controla nivel (0..1.0), sin amplificación posible.
    const effectiveGain = muted ? 0 : volume
    if (volume > 1.0 && !muted) {
      ensureAudioGraph()
    }
    const gain = gainNodeRef.current
    const ctx = audioCtxRef.current
    if (gain && ctx) {
      // El element queda como pass-through — gain hace el control.
      v.volume = 1
      // Ramp suave (30 ms) al nivel objetivo. Evita saltos bruscos
      // al mover el slider rápido o al togglear mute. El gain
      // arranca al nivel previo tras el swap (ver ensureAudioGraph),
      // así que el primer ramp es de prevVolume → volume, sin
      // silencio ni click.
      const now = ctx.currentTime
      gain.gain.cancelScheduledValues(now)
      gain.gain.setValueAtTime(gain.gain.value, now)
      gain.gain.linearRampToValueAtTime(effectiveGain, now + 0.03)
    } else {
      // Sin grafo Web Audio (todavía no cableado o falló): path
      // nativo. Cap a 1.0 si el user pidió más — no podemos amplificar.
      v.volume = Math.min(volume, 1.0)
    }
    // `v.muted` también para consistencia con el UI nativo del
    // sistema (indicador OS X en el menu bar, etc.).
    v.muted = muted
    // Resume del AudioContext si arrancó suspended (autoplay policy).
    if (ctx && ctx.state === 'suspended') {
      void ctx.resume().catch(() => {})
    }
  }, [volume, muted, videoRef, ensureAudioGraph])

  // Cleanup del AudioContext al desmontar. Sin esto el ctx queda vivo
  // referenciando el `<video>` viejo → leak de recursos de audio y
  // (en algunos WebViews) sonido zombi al reabrir el player.
  useEffect(() => {
    return () => {
      const ctx = audioCtxRef.current
      if (ctx) {
        void ctx.close().catch(() => {})
        audioCtxRef.current = null
        gainNodeRef.current = null
      }
    }
  }, [])

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

  // HUD de volumen — se activa al pulsar ArrowUp/Down/`m` o al usar
  // el botón de mute del UI, y refleja el estado NUEVO tras la
  // actualización de React. Auto-clear tras 1.2 s.
  //
  // Design: en vez de leer `volume`/`muted` cuando `bumpVolumeHud`
  // se llama (que llegaría STALE porque los setters de React son
  // asíncronos y el effect que actualiza refs corre DESPUÉS del
  // render), el callback solo levanta un flag `pendingHudRef`. Un
  // `useEffect` que depende de `[volume, muted]` — y por tanto
  // corre TRAS el commit — resuelve la bandera y snapshotea el
  // valor fresco. Así el HUD siempre coincide con el estado real.
  //
  // Nota: el slider (drag) NO llama a `bumpVolumeHud` — sólo los
  // hotkeys y el botón de mute UI. Si lo llamase el slider, el HUD
  // flashearía en cada tick de mouse durante el drag, ruidoso.
  const [volumeHud, setVolumeHud] = useState<VolumeHudValue | null>(null)
  const volumeHudTimerRef = useRef<number | null>(null)
  const pendingHudRef = useRef(false)
  const bumpVolumeHud = useCallback(() => {
    pendingHudRef.current = true
  }, [])
  useEffect(() => {
    if (!pendingHudRef.current) return
    pendingHudRef.current = false
    // eslint-disable-next-line react-hooks/set-state-in-effect -- Flash del HUD tras cambio de volumen/mute vía hotkey/botón.
    setVolumeHud({ volume, muted })
    if (volumeHudTimerRef.current) window.clearTimeout(volumeHudTimerRef.current)
    volumeHudTimerRef.current = window.setTimeout(() => {
      setVolumeHud(null)
      volumeHudTimerRef.current = null
    }, 1200)
  }, [volume, muted])
  // Cleanup del timer al desmontar.
  useEffect(() => {
    return () => {
      if (volumeHudTimerRef.current) {
        window.clearTimeout(volumeHudTimerRef.current)
      }
    }
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
    volumeHud,
    bumpVolumeHud,
    seekTo,
    seekBy,
    togglePlay,
    toggleFullscreen,
    onTimeUpdate,
    bumpControls,
  }
}
