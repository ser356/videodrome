import { useEffect } from 'react'
import type { ReactNode } from 'react'

export interface Hotkey {
  key: string
  hint: string
  run: () => void
  /** Icon to render in the HotkeyBar instead of the raw key symbol. */
  icon?: ReactNode
  /** Ignore this hotkey when the user is typing in a text input. Default true. */
  ignoreInInput?: boolean
}

/**
 * Bind a list of hotkeys to `window`. Hotkeys are matched by
 * `KeyboardEvent.key` (case-insensitive). Repeating keys fire once per
 * physical press (auto-repeat allowed).
 *
 * Text-input focus (INPUT / TEXTAREA / contentEditable) suppresses
 * single-letter hotkeys by default, so typing "r" in the search bar
 * doesn't accidentally trigger the "recargar" hotkey.
 */
export function useHotkeys(
  hotkeys: Hotkey[],
  deps: unknown[] = [],
  options: { enabled?: boolean } = {},
) {
  const { enabled = true } = options
  useEffect(() => {
    if (!enabled) return
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null
      const inInput =
        target &&
        (target.tagName === 'INPUT' ||
          target.tagName === 'TEXTAREA' ||
          target.isContentEditable)

      for (const hk of hotkeys) {
        if (matchesKey(e, hk.key)) {
          if (inInput && (hk.ignoreInInput ?? true) && !isNavKey(hk.key)) {
            continue
          }
          e.preventDefault()
          hk.run()
          return
        }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, enabled])
}

function matchesKey(e: KeyboardEvent, spec: string): boolean {
  // "Escape", "Enter", "ArrowUp", or a single character.
  return e.key.toLowerCase() === spec.toLowerCase()
}

/** Nav keys (arrows, Escape, Enter) fire even when a text input has focus,
 * because those are how the user submits or bails from the input itself. */
function isNavKey(spec: string): boolean {
  return (
    spec === 'ArrowUp' ||
    spec === 'ArrowDown' ||
    spec === 'ArrowLeft' ||
    spec === 'ArrowRight' ||
    spec === 'Escape' ||
    spec === 'Enter'
  )
}
