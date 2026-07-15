import { useEffect, useState } from 'react'
import {
  formatSize,
  streamStats,
  type StreamInfo,
  type StreamStats,
} from '../lib/api'

/**
 * Panel inferior de la vista Torrents. Dos modos:
 *
 * 1. `showMagnet=true`: muestra el magnet URI del torrent seleccionado
 *    (útil para copiar y pegar en otro cliente BitTorrent).
 * 2. `showMagnet=false`: si hay stream activo, poll cada segundo a
 *    `stream_stats` y muestra progreso / peers / MiB/s.  Si no hay
 *    stream, muestra el mensaje de estado o un placeholder.
 *
 * Cuando el backend nos dice `alive=false` (VLC cerrado o el proceso
 * murió) llamamos a `onPlayerDied` para que el padre haga el cleanup
 * (parar librqbit, limpiar mensaje). Sin esto el panel se quedaba
 * "Streaming" indefinidamente aunque el player estuviera cerrado.
 */
export function StreamPanel({
  showMagnet,
  magnet,
  stream,
  message,
  onStopStream,
  onPlayerDied,
}: {
  showMagnet: boolean
  magnet?: string
  stream: StreamInfo | null
  message: string | null
  onStopStream: () => void
  onPlayerDied: () => void
}) {
  const [stats, setStats] = useState<StreamStats | null>(null)

  useEffect(() => {
    if (!stream || showMagnet) return
    let cancelled = false
    const poll = async () => {
      try {
        const s = await streamStats(stream.id)
        if (cancelled) return
        setStats(s)
        if (!s.alive) {
          // VLC murió → backend ya se ha limpiado a sí mismo, avisamos
          // al padre para tirar el stream state y parar el poll.
          cancelled = true
          onPlayerDied()
        }
      } catch {
        // stream_stats devuelve error si el handle ya no está en el
        // mapa (por ejemplo, la respuesta anterior con alive=false ya
        // lo eliminó). Tratamos ese caso igual: player muerto.
        if (!cancelled) {
          cancelled = true
          onPlayerDied()
        }
      }
    }
    poll()
    const id = window.setInterval(poll, 1000)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [stream, showMagnet, onPlayerDied])

  if (showMagnet) {
    return (
      <div className="glass rounded-lg p-4">
        <div className="mb-1 text-[11px] uppercase tracking-wide text-dim">
          Magnet URI
        </div>
        <textarea
          readOnly
          value={magnet ?? '(selecciona un torrent)'}
          className="w-full resize-none rounded-md bg-transparent text-[12px] text-body focus:outline-none"
          rows={2}
        />
      </div>
    )
  }

  if (stream) {
    const pct = stats && stats.total_bytes > 0
      ? (stats.progress_bytes / stats.total_bytes) * 100
      : 0
    return (
      <div className="glass rounded-lg p-4" style={{ borderColor: 'rgba(0, 224, 84, 0.4)' }}>
        <div className="mb-2 flex items-baseline justify-between">
          <div>
            <span className="text-[13px] font-medium text-ink">
              ▶ Streaming
            </span>{' '}
            <span className="text-[13px] text-body">{stream.file_name}</span>
          </div>
          <button
            onClick={onStopStream}
            className="focus-ring rounded-full border border-hairline px-3 py-1 text-[12px] text-body hover:border-border-strong"
          >
            Detener
          </button>
        </div>

        {stats && (
          <>
            <div className="mb-2 h-1.5 w-full overflow-hidden rounded-full bg-surface-hi">
              <div
                className="h-full bg-good transition-all duration-500"
                style={{ width: `${pct.toFixed(1)}%` }}
              />
            </div>
            <div className="flex flex-wrap gap-x-6 text-[12px] tabular-nums text-muted">
              <span>{pct.toFixed(1)} %</span>
              <span>
                {formatSize(stats.progress_bytes)} /{' '}
                {formatSize(stats.total_bytes)}
              </span>
              <span className="text-good">↓ {stats.down_mbps.toFixed(2)} MiB/s</span>
              <span>{stats.live_peers} peers</span>
            </div>
          </>
        )}

        <div className="mt-3 truncate text-[11px] text-dim">
          {stream.url}
        </div>
      </div>
    )
  }

  // Hint / status placeholder. Con borde-l naranja + kbds resaltados
  // para que no se lea como "otro bloque de fondo" cuando el panel está
  // vacío. Si hay `message` (feedback de la última acción), lo mostramos
  // en su lugar con el mismo tratamiento visual.
  return (
    <div className="glass flex items-start gap-3 rounded-lg border-l-2 border-accent bg-accent/[0.06] p-4 text-[13px]">
      {message ? (
        <p className="text-body">{message}</p>
      ) : (
        <p className="text-body">
          Pulsa <Kbd>Enter</Kbd> para proyectar el torrent seleccionado
          en VLC (te preguntará si quieres subtítulos antes de arrancar).{' '}
          <Kbd>S</Kbd> envía el magnet a tu cliente BitTorrent por
          defecto.
        </p>
      )}
    </div>
  )
}

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="mx-0.5 rounded-sm border border-accent/40 bg-accent/15 px-1.5 py-0.5 text-[11px] font-semibold text-accent">
      {children}
    </kbd>
  )
}
