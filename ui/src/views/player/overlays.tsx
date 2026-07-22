import { DownloadSimple, SpeakerHigh, SpeakerNone, SpeakerX } from '@phosphor-icons/react'
import { useT } from '../../lib/i18n'
import type { StreamStats } from '../../lib/api'
import type { VolumeHudValue } from './useMediaControls'
import { StremioLoader } from './StremioLoader'

/**
 * Overlays y HUDs del Player que NO tocan estado del reproductor
 * (video/hls/stream/subs). Todos reciben lo que necesitan por props
 * — puro renderizado + i18n. Extraídos de `Player.tsx` para bajar
 * el tamaño del componente y facilitar tests unitarios.
 */

/**
 * Loader adaptativo:
 * - **Full StremioLoader** (backdrop + logo + spinner + stats) para
 *   arranque, cambio de audio o stall > 2s. La espera es
 *   suficientemente larga como para justificar información completa.
 * - **Ligero** (spinner sobre el frame actual) para seeks / rebuffers
 *   cortos (<2s). Menos ruidoso, respeta la sensación de "esto va
 *   rápido".
 * - **Nada** si no hay motivo (`showAny=false`) o si estamos en
 *   pantalla de error.
 *
 * Los stats (speed/peers/%) SIEMPRE se pintan cuando NO es seek ni
 * audio switch — es la única señal honesta de si el enjambre está
 * vivo o muerto (audit §3.b).
 */
export interface PlayerLoaderProps {
  error: string | null
  stream: unknown | null
  hasStartedPlayback: boolean
  seeking: boolean
  audioSwitching: boolean
  buffering: boolean
  stalledLong: boolean
  title: string
  backdropUrl: string | null
  logoUrl: string | null
  stats: StreamStats | null
}

export function PlayerLoader(props: PlayerLoaderProps) {
  const {
    error,
    stream,
    hasStartedPlayback,
    seeking,
    audioSwitching,
    buffering,
    stalledLong,
    title,
    backdropUrl,
    logoUrl,
    stats,
  } = props

  if (error) return null

  const showAny =
    !stream || !hasStartedPlayback || seeking || audioSwitching || buffering
  if (!showAny) return null

  const showFull =
    !stream || !hasStartedPlayback || audioSwitching || stalledLong

  if (showFull) {
    return (
      <StremioLoader
        title={title}
        backdropUrl={backdropUrl}
        logoUrl={logoUrl}
        stats={!seeking && !audioSwitching ? stats : null}
      />
    )
  }

  // Modo ligero: solo spinner sobre el frame del video, sin fondo
  // opaco — el user sigue viendo la peli detrás.
  return (
    <div className="pointer-events-none absolute inset-0 z-30 flex items-center justify-center">
      <div className="rounded-full bg-black/50 p-3 backdrop-blur-sm">
        <div className="h-8 w-8 animate-spin rounded-full border-2 border-white/20 border-t-white" />
      </div>
    </div>
  )
}

/**
 * HUD flotante que aparece durante 1.5s cuando el user ajusta el
 * sync del sub con las hotkeys `[` `]` `,` `.`. Se controla desde el
 * hook `useSubtitles` (nulo cuando no está visible).
 */
export function SyncHud({ text }: { text: string | null }) {
  if (!text) return null
  return (
    <div className="pointer-events-none absolute left-1/2 top-16 -translate-x-1/2 rounded-md bg-black/80 px-4 py-2 text-[13px] text-ink">
      {text}
    </div>
  )
}

/**
 * HUD del volumen — se activa ~1.2s tras pulsar ArrowUp/Down/`m`.
 * Estilo pill glassy con icono Phosphor + porcentaje tabular +
 * barra de progreso. La zona por encima del 100% (boost, hasta
 * 200%) se pinta con el color accent para señalar que estamos
 * amplificando fuera del rango nativo del `<video>`.
 *
 *   [🔊]  130%    ▬▬▬▬▬│▬▬
 *                  100%  ↑ zona accent
 *
 * Reglas de estilo (design.md del proyecto, anti-slop):
 *   * Sin emojis en el texto (icono Phosphor sí).
 *   * Sin em-dashes.
 *   * Un solo accent para el boost.
 *   * Backdrop-blur para integrarse con la vista del vídeo detrás.
 */
export function VolumeHud({ value }: { value: VolumeHudValue | null }) {
  if (!value) return null
  const { volume, muted } = value
  const pct = Math.round(volume * 100)
  const boosting = !muted && volume > 1.01
  const Icon = muted || volume === 0 ? SpeakerX : volume < 0.5 ? SpeakerNone : SpeakerHigh
  // Bar: 0-200%, 100% cae al centro. Neutro < 100%, accent >= 100%.
  const barPct = Math.min(100, pct / 2) // 0-200 → 0-100 (ancho relativo)
  return (
    <div
      className="pointer-events-none absolute right-6 top-6 flex items-center gap-3 rounded-full border border-white/10 bg-black/55 px-3.5 py-2 backdrop-blur-md shadow-[0_4px_20px_rgba(0,0,0,0.35)] transition-opacity"
      role="status"
      aria-live="polite"
    >
      <Icon size={16} weight="fill" className="text-ink/90" />
      <span className="min-w-[3ch] text-right font-mono text-[12px] tabular-nums leading-none text-ink">
        {pct}%
      </span>
      <div className="relative h-[3px] w-24 overflow-hidden rounded-full bg-white/12">
        <div
          className={`absolute inset-y-0 left-0 rounded-full transition-[width] duration-150 ${
            boosting ? 'bg-accent' : muted ? 'bg-white/30' : 'bg-white/80'
          }`}
          style={{ width: `${barPct}%` }}
        />
        {/* Marca del 100% (centro del rango 0-200%). Solo visible
         *  cuando estamos cerca o pasando el umbral, para no
         *  ensuciar la barra en uso normal. */}
        <div className="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-white/25" />
      </div>
    </div>
  )
}

/**
 * Pantalla de error del Player (fatal): mensaje + botón Volver. Se
 * pinta a pantalla completa cuando algo del pipeline (probe, hls.js,
 * <video>, drops del stream) da un error irrecuperable.
 */
export function ErrorOverlay({
  error,
  onBack,
}: {
  error: string | null
  onBack: () => void
}) {
  const t = useT()
  if (!error) return null
  return (
    <div className="absolute inset-0 flex items-center justify-center bg-black/80 px-6">
      <div className="max-w-md text-center">
        <p className="text-[15px] text-body">{error}</p>
        <button
          onClick={onBack}
          className="mt-4 rounded-sm border border-hairline bg-surface px-4 py-2 text-[13px] hover:bg-surface-hi"
        >
          {t('common.back')}
        </button>
      </div>
    </div>
  )
}

/**
 * Overlay grande "Suelta para añadir subtítulos" mientras el user
 * arrastra un fichero sobre la ventana del player. `pointer-events-none`
 * es OBLIGATORIO — si capturáramos eventos aquí, Tauri no vería los
 * `onDragDropEvent` nativos.
 */
export function SubDragOverlay({ active }: { active: boolean }) {
  const t = useT()
  if (!active) return null
  return (
    <div className="pointer-events-none absolute inset-0 z-40 flex items-center justify-center bg-black/60 backdrop-blur-sm animate-drop-in">
      <div className="mx-6 flex max-w-md flex-col items-center gap-4 rounded-2xl border-2 border-dashed border-accent/70 bg-accent/10 px-10 py-8 shadow-[0_20px_60px_-20px_rgba(0,0,0,0.7)]">
        <div className="flex h-14 w-14 items-center justify-center rounded-full bg-accent/20 text-accent animate-bounce-slow">
          <DownloadSimple size={28} weight="bold" />
        </div>
        <p className="text-[16px] font-semibold text-ink text-center">
          {t('player.subDropTitle')}
        </p>
        <p className="text-[12px] text-muted text-center">
          {t('player.subDropHint')}
        </p>
      </div>
    </div>
  )
}

/**
 * Flash verde (800ms) alrededor del video tras un drop exitoso de
 * sub. Se auto-oculta desde el handler que lo activa.
 */
export function SubDragFlash({ active }: { active: boolean }) {
  if (!active) return null
  return (
    <div className="pointer-events-none absolute inset-0 z-40 animate-drop-flash rounded-lg ring-4 ring-good/70" />
  )
}

/**
 * Toast transitorio (2.5s) cuando el user suelta un fichero que no
 * es un sub reconocible. Vive DENTRO del player (no usa el `<Toast>`
 * global) para que se vea en modo fullscreen.
 */
export function SubDragErrorToast({ message }: { message: string | null }) {
  if (!message) return null
  return (
    <div className="pointer-events-none absolute left-1/2 bottom-28 z-40 -translate-x-1/2 rounded-full bg-black/85 px-5 py-2.5 text-[13px] text-ink shadow-lg">
      {message}
    </div>
  )
}
