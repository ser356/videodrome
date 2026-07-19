import { useEffect, useState } from 'react'
import { type StreamStats } from '../../lib/api'
import { formatSpeed } from './utils'
import { LoadingDots } from './controls'

export function StremioLoader({
  title,
  backdropUrl,
  logoUrl,
  stats,
}: {
  title: string
  backdropUrl: string | null
  logoUrl: string | null
  stats?: StreamStats | null
}) {
  const bytesPerSec = stats ? stats.down_mbps * 1024 * 1024 : 0
  const hasProgress = stats != null && stats.total_bytes > 0
  const pct = hasProgress
    ? (stats!.progress_bytes / stats!.total_bytes) * 100
    : null
  // Metahub sirve el rótulo oficial de la peli (mismo CDN que
  // Stremio); 404 cuando no hay logo para ese imdb_id. En ese caso
  // — o si no tenemos imdb_id de partida — caemos al favicon de la
  // app + título en texto pequeño debajo, para que la pantalla siga
  // siendo icónica y no un `<h1>` grande solo.
  const [logoFailed, setLogoFailed] = useState(false)
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset s\u00edncrono cuando cambia el logoUrl; sin alternativa async.
    setLogoFailed(false)
  }, [logoUrl])
  const showLogo = logoUrl != null && !logoFailed
  return (
    <div className="pointer-events-none absolute inset-0 overflow-hidden bg-black">
      {backdropUrl && (
        <div
          className="absolute inset-0 bg-cover bg-center opacity-60 transition-opacity duration-500"
          style={{ backgroundImage: `url(${backdropUrl})` }}
        />
      )}
      {/* Vignette + gradiente inferior para que el título tenga
          contraste sobre cualquier backdrop (típicamente claro en el
          centro con la cara del protagonista). */}
      <div
        className="absolute inset-0"
        style={{
          background: backdropUrl
            ? 'radial-gradient(ellipse at center, rgba(0,0,0,0.2) 0%, rgba(0,0,0,0.55) 55%, rgba(0,0,0,0.9) 100%)'
            : 'radial-gradient(circle at 50% 50%, rgba(255,255,255,0.04) 0%, rgba(0,0,0,0) 60%)',
        }}
      />
      <div className="relative flex h-full w-full flex-col items-center justify-center gap-6 px-8 text-center">
        {showLogo ? (
          <img
            src={logoUrl!}
            alt={title}
            onError={() => setLogoFailed(true)}
            className="max-h-[28vh] max-w-[60vw] object-contain drop-shadow-[0_4px_16px_rgba(0,0,0,0.9)]"
          />
        ) : (
          <div className="flex flex-col items-center gap-4">
            <img
              src="/favicon.svg"
              alt="Videodrome"
              className="h-20 w-20 opacity-90 drop-shadow-[0_4px_16px_rgba(0,0,0,0.9)] sm:h-24 sm:w-24"
            />
            <h1 className="text-balance text-[20px] font-medium tracking-tight text-ink drop-shadow-[0_2px_8px_rgba(0,0,0,0.9)] sm:text-[24px]">
              {title}
            </h1>
          </div>
        )}
        <div className="flex items-center gap-3 text-[12px] uppercase tracking-[0.18em] text-dim drop-shadow-[0_1px_4px_rgba(0,0,0,0.9)]">
          <span className="h-4 w-4 animate-spin rounded-full border-2 border-white/40 border-t-white" />
          <span>
            Cargando<LoadingDots />
          </span>
        </div>
        {stats && (
          <div className="flex flex-wrap items-center justify-center gap-x-5 gap-y-1 text-[12px] tabular-nums text-body/80 drop-shadow-[0_1px_4px_rgba(0,0,0,0.9)]">
            <span>{formatSpeed(bytesPerSec)}</span>
            <span className="text-dim">·</span>
            <span>{stats.live_peers} peers</span>
            {pct != null && (
              <>
                <span className="text-dim">·</span>
                <span>{pct.toFixed(1)}%</span>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
