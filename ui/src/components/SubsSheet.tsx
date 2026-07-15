import { useEffect, useRef, useState } from 'react'
import type { Subtitle } from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Modal (sheet centrada) para elegir subtítulo de OpenSubtitles.
 * Se activa desde la vista Torrents con `x`. Hotkeys internas:
 * j/k mover, Enter descarga, Esc cierra.
 */
export function SubsSheet({
  subs,
  loading,
  onPick,
  onClose,
}: {
  subs: Subtitle[]
  loading: boolean
  onPick: (sub: Subtitle) => void
  onClose: () => void
}) {
  const [sel, setSel] = useState(0)
  const rowsRef = useRef<Array<HTMLLIElement | null>>([])

  useEffect(() => {
    setSel(0)
  }, [subs])

  useEffect(() => {
    rowsRef.current[sel]?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [sel])

  const move = (delta: number) => {
    if (subs.length === 0) return
    setSel((i) => (i + delta + subs.length) % subs.length)
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: '', run: () => move(1) },
    { key: 'ArrowDown', hint: '', run: () => move(1) },
    { key: 'k', hint: '', run: () => move(-1) },
    { key: 'ArrowUp', hint: '', run: () => move(-1) },
    { key: 'Enter', hint: '', run: () => subs[sel] && onPick(subs[sel]) },
    { key: 'Escape', hint: '', run: onClose, ignoreInInput: false },
  ]
  useHotkeys(hotkeys, [subs, sel])

  return (
    <div
      onClick={onClose}
      className="fixed inset-0 z-50 flex items-center justify-center bg-scrim/70 backdrop-blur-sm"
      role="dialog"
      aria-label="Subtítulos"
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="glass-strong flex max-h-[80vh] w-full max-w-[720px] flex-col rounded-xl"
      >
        <header className="flex items-center justify-between border-b border-hairline px-5 py-3">
          <h2 className="text-[15px] font-semibold text-ink">
            Elegir subtítulo
          </h2>
          <button
            onClick={onClose}
            className="focus-ring rounded-full px-3 py-1 text-[12px] text-muted hover:bg-surface-hi"
          >
            Esc
          </button>
        </header>

        {loading ? (
          <div className="flex-1 p-10 text-center text-[14px] text-muted">
            Buscando…
          </div>
        ) : subs.length === 0 ? (
          <div className="flex-1 p-10 text-center text-[14px] text-muted">
            No hay subtítulos para este release en tus idiomas por defecto.
          </div>
        ) : (
          <ul className="flex-1 overflow-y-auto">
            {subs.map((sub, i) => (
              <li
                key={sub.file_id}
                ref={(el: HTMLLIElement | null) => { rowsRef.current[i] = el }}
                onClick={() => setSel(i)}
                onDoubleClick={() => onPick(sub)}
                className={`flex cursor-pointer items-center gap-3 border-b border-hairline-soft px-5 py-3 transition-colors ${
                  i === sel ? 'bg-surface-hi text-ink' : 'text-body hover:bg-canvas'
                }`}
              >
                <span
                  className={`text-[11px] ${
                    i === sel ? 'text-accent' : 'text-dim'
                  }`}
                >
                  {i === sel ? '▶' : ''}
                </span>
                <span className="w-9 rounded-md border border-hairline bg-canvas px-1.5 py-0.5 text-center text-[11px] font-medium uppercase tracking-wide text-info">
                  {sub.language}
                </span>
                <span className="flex-1 truncate text-[13px]">
                  {sub.release || sub.file_name || `sub-${sub.file_id}`}
                </span>
                <span className="text-[11px] tabular-nums text-good">
                  ↓ {formatShort(sub.downloads)}
                </span>
                {sub.hearing_impaired && (
                  <span className="rounded-full border border-info/40 bg-info/10 px-2 py-0.5 text-[10px] text-info">
                    SDH
                  </span>
                )}
              </li>
            ))}
          </ul>
        )}

        <footer className="border-t border-hairline px-5 py-2 text-[11px] text-dim">
          <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[11px] text-body">j/k</kbd> mover ·{' '}
          <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[11px] text-body">⏎</kbd> descargar ·{' '}
          <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[11px] text-body">Esc</kbd> cerrar
        </footer>
      </div>
    </div>
  )
}

function formatShort(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`
  return String(n)
}
