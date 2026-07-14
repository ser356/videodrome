import type { PropsWithChildren } from 'react'
import { Link } from 'react-router-dom'

/**
 * Top navigation. Wordmark-only ("screener"), thin hairline, right slot
 * for the active view's controls.
 *
 * En macOS, `tauri.conf.json` usa `titleBarStyle: Overlay` para que la
 * ventana no tenga barra nativa. Los traffic lights (cerrar / minimizar
 * / maximizar) quedan flotando arriba-izquierda, así que dejamos ~86px
 * de padding-left cuando estamos en macOS para no tapar el wordmark.
 *
 * Además, el `data-tauri-drag-region` permite arrastrar la ventana
 * agarrando la barra vacía (sustituto de la titlebar nativa).
 */
export function TopNav({ children }: PropsWithChildren) {
  const isMac =
    typeof navigator !== 'undefined' &&
    navigator.platform.toLowerCase().includes('mac')

  return (
    <header
      data-tauri-drag-region
      className="glass sticky top-0 z-30 h-[56px] rounded-none"
    >
      <div
        data-tauri-drag-region
        className="mx-auto flex h-full max-w-[1400px] items-center justify-between px-8"
        style={isMac ? { paddingLeft: '86px' } : undefined}
      >
        <Link
          to="/"
          className="focus-ring rounded-md text-[17px] font-semibold tracking-tight text-ink"
          aria-label="Inicio"
        >
          videodrome
        </Link>
        <nav className="flex items-center gap-4 text-[14px] text-muted">
          {children}
        </nav>
      </div>
    </header>
  )
}

