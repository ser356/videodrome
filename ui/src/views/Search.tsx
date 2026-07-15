import { useNavigate } from 'react-router-dom'
import { HotkeyBar } from '../components/HotkeyBar'
import { TopNav } from '../components/TopNav'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Vista `View::Search` de la TUI. Un solo input; Enter dispara la
 * búsqueda directa contra los providers de torrents (bypass Letterboxd
 * y TMDB). El resultado se muestra en la ruta `/torrents/search`.
 */
export function Search() {
  const nav = useNavigate()

  const hotkeys: Hotkey[] = [
    { key: 'Escape', hint: 'volver', run: () => nav('/'), ignoreInInput: false },
  ]
  useHotkeys(hotkeys, [])

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav />

      <main className="mx-auto flex w-full max-w-[720px] flex-1 flex-col justify-center px-8">
        <h1 className="mb-1 text-[24px] font-semibold text-ink">
          Buscar torrents
        </h1>
        <p className="mb-6 text-[14px] text-muted">
          Escribe el título. Añade el año al final para desambiguar remakes
          (por ejemplo, "Funny Games 2007").
        </p>

        <form
          onSubmit={(e) => {
            e.preventDefault()
            const q = new FormData(e.currentTarget).get('q')?.toString().trim()
            if (q) nav(`/search/results?q=${encodeURIComponent(q)}`)
          }}
          className="flex gap-2"
        >
          <input
            name="q"
            autoFocus
            required
            placeholder="Título…"
            className="focus-ring glass h-11 flex-1 rounded-full px-4 text-[15px] text-ink placeholder:text-dim"
          />
          <button
            type="submit"
            className="focus-ring h-11 rounded-full bg-accent px-5 text-[15px] font-semibold text-on-accent transition-colors hover:bg-accent-hover"
          >
            Buscar
          </button>
        </form>
      </main>

      <HotkeyBar
        hotkeys={[
          { key: 'Enter', hint: 'buscar', run: () => {} },
          { key: 'Escape', hint: 'volver', run: () => {} },
        ]}
      />
    </div>
  )
}
