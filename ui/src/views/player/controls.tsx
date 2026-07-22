import { useEffect, useRef, useState } from 'react'
import {
  ArrowDown,
  SpeakerHigh,
  SpeakerNone,
  SpeakerX,
  UsersThree,
  X,
} from '@phosphor-icons/react'
import { formatSize, type StreamStats } from '../../lib/api'
import { useT } from '../../lib/i18n'
import { formatEta, formatTime } from './utils'

export function SeekBar({
  currentTime,
  duration,
  videoRef,
  onSeek,
  hover,
  setHover,
}: {
  currentTime: number
  duration: number | null
  videoRef: React.RefObject<HTMLVideoElement | null>
  onSeek: (t: number) => void
  hover: number | null
  setHover: (t: number | null) => void
}) {
  const barRef = useRef<HTMLDivElement | null>(null)
  const [buffered, setBuffered] = useState<[number, number][]>([])

  // Refresca los rangos bufferizados cada 500ms mientras el player
  // está montado. `TimeRanges` no dispara eventos, hay que pollear.
  // `v.buffered` está ya en tiempo absoluto (el modelo VOD no
  // remapea el timeline entre modos DIRECT y TRANSMUX).
  useEffect(() => {
    const id = window.setInterval(() => {
      const v = videoRef.current
      if (!v) return
      const rs: [number, number][] = []
      for (let i = 0; i < v.buffered.length; i++) {
        rs.push([v.buffered.start(i), v.buffered.end(i)])
      }
      setBuffered(rs)
    }, 500)
    return () => window.clearInterval(id)
  }, [videoRef])

  const timeFromEvent = (e: React.MouseEvent) => {
    const bar = barRef.current
    if (!bar || !duration) return null
    const rect = bar.getBoundingClientRect()
    const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))
    return ratio * duration
  }

  const pct = (t: number) =>
    duration && duration > 0 ? (t / duration) * 100 : 0

  return (
    <div
      ref={barRef}
      className="group relative h-6 cursor-pointer"
      onClick={(e) => {
        const t = timeFromEvent(e)
        if (t != null) onSeek(t)
      }}
      onMouseMove={(e) => setHover(timeFromEvent(e))}
      onMouseLeave={() => setHover(null)}
    >
      {/* Base track */}
      <div className="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 rounded-full bg-white/15" />
      {/* Buffered rangos */}
      {buffered.map(([s, e], i) => (
        <div
          key={i}
          className="absolute top-1/2 h-1 -translate-y-1/2 rounded-full bg-white/30"
          style={{
            left: `${pct(s)}%`,
            width: `${Math.max(0, pct(e) - pct(s))}%`,
          }}
        />
      ))}
      {/* Playhead */}
      <div
        className="absolute top-1/2 h-1 -translate-y-1/2 rounded-full bg-accent"
        style={{ width: `${pct(currentTime)}%` }}
      />
      {/* Thumb */}
      <div
        className="absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-accent shadow-md transition-transform group-hover:scale-125"
        style={{ left: `${pct(currentTime)}%` }}
      />
      {/* Hover tooltip */}
      {hover != null && duration && (
        <div
          className="pointer-events-none absolute -top-8 -translate-x-1/2 rounded-sm bg-black/85 px-2 py-1 text-[11px] tabular-nums text-ink"
          style={{ left: `${pct(hover)}%` }}
        >
          {formatTime(hover)}
        </div>
      )}
    </div>
  )
}

export function VolumeControl({
  volume,
  muted,
  onVolume,
  onToggleMute,
}: {
  volume: number
  muted: boolean
  onVolume: (v: number) => void
  onToggleMute: () => void
}) {
  const t = useT()
  const Icon = muted || volume === 0 ? SpeakerX : volume < 0.5 ? SpeakerNone : SpeakerHigh
  // Slider 0..=200% (VLC/Stremio). `useMediaControls` enruta valores
  // > 1.0 a través de un GainNode Web Audio; visualmente pintamos
  // un badge con el porcentaje cuando estamos amplificando (>100%)
  // para que el user sepa que está fuera del rango nativo.
  const pct = Math.round(volume * 100)
  const boosting = !muted && volume > 1.01
  return (
    <div className="group flex items-center gap-2">
      <button
        onClick={onToggleMute}
        className="flex h-9 w-9 items-center justify-center rounded-full text-ink hover:bg-surface"
        title={muted ? t('player.unmuteTitle') : t('player.muteTitle')}
      >
        <Icon size={18} weight="bold" />
      </button>
      <input
        type="range"
        min={0}
        max={2}
        step={0.02}
        value={muted ? 0 : volume}
        onChange={(e) => {
          const v = parseFloat(e.target.value)
          onVolume(v)
          if (v > 0 && muted) onToggleMute()
        }}
        className="h-1 w-24 cursor-pointer appearance-none rounded-full bg-white/15 accent-accent opacity-0 transition-opacity group-hover:opacity-100"
      />
      {boosting && (
        <span
          className="rounded-full bg-white/10 px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-ink/80 opacity-0 transition-opacity group-hover:opacity-100"
          title={t('player.volumeBoostTitle', { defaultValue: 'Audio amplificado' })}
        >
          {pct}%
        </span>
      )}
    </div>
  )
}

export function StatsPanel({
  stats,
  onClose,
}: {
  stats: StreamStats | null
  onClose: () => void
}) {
  const tr = useT()
  const hasProgress = stats != null && stats.total_bytes > 0
  const pct = hasProgress
    ? (stats!.progress_bytes / stats!.total_bytes) * 100
    : 0
  const remaining = hasProgress
    ? Math.max(0, stats!.total_bytes - stats!.progress_bytes)
    : 0
  const bytesPerSec = stats ? stats.down_mbps * 1024 * 1024 : 0
  const etaSec = bytesPerSec > 0 ? remaining / bytesPerSec : null

  return (
    <div
      className="absolute bottom-24 right-6 z-30 w-[280px] rounded-xl border border-white/10 bg-black/80 p-4 shadow-[0_20px_60px_-20px_rgba(0,0,0,0.6)] backdrop-blur-xl backdrop-saturate-150"
      onClick={(e) => e.stopPropagation()}
    >
      <header className="mb-3 flex items-center justify-between">
        <p className="text-[11px] uppercase tracking-[0.14em] text-dim">
          Stream
        </p>
        <button
          onClick={onClose}
          className="flex h-6 w-6 items-center justify-center rounded-full text-muted hover:bg-surface hover:text-ink"
          aria-label={tr('common.close')}
        >
          <X size={12} weight="bold" />
        </button>
      </header>

      {!stats && (
        <div className="flex items-center gap-2 py-2 text-[12px] text-muted">
          <span className="h-3 w-3 animate-spin rounded-full border-2 border-accent border-t-transparent" />
          <span>{tr('player.waitingData')}</span>
        </div>
      )}

      {stats && (
        <>
          {hasProgress && (
            <div className="mb-3 h-1 w-full overflow-hidden rounded-full bg-white/10">
              <div
                className="h-full rounded-full bg-gradient-to-r from-accent to-good transition-all duration-700 ease-out"
                style={{ width: `${Math.max(2, pct).toFixed(1)}%` }}
              />
            </div>
          )}
          <div className="grid grid-cols-2 gap-x-6 gap-y-2 text-[12px] tabular-nums text-body">
            <Metric
              icon={<ArrowDown size={13} weight="bold" />}
              label={tr('player.stat.speed')}
              value={
                <span className="text-good">
                  {stats.down_mbps.toFixed(2)}{' '}
                  <span className="text-[10px] uppercase text-dim">MiB/s</span>
                </span>
              }
            />
            <Metric
              icon={<UsersThree size={13} weight="bold" />}
              label={tr('player.stat.peers')}
              value={stats.live_peers.toString()}
            />
            <Metric
              label="ETA"
              value={etaSec != null ? formatEta(etaSec) : '—'}
            />
            <Metric
              label={tr('player.stat.progress')}
              value={hasProgress ? `${pct.toFixed(1)} %` : '—'}
            />
            {hasProgress && (
              <Metric
                label={tr('player.stat.downloaded')}
                value={
                  <span>
                    {formatSize(stats.progress_bytes)}{' '}
                    <span className="text-dim">/ {formatSize(stats.total_bytes)}</span>
                  </span>
                }
                span={2}
              />
            )}
          </div>
        </>
      )}
    </div>
  )
}

/** Métrica del HUD: label pequeña + valor con opción a icono
 * inline. `span=2` la hace ocupar toda la fila del grid. */
export function Metric({
  icon,
  label,
  value,
  span = 1,
}: {
  icon?: React.ReactNode
  label: string
  value: React.ReactNode
  span?: 1 | 2
}) {
  return (
    <div
      className={`flex flex-col items-start gap-0.5 ${span === 2 ? 'col-span-2' : ''}`}
    >
      <span className="flex items-center gap-1 text-[10px] uppercase tracking-[0.1em] text-dim">
        {icon}
        {label}
      </span>
      <span className="text-[13px] font-medium text-ink">{value}</span>
    </div>
  )
}

/** Tres puntitos animados en secuencia — imita el ellipsis de
 * carga clásico pero controlado (sin depender de fuentes/emoji). */
export function LoadingDots() {
  return (
    <span className="ml-0.5 inline-flex">
      <span
        className="inline-block animate-pulse"
        style={{ animationDelay: '0ms' }}
      >
        .
      </span>
      <span
        className="inline-block animate-pulse"
        style={{ animationDelay: '180ms' }}
      >
        .
      </span>
      <span
        className="inline-block animate-pulse"
        style={{ animationDelay: '360ms' }}
      >
        .
      </span>
    </span>
  )
}
