import { Sliders } from '@phosphor-icons/react'
import { useEffect, useRef, useState } from 'react'

/**
 * Popover con los filtros de la vista Recomendaciones (rating mínimo,
 * cantidad de resultados). Se abre desde un botón "Filtros" en el
 * TopNav para sacarlos del chrome principal. Los cambios NO recargan
 * automáticamente; el user aplica con "Recargar" cuando quiera.
 */
export function FiltersDropdown({
  minRating,
  count,
  dirty,
  onChange,
}: {
  minRating: number
  count: number
  dirty: boolean
  onChange: (minRating: number, count: number) => void
}) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        className={`focus-ring glass relative flex h-9 w-9 items-center justify-center rounded-full transition-transform hover:scale-[1.05] ${
          dirty ? 'text-ink' : 'text-body'
        }`}
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label="Filtros"
        title="Filtros"
      >
        <Sliders size={16} weight="bold" />
        {dirty && (
          <span className="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-white ring-2 ring-canvas" />
        )}
      </button>

      {open && (
        <div
          role="menu"
          className="popover absolute right-0 top-[calc(100%+8px)] z-40 flex w-[240px] flex-col gap-3 rounded-lg p-4"
        >
          <FilterRow
            label="Rating mínimo"
            value={minRating.toFixed(1)}
          >
            <RangeInput
              min={0.5}
              max={5}
              step={0.5}
              value={minRating}
              onChange={(v) => onChange(v, count)}
            />
          </FilterRow>

          <FilterRow
            label="Cuántas mostrar"
            value={String(count)}
          >
            <RangeInput
              min={5}
              max={60}
              step={5}
              value={count}
              onChange={(v) => onChange(minRating, v)}
            />
          </FilterRow>

          <p className="border-t border-hairline-soft pt-2 text-[11px] text-muted">
            Pulsa{' '}
            <kbd className="rounded-sm border border-hairline bg-surface px-1.5 py-0.5 text-[10px] text-body">
              R
            </kbd>{' '}
            para recargar.
          </p>
        </div>
      )}
    </div>
  )
}

// ─── row + slider primitivo ───

function FilterRow({
  label,
  value,
  children,
}: {
  label: string
  value: string
  children: React.ReactNode
}) {
  return (
    <div>
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-[13px] font-medium text-ink">{label}</span>
        <span className="text-[13px] tabular-nums text-body">{value}</span>
      </div>
      <div className="mt-2">{children}</div>
    </div>
  )
}

function RangeInput({
  min,
  max,
  step,
  value,
  onChange,
}: {
  min: number
  max: number
  step: number
  value: number
  onChange: (v: number) => void
}) {
  return (
    <input
      type="range"
      min={min}
      max={max}
      step={step}
      value={value}
      onChange={(e) => onChange(Number(e.target.value))}
      className="h-1 w-full cursor-pointer appearance-none rounded-full bg-surface-hi accent-accent"
    />
  )
}
