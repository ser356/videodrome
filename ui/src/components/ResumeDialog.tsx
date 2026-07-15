import { useState } from 'react'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Diálogo estilo Stremio: "Vimos que dejaste esta peli a la mitad,
 * ¿reanudar o empezar de cero?".
 *
 * Se muestra solo cuando la caché de streams tiene un `resume.json`
 * para el infohash del magnet Y conocemos el runtime de TMDB (para
 * convertir la fracción a segundos y pasarlos como `--start-time` a
 * VLC). En búsquedas directas no aparece porque runtime es `null`.
 *
 * Foco entre botones con `←/→`. `Enter` confirma. `R` reanuda, `Z`
 * empieza de cero. `Esc` cancela y no arranca stream.
 */
export function ResumeDialog({
  fraction,
  seconds,
  onResume,
  onRestart,
  onClose,
}: {
  fraction: number
  seconds: number
  onResume: () => void
  onRestart: () => void
  onClose: () => void
}) {
  const [focus, setFocus] = useState<'resume' | 'restart'>('resume')

  const confirm = () => {
    if (focus === 'resume') onResume()
    else onRestart()
  }

  const hotkeys: Hotkey[] = [
    { key: 'ArrowLeft', hint: '', run: () => setFocus('resume') },
    { key: 'ArrowRight', hint: '', run: () => setFocus('restart') },
    { key: 'ArrowUp', hint: '', run: () => setFocus('resume') },
    { key: 'ArrowDown', hint: '', run: () => setFocus('restart') },
    { key: 'h', hint: '', run: () => setFocus('resume') },
    { key: 'l', hint: '', run: () => setFocus('restart') },
    { key: 'Enter', hint: '', run: confirm },
    { key: 'r', hint: '', run: onResume },
    { key: 'z', hint: '', run: onRestart },
    { key: 'Escape', hint: '', run: onClose, ignoreInInput: false },
  ]
  useHotkeys(hotkeys, [focus])

  const pct = Math.round(fraction * 100)
  const stamp = formatHms(seconds)

  return (
    <div
      onClick={onClose}
      className="fixed inset-0 z-50 flex items-center justify-center bg-scrim/70 backdrop-blur-sm"
      role="dialog"
      aria-label="Reanudar reproducción"
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="glass-strong flex w-full max-w-[520px] flex-col gap-5 rounded-xl p-6"
      >
        <header>
          <p className="text-[11px] uppercase tracking-wide text-dim">
            Ya viste parte de esta peli
          </p>
          <h2 className="mt-1 text-[18px] font-semibold text-ink">
            ¿Reanudar donde lo dejaste?
          </h2>
          <p className="mt-2 text-[12px] text-muted">
            Progreso guardado:{' '}
            <span className="text-body">{stamp}</span>{' '}
            <span className="text-dim">({pct}%)</span>
          </p>
        </header>

        <div className="grid grid-cols-2 gap-3">
          <button
            onClick={onResume}
            onMouseEnter={() => setFocus('resume')}
            className={`flex flex-col items-start gap-1 rounded-lg border px-4 py-3 text-left transition-colors ${
              focus === 'resume'
                ? 'border-accent bg-accent/20 ring-2 ring-accent/40'
                : 'border-accent/30 bg-accent/5 hover:bg-accent/10'
            }`}
          >
            <span className="text-[14px] font-semibold text-accent">
              Reanudar
            </span>
            <span className="text-[11px] text-muted">
              VLC arranca en {stamp}
            </span>
          </button>

          <button
            onClick={onRestart}
            onMouseEnter={() => setFocus('restart')}
            className={`flex flex-col items-start gap-1 rounded-lg border px-4 py-3 text-left transition-colors ${
              focus === 'restart'
                ? 'border-border-strong bg-surface-hi ring-2 ring-white/20'
                : 'border-hairline bg-surface hover:border-border-strong hover:bg-surface-hi'
            }`}
          >
            <span className="text-[14px] font-semibold text-ink">
              Empezar de cero
            </span>
            <span className="text-[11px] text-muted">
              Ignorar el progreso anterior
            </span>
          </button>
        </div>

        <footer className="border-t border-hairline pt-3 text-[11px] text-dim">
          <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[11px] text-body">
            ← / →
          </kbd>{' '}
          mover ·{' '}
          <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[11px] text-body">
            ⏎
          </kbd>{' '}
          confirmar ·{' '}
          <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[11px] text-body">
            Esc
          </kbd>{' '}
          cancelar
        </footer>
      </div>
    </div>
  )
}

/** Formatea segundos como `H:MM:SS` (o `MM:SS` para <1h). */
function formatHms(total: number): string {
  const s = Math.max(0, Math.floor(total))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  const pad = (n: number) => n.toString().padStart(2, '0')
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`
}
