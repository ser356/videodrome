import type { Hotkey } from '../lib/hotkeys'

/**
 * Sticky bottom bar with the active hotkey hints. Each entry shows an
 * icon (Phosphor) + label. The raw keyboard key is exposed as a `title`
 * for hover tooltip, not as a visible keycap.
 */
export function HotkeyBar({ hotkeys }: { hotkeys: Hotkey[] }) {
  return (
    <div className="glass sticky bottom-0 z-30 rounded-none">
      <div className="mx-auto flex max-w-[1400px] flex-wrap items-center gap-x-6 gap-y-1 px-8 py-2.5 text-[13px] text-body">
        {hotkeys.map((hk) => (
          <span
            key={hk.key + hk.hint}
            title={`Atajo: ${formatKey(hk.key)}`}
            className="inline-flex items-center gap-2"
          >
            <span className="flex h-5 w-5 items-center justify-center text-accent">
              {hk.icon ?? (
                <span className="text-[13px] font-semibold">
                  {formatKey(hk.key)}
                </span>
              )}
            </span>
            <span>{hk.hint}</span>
          </span>
        ))}
      </div>
    </div>
  )
}

function formatKey(key: string): string {
  switch (key) {
    case 'ArrowUp':
      return '↑'
    case 'ArrowDown':
      return '↓'
    case 'ArrowLeft':
      return '←'
    case 'ArrowRight':
      return '→'
    case 'Escape':
      return 'Esc'
    case 'Enter':
      return '⏎'
    case ' ':
      return 'Space'
    default:
      return key.length === 1 ? key.toUpperCase() : key
  }
}
