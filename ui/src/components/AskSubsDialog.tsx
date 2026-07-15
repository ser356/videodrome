import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Diálogo previo a lanzar el stream: "¿Cargar subtítulos?".
 *
 * - `Enter` (o click en "Con subtítulos") abre la SubsSheet.
 * - `N` (o click en "Sin subtítulos") arranca el stream directamente.
 * - `Esc` cancela y no hace nada.
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
  const hotkeys: Hotkey[] = [
    { key: 'Enter', hint: '', run: onYes },
    { key: 'n', hint: '', run: onNo },
    { key: 'Escape', hint: '', run: onClose, ignoreInInput: false },
  ]
  useHotkeys(hotkeys, [])

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
            autoFocus
            className="focus-ring flex flex-col items-start gap-1 rounded-lg border border-accent/50 bg-accent/10 px-4 py-3 text-left transition-colors hover:bg-accent/20"
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
            className="focus-ring flex flex-col items-start gap-1 rounded-lg border border-hairline bg-surface px-4 py-3 text-left transition-colors hover:border-border-strong hover:bg-surface-hi"
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
            ⏎
          </kbd>{' '}
          con subs ·{' '}
          <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[11px] text-body">
            N
          </kbd>{' '}
          sin subs ·{' '}
          <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[11px] text-body">
            Esc
          </kbd>{' '}
          cancelar
        </footer>
      </div>
    </div>
  )
}
