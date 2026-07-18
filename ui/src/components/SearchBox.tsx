import { MagnifyingGlass } from '@phosphor-icons/react'
import { useNavigate } from 'react-router-dom'
import { useT } from '../lib/i18n'

/**
 * Caja de búsqueda pill (glass). Enter navega a la pantalla intermedia
 * `/search/results`, que resuelve la query contra TMDB y muestra posters
 * para desambiguar antes de saltar a torrents.
 */
export function SearchBox({ compact = false }: { compact?: boolean }) {
  const nav = useNavigate()
  const t = useT()
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        const q = new FormData(e.currentTarget)
          .get('q')
          ?.toString()
          .trim()
        if (q) nav(`/search/results?q=${encodeURIComponent(q)}`)
      }}
      className="glass flex items-center gap-2 rounded-full px-3 py-1.5 focus-within:outline focus-within:outline-2 focus-within:outline-accent"
    >
      <MagnifyingGlass size={14} weight="bold" className="text-muted" />
      <input
        name="q"
        type="text"
        placeholder={t('search.boxPlaceholder')}
        className={`bg-transparent text-[13px] text-ink placeholder:text-dim focus:outline-none ${
          compact ? 'w-[200px]' : 'w-[260px]'
        }`}
        spellCheck={false}
      />
    </form>
  )
}
