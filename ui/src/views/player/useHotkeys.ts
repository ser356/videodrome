import type React from 'react'
import { useEffect } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { type StreamInfo } from '../../lib/api'

interface ActiveSub {
  source: 'openSubs' | 'embedded'
}

interface HotkeysOptions {
  videoRef: React.RefObject<HTMLVideoElement | null>
  isFullscreenRef: React.MutableRefObject<boolean>
  seekBy: (delta: number) => void
  toggleFullscreen: () => Promise<void>
  activeSub: ActiveSub | null
  subSpeed: number
  showSyncHud: (text: string) => void
  setVolume: React.Dispatch<React.SetStateAction<number>>
  setMuted: React.Dispatch<React.SetStateAction<boolean>>
  setSubsPanelOpen: React.Dispatch<React.SetStateAction<boolean>>
  setSubOffset: React.Dispatch<React.SetStateAction<number>>
  setSubSpeed: React.Dispatch<React.SetStateAction<number>>
  setIsFullscreen: React.Dispatch<React.SetStateAction<boolean>>
  handleBack: () => void
  stream: StreamInfo | null
}

/**
 * Hotkeys globales del player. Enganchado a `document` para funcionar
 * aunque el foco esté fuera del `<video>` (típico tras click en un
 * botón). Mapeo:
 *
 *   Space / k      Play-pause
 *   j / ArrowLeft  Seek -10s
 *   l / ArrowRight Seek +10s
 *   ArrowUp/Down   Volumen ±5%
 *   m              Mute toggle
 *   f              Fullscreen toggle
 *   c              Panel de subs toggle
 *   [ / ]          Sub offset ±0.5s (Shift → ±0.1s fino)
 *   , / .          Sub speed (rota PAL / sin cambio / NTSC)
 *   Escape         Sale de fullscreen o vuelve atrás
 *
 * Deps del efecto: `[stream, activeSub, subSpeed]` — idénticas al
 * useEffect original en Player.tsx. El resto de dependencias se leen
 * a través de refs o setter callbacks (que React garantiza estables)
 * para no re-suscribir el listener en cada render y evitar stale
 * closures.
 */
export function useHotkeys({
  videoRef,
  isFullscreenRef,
  seekBy,
  toggleFullscreen,
  activeSub,
  subSpeed,
  showSyncHud,
  setVolume,
  setMuted,
  setSubsPanelOpen,
  setSubOffset,
  setSubSpeed,
  setIsFullscreen,
  handleBack,
  stream,
}: HotkeysOptions): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // No queremos que teclear en un input dispare hotkeys.
      const target = e.target as HTMLElement | null
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return

      const v = videoRef.current
      if (!v) return
      switch (e.key) {
        case ' ':
        case 'k':
          e.preventDefault()
          if (v.paused) void v.play()
          else v.pause()
          break
        case 'j':
        case 'ArrowLeft':
          e.preventDefault()
          seekBy(-10)
          break
        case 'l':
        case 'ArrowRight':
          e.preventDefault()
          seekBy(10)
          break
        case 'ArrowUp':
          e.preventDefault()
          setVolume((x) => Math.min(1, x + 0.05))
          break
        case 'ArrowDown':
          e.preventDefault()
          setVolume((x) => Math.max(0, x - 0.05))
          break
        case 'm':
          setMuted((m) => !m)
          break
        case 'f':
          void toggleFullscreen()
          break
        case 'c':
          setSubsPanelOpen((o) => !o)
          break
        case '[':
          if (activeSub) {
            const delta = e.shiftKey ? -0.1 : -0.5
            setSubOffset((val) => {
              const next = +(val + delta).toFixed(2)
              showSyncHud(`Sub offset ${next > 0 ? '+' : ''}${next.toFixed(2)}s`)
              return next
            })
          }
          break
        case ']':
          if (activeSub) {
            const delta = e.shiftKey ? 0.1 : 0.5
            setSubOffset((val) => {
              const next = +(val + delta).toFixed(2)
              showSyncHud(`Sub offset ${next > 0 ? '+' : ''}${next.toFixed(2)}s`)
              return next
            })
          }
          break
        case ',':
        case '.': {
          if (activeSub) {
            // Rota entre 3 valores conocidos:
            //   0.95904 = 23.976/25 (sub PAL sobre video NTSC)
            //   1.00000 = sin cambio
            //   1.04271 = 25/23.976 (sub NTSC sobre video PAL)
            const steps = [0.95904, 1.0, 1.04271]
            const cur = steps.findIndex((s) => Math.abs(s - subSpeed) < 0.001)
            const dir = e.key === '.' ? 1 : -1
            const next = steps[(cur + dir + steps.length) % steps.length]
            setSubSpeed(next)
            const label =
              next === 1.0 ? 'sin cambio' : next < 1 ? 'sub PAL → NTSC' : 'sub NTSC → PAL'
            showSyncHud(`Sub speed ${next.toFixed(4)}× (${label})`)
          }
          break
        }
        case 'Escape':
          // Si estamos en fullscreen (Tauri window), salimos;
          // si no, volvemos atrás. Ref para no re-suscribir el
          // listener al entrar/salir de fullscreen.
          if (isFullscreenRef.current) {
            void getCurrentWindow().setFullscreen(false).catch(() => {})
            setIsFullscreen(false)
          } else {
            handleBack()
          }
          break
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stream, activeSub, subSpeed])
}
