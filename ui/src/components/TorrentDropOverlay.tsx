import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { Magnet } from '@phosphor-icons/react'
import {
  isTauri,
  resolveDroppedTorrent,
  type DroppedTorrentSource,
} from '../lib/api'
import { useT } from '../lib/i18n'

/**
 * Overlay global de drag & drop de torrents. Se monta UNA VEZ en
 * `main.tsx` fuera del `<Routes>` para escuchar drops en cualquier
 * pantalla — el user puede soltar un `.torrent` o un enlace magnet
 * en Home, Search, Torrents, o incluso en el propio Player, y
 * aterrizará en `/torrents/dropped` con la lista de ficheros
 * lista para reproducir.
 *
 * Dos listeners porque Tauri 2 solo entrega file-drops nativos por
 * `webview.onDragDropEvent` (WebView intercepta los HTML5 events
 * cuando el drop es un fichero real del OS); los magnet URIs de
 * texto SÍ llegan como HTML5 `drop` con `dataTransfer.getData
 * ('text/plain')`. Coordinamos ambos con el mismo estado.
 *
 * Fases del estado:
 *   * `dragActive` — mientras hay algo flotando sobre la ventana:
 *     pinta el overlay grande "Suelta para reproducir". Debounced
 *     con contador (Tauri emite over/leave por cada tick) para no
 *     parpadear al pasar entre subelementos.
 *   * `resolving` — mientras el backend descarga metadata + lista
 *     ficheros (list_files → puede tardar 3–15 s con swarm nuevo).
 *   * `error` — mensaje transitorio si el drop no es válido
 *     (fichero sin `.torrent`, magnet malformado, backend falla).
 */
export function TorrentDropOverlay() {
  const t = useT()
  const nav = useNavigate()
  const [dragActive, setDragActive] = useState(false)
  const [resolving, setResolving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const errorTimerRef = useRef<number | null>(null)
  // Guard anti double-drop: si el user suelta MIENTRAS estamos
  // resolviendo el anterior, ignoramos silenciosamente en vez de
  // encolar. El flujo esperado es un solo torrent por interacción.
  const inFlightRef = useRef(false)

  const flashError = (msg: string) => {
    setError(msg)
    if (errorTimerRef.current) window.clearTimeout(errorTimerRef.current)
    errorTimerRef.current = window.setTimeout(() => setError(null), 3200)
  }

  const handleDrop = async (source: DroppedTorrentSource) => {
    if (inFlightRef.current) return
    inFlightRef.current = true
    setResolving(true)
    try {
      const resolved = await resolveDroppedTorrent(source)
      // El estado se propaga por `location.state` — no lo persistimos
      // en URL porque el magnet URI pasa fácil de 500 chars.
      nav('/torrents/dropped', { state: resolved })
    } catch (e) {
      flashError(String(e))
    } finally {
      setResolving(false)
      inFlightRef.current = false
    }
  }

  // ── Tauri file-drop (ficheros .torrent del OS)
  useEffect(() => {
    if (!isTauri()) return
    let unlisten: (() => void) | null = null
    let cancelled = false
    ;(async () => {
      try {
        const webview = getCurrentWebview()
        const off = await webview.onDragDropEvent((event) => {
          const payload = event.payload
          // Solo entramos en overlay-mode si el drag lleva algún
          // `.torrent`. Otros drops (subs .srt/.vtt/.ass sobre el
          // Player, o cualquier fichero suelto) NO deben pintar el
          // overlay "Suelta para reproducir" — Player.tsx tiene
          // su propio listener para subs y no queremos competir
          // visualmente. Ambos listeners coexisten porque
          // `onDragDropEvent` reparte a todos los suscriptores.
          if (payload.type === 'enter' || payload.type === 'over') {
            const paths = (payload.paths ?? []) as string[]
            if (paths.some((p) => /\.torrent$/i.test(p))) {
              setDragActive(true)
            }
          } else if (payload.type === 'leave') {
            setDragActive(false)
          } else if (payload.type === 'drop') {
            setDragActive(false)
            const paths = (payload.paths ?? []) as string[]
            const torrent = paths.find((p) => /\.torrent$/i.test(p))
            if (!torrent) {
              // No es un .torrent — dejamos que otros listeners
              // (Player.tsx para subs) se ocupen. No mostramos
              // error para no molestar en drops legítimos de subs.
              return
            }
            void handleDrop({ kind: 'file', path: torrent })
          }
        })
        if (cancelled) {
          off()
        } else {
          unlisten = off
        }
      } catch (e) {
        console.warn('TorrentDropOverlay: onDragDropEvent setup failed:', e)
      }
    })()
    return () => {
      cancelled = true
      unlisten?.()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // ── HTML5 drop de texto (magnet: URIs desde el navegador)
  //
  // Necesita capturar `dragover` con `preventDefault` para que el
  // `drop` dispare. Solo activa el overlay si el payload incluye
  // texto tipo magnet — así evitamos pintar overlay para drags
  // internos de la app (que el user no está intentando dropear
  // como torrent).
  useEffect(() => {
    if (typeof window === 'undefined') return
    let overCounter = 0
    const hasMagnetPayload = (e: DragEvent) => {
      const types = e.dataTransfer?.types
      if (!types) return false
      // No hay forma fiable de leer el TEXTO en dragover (por seguridad
      // Chromium solo lo entrega en drop). Nos fiamos de que haya un
      // tipo `text/plain` o `text/uri-list` presente.
      return (
        Array.from(types).includes('text/plain') ||
        Array.from(types).includes('text/uri-list')
      )
    }
    const onDragEnter = (e: DragEvent) => {
      if (!hasMagnetPayload(e)) return
      e.preventDefault()
      overCounter += 1
      setDragActive(true)
    }
    const onDragOver = (e: DragEvent) => {
      if (!hasMagnetPayload(e)) return
      e.preventDefault()
      e.dataTransfer!.dropEffect = 'copy'
    }
    const onDragLeave = (e: DragEvent) => {
      if (!hasMagnetPayload(e)) return
      e.preventDefault()
      overCounter = Math.max(0, overCounter - 1)
      if (overCounter === 0) setDragActive(false)
    }
    const onDrop = (e: DragEvent) => {
      const raw =
        e.dataTransfer?.getData('text/uri-list') ||
        e.dataTransfer?.getData('text/plain') ||
        ''
      const trimmed = raw.trim()
      if (!trimmed.toLowerCase().startsWith('magnet:')) {
        // Silencio si no es magnet — puede ser cualquier texto
        // que el user haya arrastrado sin intención.
        overCounter = 0
        setDragActive(false)
        return
      }
      e.preventDefault()
      overCounter = 0
      setDragActive(false)
      void handleDrop({ kind: 'magnet', uri: trimmed })
    }
    window.addEventListener('dragenter', onDragEnter)
    window.addEventListener('dragover', onDragOver)
    window.addEventListener('dragleave', onDragLeave)
    window.addEventListener('drop', onDrop)
    return () => {
      window.removeEventListener('dragenter', onDragEnter)
      window.removeEventListener('dragover', onDragOver)
      window.removeEventListener('dragleave', onDragLeave)
      window.removeEventListener('drop', onDrop)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Nada que pintar en reposo — no queremos añadir ni un div en
  // el árbol cuando no hay drag activo (evita interceptar clicks
  // por accidente si algo saliera mal con pointer-events).
  if (!dragActive && !resolving && !error) return null

  return (
    <div
      className="pointer-events-none fixed inset-0 z-[100] flex items-center justify-center bg-canvas/70 backdrop-blur-md animate-drop-in"
      role="status"
      aria-live="polite"
    >
      <div className="glass-strong flex max-w-[420px] flex-col items-center gap-4 rounded-2xl px-10 py-8 text-center outline outline-2 outline-accent/60">
        <div className="text-accent animate-bounce-slow">
          <Magnet size={56} weight="duotone" />
        </div>
        {resolving ? (
          <>
            <h2 className="text-[18px] font-semibold text-ink">
              {t('drop.resolving')}
            </h2>
            <p className="text-[13px] text-muted">
              {t('drop.resolvingHint')}
            </p>
          </>
        ) : error ? (
          <>
            <h2 className="text-[18px] font-semibold text-ink">
              {t('drop.failed')}
            </h2>
            <p className="text-[13px] text-danger">{error}</p>
          </>
        ) : (
          <>
            <h2 className="text-[18px] font-semibold text-ink">
              {t('drop.title')}
            </h2>
            <p className="text-[13px] text-muted">{t('drop.subtitle')}</p>
          </>
        )}
      </div>
    </div>
  )
}
