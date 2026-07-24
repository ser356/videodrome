import { useEffect } from 'react'
import { useLocation, useNavigationType } from 'react-router-dom'

/**
 * Scroll restoration por ruta (in-memory).
 *
 * React Router 6 no restaura scroll por sí solo (a diferencia del
 * behavior nativo del browser en apps MPA). Este componente:
 *
 *   * Snapshotea `window.scrollY` de la ruta SALIENTE en `cleanup`
 *     del effect — el timing coincide con "el user acaba de
 *     navegar fuera", así que el valor guardado es el último
 *     scroll visible antes del cambio.
 *   * Al montar la NUEVA ruta, restaura desde el Map si hay entry
 *     (típicamente al volver con "atrás"), o scrollea al top si no.
 *   * Excepciones (`ALWAYS_TOP`): rutas que deben empezar SIEMPRE
 *     arriba independientemente de dónde vinieran ni de cuántas
 *     veces se hayan visitado — p.ej. `/settings`, que se percibe
 *     mejor como panel fijo estilo Preferencias del sistema.
 *
 * Retry con ResizeObserver: muchas vistas (Recommendations, Search
 * Results, Torrents) re-fetchean el contenido al montar → el body
 * empieza a altura ~0 y crece asíncronamente. Un `scrollTo` inmediato
 * queda capado por el browser al `scrollHeight` actual (0) y se
 * pierde. Solución: observamos el redimensionamiento de
 * `document.documentElement` y reaplicamos `scrollTo(0, saved)`
 * mientras el body siga creciendo y aún no hayamos alcanzado el
 * target. Auto-desmontamos el observer tras 1500 ms o cuando ya
 * hemos llegado, lo que ocurra antes.
 *
 * El Map vive en el módulo (no en state) porque queremos que
 * sobreviva a re-renders y a los remounts que hace React 19 en
 * StrictMode. NO persiste entre reinicios de la app — es
 * intencional: si cierras Videodrome y lo abres, empiezas desde
 * arriba, como cualquier app nativa.
 */

const scrollPositions = new Map<string, number>()

/** Rutas que siempre montan en scroll=0. */
const ALWAYS_TOP = new Set<string>(['/settings'])

/** Máximo tiempo que reintentamos el restore mientras crece el body. */
const RESTORE_WINDOW_MS = 1500

/** Aplica el scroll de forma robusta a los dos targets que WebKit
 *  considera "scrolling element" según modo (quirks/standards). */
function applyScroll(y: number) {
  window.scrollTo(0, y)
  // Fallback defensivo — algunos WebViews solo mueven uno de los dos.
  document.documentElement.scrollTop = y
}

export function ScrollRestore() {
  const location = useLocation()
  const navType = useNavigationType()

  useEffect(() => {
    const key = location.pathname + location.search
    if (ALWAYS_TOP.has(location.pathname)) {
      applyScroll(0)
      return
    }

    const saved = scrollPositions.get(key)
    let observer: ResizeObserver | null = null
    let timeout = 0

    if (saved != null && saved > 0) {
      // Intento inmediato.
      applyScroll(saved)

      // ResizeObserver: mientras el body crezca, reaplicamos hasta
      // llegar al target o agotar la ventana. Comprobamos también
      // que `scrollHeight >= saved + viewport` para no forzar un
      // scroll imposible si el contenido acaba siendo más corto de
      // lo que era antes (p.ej. filtros que reducen la lista).
      observer = new ResizeObserver(() => {
        const max =
          document.documentElement.scrollHeight - window.innerHeight
        const target = Math.min(saved, Math.max(0, max))
        if (window.scrollY !== target) applyScroll(target)
        if (target >= saved) {
          observer?.disconnect()
          observer = null
        }
      })
      observer.observe(document.documentElement)

      timeout = window.setTimeout(() => {
        observer?.disconnect()
        observer = null
      }, RESTORE_WINDOW_MS)
    } else if (navType === 'PUSH') {
      applyScroll(0)
    }

    return () => {
      // Snapshot al salir.
      const y =
        window.scrollY || document.documentElement.scrollTop || 0
      if (!ALWAYS_TOP.has(location.pathname)) {
        scrollPositions.set(key, y)
      }
      if (observer) observer.disconnect()
      if (timeout) window.clearTimeout(timeout)
    }
  }, [location.pathname, location.search, navType])

  return null
}

