import type { PropsWithChildren } from 'react'
import { Gear } from '@phosphor-icons/react'
import { Link, useLocation, useNavigate } from 'react-router-dom'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'
import { useT } from '../lib/i18n'

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
 *
 * Muestra un icono de engranaje que navega a `/settings` en todas las
 * vistas EXCEPTO Home (donde ya hay un botón "Ajustes" explícito) y la
 * propia vista de Ajustes (evita el "botón que va a la página actual").
 */
export function TopNav({ children }: PropsWithChildren) {
  const t = useT()
  const isMac =
    typeof navigator !== 'undefined' &&
    navigator.platform.toLowerCase().includes('mac')

  const location = useLocation()
  const nav = useNavigate()
  const showGear = location.pathname !== '/' && location.pathname !== '/settings'

  // Hotkey global "," (coma) para saltar a Ajustes desde cualquier vista
  // que monte el TopNav. Se registra aquí para no tener que replicarla
  // en el array de hotkeys de cada view. La convención "," proviene de
  // Cmd+, en macOS.
  const gearHotkey: Hotkey[] = showGear
    ? [{ key: ',', hint: '', run: () => nav('/settings') }]
    : []
  useHotkeys(gearHotkey, [showGear])

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
          aria-label={t('nav.home')}
        >
          videodrome
        </Link>
        <nav className="flex items-center gap-3 text-[14px] text-muted">
          {children}
          {showGear && (
            <button
              onClick={() => nav('/settings')}
              aria-label={t('nav.settings')}
              title={`${t('nav.settings')} (,)`}
              className="focus-ring flex h-9 w-9 items-center justify-center rounded-full border border-hairline text-body transition-colors hover:border-border-strong hover:text-ink"
            >
              <Gear size={16} weight="bold" />
            </button>
          )}
        </nav>
      </div>
    </header>
  )
}

