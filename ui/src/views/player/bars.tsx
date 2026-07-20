import {
  ArrowsIn,
  ArrowsOut,
  CaretLeft,
  ClosedCaptioning,
  Gauge,
  MusicNotes,
  Pause,
  Play,
} from '@phosphor-icons/react'
import type { RefObject } from 'react'
import { useT } from '../../lib/i18n'
import type { MediaStream } from '../../lib/api'
import { SeekBar, VolumeControl } from './controls'
import { formatTime } from './utils'
import type { ActiveSub } from './useSubtitles'

/**
 * Barras del Player (top + control). Puras presentacionales — todo
 * el estado llega por props, todos los eventos suben por callbacks.
 * Extraídas de `Player.tsx` para bajar el tamaño del componente y
 * facilitar tests visuales aislados.
 */

// ── Top bar ─────────────────────────────────────────────────────

export interface PlayerTopBarProps {
  title: string
  subRelease: string | null
  isSeries: boolean
  season: number | null
  episode: number | null
  tmdbId: number | null
  duration: number | null
  currentTime: number
  controlsVisible: boolean
  onBack: () => void
  /** Se llama al pulsar "siguiente episodio" (visible solo cuando
   * `currentTime/duration > 0.9` y hay tmdbId + S/E). Recibe el
   * número del episodio siguiente (E+1). */
  onNextEpisode: (nextEpisode: number) => void
}

export function PlayerTopBar(props: PlayerTopBarProps) {
  const t = useT()
  const {
    title,
    subRelease,
    isSeries,
    season,
    episode,
    tmdbId,
    duration,
    currentTime,
    controlsVisible,
    onBack,
    onNextEpisode,
  } = props

  const showNextEpisode =
    isSeries &&
    tmdbId != null &&
    season != null &&
    episode != null &&
    duration != null &&
    duration > 0 &&
    currentTime / duration > 0.9

  return (
    <>
      <div
        className={`pointer-events-none absolute inset-x-0 top-0 h-32 bg-gradient-to-b from-black/80 to-transparent transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0'
        }`}
      />
      {/* Drag region invisible SIEMPRE activa en el borde superior.
          `titleBarStyle: Overlay + hiddenTitle: true` esconde la
          barra del sistema, así que sin este strip la ventana no se
          puede arrastrar en modo player (el `<video>` ocupa el
          100%). Altura 28px = alto de los semaphore buttons en
          macOS; queda debajo del back button (que arranca en pt-5,
          y=20, con h-9 = ocupa hasta y=56 y captura sus clicks al
          estar en z-index mayor por orden DOM). Persiste con
          controles ocultos — no depende de `controlsVisible`. */}
      <div
        data-tauri-drag-region
        className="absolute inset-x-0 top-0 z-10 h-7"
        aria-hidden="true"
      />
      <div
        data-tauri-drag-region
        className={`absolute inset-x-0 top-0 z-20 flex items-center gap-3 px-5 pt-5 transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0 pointer-events-none'
        }`}
        onClick={(e) => e.stopPropagation()}
      >
        <button
          onClick={onBack}
          className="flex h-9 w-9 items-center justify-center rounded-full bg-black/40 text-ink hover:bg-black/60"
          title={t('player.backTitle')}
        >
          <CaretLeft size={18} weight="bold" />
        </button>
        <div className="min-w-0 flex-1">
          <p className="truncate text-[15px] font-medium text-ink">
            {title}
            {isSeries && season != null && episode != null && (
              <span className="ml-2 text-[12px] font-normal text-muted">
                · S{String(season).padStart(2, '0')}E
                {String(episode).padStart(2, '0')}
              </span>
            )}
          </p>
          {subRelease && (
            <p className="truncate text-[12px] text-muted">
              {t('player.subs')}: {subRelease}
            </p>
          )}
        </div>
        {showNextEpisode && (
          <button
            onClick={() => onNextEpisode(episode! + 1)}
            className="rounded-full border border-accent bg-accent/10 px-3 py-1.5 text-[12px] font-semibold text-accent hover:bg-accent/20"
            title={t('player.nextEpisodeTitle')}
          >
            {t('player.nextEpisode')}
          </button>
        )}
      </div>
    </>
  )
}

// ── Control bar (bottom) ────────────────────────────────────────

export interface PlayerControlBarProps {
  videoRef: RefObject<HTMLVideoElement | null>
  currentTime: number
  duration: number | null
  paused: boolean
  volume: number
  muted: boolean
  isFullscreen: boolean
  controlsVisible: boolean
  seekHover: number | null
  setSeekHover: (v: number | null) => void
  onSeek: (absoluteSeconds: number) => void
  onTogglePlay: () => void
  onSetVolume: (v: number) => void
  onToggleMute: () => void
  onToggleFullscreen: () => void

  statsPanelOpen: boolean
  setStatsPanelOpen: (v: boolean | ((prev: boolean) => boolean)) => void

  audioTracks: MediaStream[]
  activeAudioIdx: number
  audioPanelOpen: boolean
  setAudioPanelOpen: (v: boolean | ((prev: boolean) => boolean)) => void

  activeSub: ActiveSub | null
  setSubsPanelOpen: (v: boolean | ((prev: boolean) => boolean)) => void
}

export function PlayerControlBar(props: PlayerControlBarProps) {
  const t = useT()
  const {
    videoRef,
    currentTime,
    duration,
    paused,
    volume,
    muted,
    isFullscreen,
    controlsVisible,
    seekHover,
    setSeekHover,
    onSeek,
    onTogglePlay,
    onSetVolume,
    onToggleMute,
    onToggleFullscreen,
    statsPanelOpen,
    setStatsPanelOpen,
    audioTracks,
    activeAudioIdx,
    audioPanelOpen,
    setAudioPanelOpen,
    activeSub,
    setSubsPanelOpen,
  } = props

  return (
    <>
      <div
        className={`pointer-events-none absolute inset-x-0 bottom-0 h-40 bg-gradient-to-t from-black/85 to-transparent transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0'
        }`}
      />
      <div
        className={`absolute inset-x-0 bottom-0 px-6 pb-5 pt-2 transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0 pointer-events-none'
        }`}
        onClick={(e) => e.stopPropagation()}
      >
        <SeekBar
          currentTime={currentTime}
          duration={duration}
          videoRef={videoRef}
          onSeek={onSeek}
          hover={seekHover}
          setHover={setSeekHover}
        />
        <div className="mt-3 flex items-center gap-4">
          <button
            onClick={onTogglePlay}
            className="flex h-11 w-11 items-center justify-center rounded-full bg-accent text-on-accent transition-colors hover:bg-accent-hover"
            title={paused ? t('player.playTitle') : t('player.pauseTitle')}
          >
            {paused ? <Play size={20} weight="fill" /> : <Pause size={20} weight="fill" />}
          </button>

          <VolumeControl
            volume={volume}
            muted={muted}
            onVolume={onSetVolume}
            onToggleMute={onToggleMute}
          />

          <div className="ml-2 text-[12px] tabular-nums text-body">
            {formatTime(currentTime)}
            {' / '}
            <span className="text-muted">
              {duration != null ? formatTime(duration) : '--:--'}
            </span>
          </div>

          <div className="flex-1" />

          <button
            onClick={() => setStatsPanelOpen((o) => !o)}
            className={`flex h-9 w-9 items-center justify-center rounded-full transition-colors ${
              statsPanelOpen
                ? 'bg-accent/20 text-accent'
                : 'text-ink hover:bg-surface'
            }`}
            title=""
            aria-label={t('player.stats')}
            aria-pressed={statsPanelOpen}
          >
            <Gauge size={18} weight={statsPanelOpen ? 'fill' : 'bold'} />
          </button>

          {audioTracks.length > 1 && (
            <button
              onClick={() => setAudioPanelOpen((o) => !o)}
              className={`flex h-9 items-center gap-1.5 rounded-full px-3 transition-colors ${
                audioPanelOpen
                  ? 'bg-accent/20 text-accent'
                  : 'text-ink hover:bg-surface'
              }`}
              title=""
              aria-label={t('player.audioTrack')}
              aria-pressed={audioPanelOpen}
            >
              <MusicNotes size={18} weight={audioPanelOpen ? 'fill' : 'bold'} />
              {audioTracks[activeAudioIdx]?.language && (
                <span className="text-[11px] font-medium uppercase">
                  {audioTracks[activeAudioIdx].language}
                </span>
              )}
            </button>
          )}

          <button
            onClick={() => setSubsPanelOpen((o) => !o)}
            className={`flex h-9 items-center gap-1.5 rounded-full px-3 transition-colors ${
              activeSub
                ? 'bg-accent/20 text-accent'
                : 'text-ink hover:bg-surface'
            }`}
            title={
              activeSub
                ? `${t('player.subs')}: ${activeSub.release}`
                : t('player.subtitlesTitle')
            }
          >
            <ClosedCaptioning size={18} weight={activeSub ? 'fill' : 'bold'} />
            {activeSub && (
              <span className="text-[11px] font-medium uppercase">
                {activeSub.language}
              </span>
            )}
          </button>

          <button
            onClick={onToggleFullscreen}
            className="flex h-9 w-9 items-center justify-center rounded-full text-ink hover:bg-surface"
            title={t('player.fullscreenTitle')}
          >
            {isFullscreen ? (
              <ArrowsIn size={18} weight="bold" />
            ) : (
              <ArrowsOut size={18} weight="bold" />
            )}
          </button>
        </div>
      </div>
    </>
  )
}
