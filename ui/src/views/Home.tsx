import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { KeyReturn } from '@phosphor-icons/react'
import { HotkeyBar } from '../components/HotkeyBar'
import { TopNav } from '../components/TopNav'
import { hasSession, isTauri } from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Menu principal, equivalente al enum `View::Menu` de la TUI: dos
 * opciones (Recomendaciones / Búsqueda directa) navegables con j/k y
 * Enter. Si no hay sesión de Letterboxd, "Recomendaciones" redirige a
 * /login antes.
 */
const OPTIONS = [
  {
    key: 'recs',
    label: 'Recomendaciones desde Letterboxd',
    hint: 'Genera y navega por películas recomendadas basadas en tu historial.',
    path: '/recs',
    needsSession: true,
  },
  {
    key: 'search',
    label: 'Buscar torrents directamente',
    hint: 'Escribe un título y busca torrents sin pasar por Letterboxd.',
    path: '/search',
    needsSession: false,
  },
] as const

export function Home() {
  const nav = useNavigate()
  const [i, setI] = useState(0)
  const [loggedIn, setLoggedIn] = useState<boolean | null>(null)

  useEffect(() => {
    if (!isTauri()) {
      setLoggedIn(false)
      return
    }
    hasSession().then(setLoggedIn).catch(() => setLoggedIn(false))
  }, [])

  const go = (opt: (typeof OPTIONS)[number]) => {
    if (opt.needsSession && loggedIn === false) {
      nav('/login?next=' + encodeURIComponent(opt.path))
      return
    }
    nav(opt.path)
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: 'bajar', run: () => setI((x) => Math.min(x + 1, OPTIONS.length - 1)) },
    { key: 'ArrowDown', hint: '', run: () => setI((x) => Math.min(x + 1, OPTIONS.length - 1)) },
    { key: 'k', hint: 'subir', run: () => setI((x) => Math.max(x - 1, 0)) },
    { key: 'ArrowUp', hint: '', run: () => setI((x) => Math.max(x - 1, 0)) },
    { key: 'Enter', hint: 'seleccionar', run: () => go(OPTIONS[i]) },
    { key: ',', hint: 'Ajustes', run: () => nav('/settings') },
  ]
  useHotkeys(hotkeys, [i, loggedIn])

  const barKeys: Hotkey[] = [
    { key: 'j', hint: 'Mover', run: () => {} },
    { key: 'Enter', hint: 'seleccionar', run: () => {} },
  ]

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        {loggedIn ? (
          <span className="rounded-full px-3 py-1 text-[13px] text-muted">
            Sesión activa
          </span>
        ) : (
          <button
            onClick={() => nav('/login')}
            className="focus-ring glass rounded-full px-4 py-1.5 text-[13px] text-ink transition-transform hover:scale-[1.02]"
          >
            Iniciar sesión
          </button>
        )}
        <button
          onClick={() => nav('/settings')}
          className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-[13px] text-body hover:border-border-strong"
          title="Ajustes (,)"
        >
          Ajustes
        </button>
      </TopNav>

      <main className="mx-auto flex w-full max-w-[720px] flex-1 flex-col justify-center px-8">
        <h1 className="mb-2 text-[32px] font-semibold leading-tight tracking-tight text-ink">
          ¿Qué hacemos hoy?
        </h1>
        <p className="mb-10 text-[15px] text-muted">
          Elige una de las dos opciones o pulsa Enter sobre la resaltada.
        </p>

        <ul className="flex flex-col gap-2">
          {OPTIONS.map((opt, idx) => {
            const active = idx === i
            return (
              <li key={opt.key}>
                <button
                  onClick={() => go(opt)}
                  onMouseEnter={() => setI(idx)}
                  className={`focus-ring glass w-full rounded-lg px-5 py-4 text-left transition-transform ${
                    active
                      ? 'scale-[1.01] outline outline-1 outline-white/30'
                      : 'hover:scale-[1.005]'
                  }`}
                >
                  <div className="flex items-baseline justify-between gap-4">
                    <span className="text-[16px] font-medium text-ink">
                      {opt.label}
                    </span>
                    {active && (
                      <span
                        className="flex h-6 w-6 items-center justify-center text-accent"
                        aria-label="Pulsa Enter"
                        title="Enter"
                      >
                        <KeyReturn size={18} weight="bold" />
                      </span>
                    )}
                  </div>
                  <p className="mt-1 text-[13px] text-muted">{opt.hint}</p>
                </button>
              </li>
            )
          })}
        </ul>
      </main>

      <HotkeyBar hotkeys={barKeys} />
    </div>
  )
}
