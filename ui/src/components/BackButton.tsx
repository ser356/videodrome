import { CaretLeft } from '@phosphor-icons/react'
import { useT } from '../lib/i18n'

/**
 * Botón "volver" físico. Por defecto es solo un círculo con la chevron
 * (icon-only, para chrome denso). Pasa `label` si además quieres texto
 * al lado.
 */
export function BackButton({
  onClick,
  label,
}: {
  onClick: () => void
  label?: string
}) {
  const t = useT()
  const back = t('common.back')
  return (
    <button
      onClick={onClick}
      className={`focus-ring group inline-flex items-center gap-2 rounded-full text-[13px] text-body transition-transform hover:scale-[1.02] ${
        label ? 'py-1 pl-1 pr-4' : ''
      }`}
      aria-label={label ?? back}
      title={back}
    >
      <span className="glass flex h-8 w-8 items-center justify-center rounded-full text-ink">
        <CaretLeft size={16} weight="bold" />
      </span>
      {label && <span>{label}</span>}
    </button>
  )
}
