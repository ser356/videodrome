import { MagnifyingGlass } from '@phosphor-icons/react'
import { useNavigate } from 'react-router-dom'

/**
 * Caja de búsqueda pill (glass), Enter navega a /torrents/search con la
 * query. Es el atajo desde cualquier vista para buscar torrents por
 * título sin pasar por las recomendaciones de Letterboxd.
 */
export function SearchBox({ compact = false }: { compact?: boolean }) {
  const nav = useNavigate()
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        const q = new FormData(e.currentTarget)
          .get('q')
          ?.toString()
          .trim()
        if (q) nav(`/torrents/search?q=${encodeURIComponent(q)}`)
      }}
      className="glass flex items-center gap-2 rounded-full px-3 py-1.5 focus-within:outline focus-within:outline-2 focus-within:outline-accent"
    >
      <MagnifyingGlass size={14} weight="bold" className="text-muted" />
      <input
        name="q"
        type="text"
        placeholder="Buscar película…"
        className={`bg-transparent text-[13px] text-ink placeholder:text-dim focus:outline-none ${
          compact ? 'w-[200px]' : 'w-[260px]'
        }`}
        spellCheck={false}
      />
    </form>
  )
}
