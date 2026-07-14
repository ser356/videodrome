import type { ReactNode } from 'react'

/**
 * Toast flotante inferior. Se muestra encima de la HotkeyBar cuando
 * `visible` es true. No auto-desaparece; el caller controla el estado.
 */
export function Toast({
  visible,
  children,
}: {
  visible: boolean
  children: ReactNode
}) {
  if (!visible) return null
  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-16 z-40 flex justify-center px-8">
      <div
        role="status"
        className="popover pointer-events-auto flex items-center gap-3 rounded-full px-4 py-2 text-[13px] text-body shadow-lg"
      >
        {children}
      </div>
    </div>
  )
}
