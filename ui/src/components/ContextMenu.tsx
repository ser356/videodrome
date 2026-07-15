import { useEffect, useLayoutEffect, useRef, useState } from 'react'

/**
 * Ítem del menú contextual. `onClick` cierra automáticamente.
 * `destructive` pinta en rojo (usado para "no sugerir", eliminar…).
 */
export interface ContextMenuItem {
  label: string
  onClick: () => void
  hint?: string
  destructive?: boolean
  disabled?: boolean
}

interface Props {
  x: number
  y: number
  items: ContextMenuItem[]
  onClose: () => void
}

/**
 * Menú flotante posicionado en `(x, y)` (coords de viewport). Se
 * autoajusta si se sale por el borde derecho/inferior. Cierra con
 * Escape, click fuera o al elegir un ítem.
 *
 * Usa `glass-strong` para separarse visualmente del contenido de
 * fondo — nunca se muestra sobre un overlay oscuro, así que necesita
 * más blur y borde que un popover normal.
 */
export function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement | null>(null)
  const [pos, setPos] = useState({ x, y })

  // Ajusta la posición si el menú se sale por el borde derecho/inferior.
  // Se calcula tras el primer paint (useLayoutEffect) para tener las
  // dimensiones reales del contenedor, no las estimadas.
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const vw = window.innerWidth
    const vh = window.innerHeight
    let nx = x
    let ny = y
    if (x + rect.width > vw - 8) nx = Math.max(8, vw - rect.width - 8)
    if (y + rect.height > vh - 8) ny = Math.max(8, vh - rect.height - 8)
    setPos({ x: nx, y: ny })
  }, [x, y])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    }
    const onClickOutside = (e: MouseEvent) => {
      if (!ref.current) return
      if (!ref.current.contains(e.target as Node)) onClose()
    }
    // Nota: NO registramos un listener global sobre `contextmenu`. Si
    // el user hace un segundo right-click en otra card, el handler
    // sintético de esa card ya llama a setMenu con las nuevas coords;
    // añadir aquí un cierre a nivel documento producía un race (React
    // dispatcha primero el synthetic y luego los doc-listeners, así
    // que setMenu(new) → setMenu(null) → menú desaparecía y "no hacía
    // nada"). El `mousedown` fuera cubre el caso de cerrar cuando el
    // user clica en otra parte de la UI.
    document.addEventListener('keydown', onKey)
    document.addEventListener('mousedown', onClickOutside)
    return () => {
      document.removeEventListener('keydown', onKey)
      document.removeEventListener('mousedown', onClickOutside)
    }
  }, [onClose])

  return (
    <div
      ref={ref}
      role="menu"
      style={{ top: pos.y, left: pos.x }}
      className="glass-strong fixed z-[60] min-w-[220px] rounded-lg py-1.5 text-[13px] shadow-2xl animate-modal-in"
    >
      {items.map((item, i) => (
        <button
          key={i}
          role="menuitem"
          disabled={item.disabled}
          onClick={() => {
            if (item.disabled) return
            item.onClick()
            onClose()
          }}
          className={`flex w-full items-center justify-between gap-6 px-3.5 py-2 text-left transition-colors ${
            item.disabled
              ? 'cursor-not-allowed text-dim'
              : item.destructive
                ? 'text-danger hover:bg-danger/15'
                : 'text-body hover:bg-white/10 hover:text-ink'
          }`}
        >
          <span className="truncate">{item.label}</span>
          {item.hint && (
            <span className="shrink-0 text-[11px] text-dim">{item.hint}</span>
          )}
        </button>
      ))}
    </div>
  )
}
