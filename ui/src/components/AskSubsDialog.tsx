import { useState } from 'react'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Diálogo previo a lanzar el stream: "¿Cargar subtítulos?".
 *
 * Foco entre botones con `←/→` (y `h/l` estilo vim, `j/k` también).
 * `Enter` confirma el botón enfocado. `S` salta directo a "con subs"
 * y `N` a "sin subs" para quien no quiera mover el foco. `Esc` cancela.
 */
export function AskSubsDialog({
  title,
  onYes,
  onNo,
  onClose,
}: {
  title: string
  onYes: () => void
  onNo: () => void
  onClose: () => void
}) {
  const [focus, setFocus] = useState<'yes' | 'no'>('yes')

  const confirm = () => {
    if (focus === 'yes') onYes()
    else onNo()
  }

  const hotkeys: Hotkey[] = [
    { key: 'ArrowLeft', hint: '', run: () => setFocus('yes') },
    { key: 'ArrowRight', hint: '', run: () => setFocus('no') },
    { key: 'ArrowUp', hint: '', run: () => setFocus('yes') },
    { key: 'ArrowDown', hint: '', run: () => setFocus('no') },
    { key: 'h', hint: '', run: () => setFocus('yes') },
    { key: 'k', hint: '', run: () => setFocus('yes') },
    { key: 'l', hint: '', run: () => setFocus('no') },
    { key: 'j', hint: '', run: () => setFocus('no') },
    { key: 'Enter', hint: '', run: confirm },
    { key: 's', hint: '', run: onYes },
    { key: 'n', hint: '', run: onNo },
    { key: 'Escape', hint: '', run: onClose, ignoreInInput: false },
  ]
  useHotkeys(hotkeys, [focus])

  return (
    <div
      onClick={onClose}
      className="fixed inset-0 z-50 flex items-center justify-center bg-scrim/70 backdrop-blur-sm"
      role="dialog"
      aria-label="¿Cargar subtítulos?"
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="glass-strong flex w-full max-w-[520px] flex-col gap-5 rounded-xl p-6"
      >
        <header>
          <p className="text-[11px] uppercase tracking-wide text-dim">
            Antes de proyectar
          </p>
          <h2 className="mt-1 text-[18px] font-semibold text-ink">
            ¿Cargar subtítulos?
          </h2>
          {title && (
            <p className="mt-1 truncate text-[12px] text-muted">{title}</p>
          )}
        </header>

        <div className="grid grid-cols-2 gap-3">
          <button
            onClick={onYes}
            onMouseEnter={() => setFocus('yes')}
            className={`flex flex-col items-start gap-1 rounded-lg border px-4 py-3 text-left transition-colors ${
              focus === 'yes'
                ? 'border-accent bg-accent/20 ring-2 ring-accent/40'
                : 'border-accent/30 bg-accent/5 hover:bg-accent/10'
            }`}
          >
            <span className="text-[14px] font-semibold text-accent">
              Con subtítulos
            </span>
            <span className="text-[11px] text-muted">
              Buscar en OpenSubtitles y elegir
            </span>
          </button>

          <button
            onClick={onNo}
            onMouseEnter={() => setFocus('no')}
            className={`flex flex-col items-start gap-1 rounded-lg border px-4 py-3 text-left transition-colors ${
              focus === 'no'
                ? 'border-border-strong bg-surface-hi ring-2 ring-white/20'
                : 'border-hairline bg-surface hover:border-border-strong hover:bg-surface-hi'
            }`}
          >
            <span className="text-[14px] font-semibold text-ink">
              Sin subtítulos
            </span>
            <span className="text-[11px] text-muted">
              Streamear directo, audio original
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
