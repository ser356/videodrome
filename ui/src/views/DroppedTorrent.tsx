import { useEffect, useMemo, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { FilmSlate, Magnet as MagnetIcon } from '@phosphor-icons/react'
import { HotkeyBar } from '../components/HotkeyBar'
import { TopNav } from '../components/TopNav'
import {
  formatSize,
  type ResolvedDroppedTorrent,
  type TorrentFileInfo,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'
import { useT } from '../lib/i18n'

/**
 * Vista de "torrent dropeado" — llega aquí `TorrentDropOverlay`
 * tras resolver metadata (`resolve_dropped_torrent`). Diferencia
 * clave con `Torrents.tsx`: ya tenemos UN torrent concreto (no una
 * lista de candidatos de providers), así que la interacción es
 * simplemente elegir el fichero de vídeo a reproducir dentro del
 * torrent y arrancar el stream.
 *
 * Estética alineada con `Torrents.tsx` (mismo `TopNav`, mismas
 * cards de `glass` hairline, `HotkeyBar` inferior). Sin providers
 * pill (no aplica), sin buscador (el user acaba de dropear su fuente).
 *
 * El streaming lo arranca `Player.tsx` al montar, igual que desde
 * la ruta normal de Torrents — aquí solo navegamos con el
 * `PlayerState` correcto (magnet + fileHint + título del torrent).
 * Si el user aterriza sin `location.state` (ej. refresh, deep-link
 * a mano), volvemos a Home — no queremos pintar un empty state
 * "misterioso".
 */
export function DroppedTorrent() {
  const t = useT()
  const nav = useNavigate()
  const location = useLocation()
  const resolved = (location.state as ResolvedDroppedTorrent | null) ?? null

  useEffect(() => {
    if (!resolved) {
      nav('/', { replace: true })
    }
  }, [resolved, nav])

  // Vídeos primero (por tamaño desc), no-vídeos al final — mismo
  // criterio que usa el backend `select_file` para elegir el más
  // grande cuando el user no pasa hint. Los ficheros no-vídeo
  // (nfo, samples, extras) los mostramos para transparencia pero
  // deshabilitados: el player no sabe qué hacer con ellos.
  const files = useMemo<TorrentFileInfo[]>(() => {
    if (!resolved) return []
    const copy = [...resolved.files]
    copy.sort((a, b) => {
      if (a.is_video !== b.is_video) return a.is_video ? -1 : 1
      return b.size - a.size
    })
    return copy
  }, [resolved])

  const videoCount = useMemo(() => files.filter((f) => f.is_video).length, [files])
  const [sel, setSel] = useState(0)

  const move = (delta: number) => {
    const playable = files.map((f, i) => (f.is_video ? i : -1)).filter((i) => i >= 0)
    if (playable.length === 0) return
    const currentPos = playable.indexOf(sel)
    const next = currentPos < 0 ? 0 : (currentPos + delta + playable.length) % playable.length
    setSel(playable[next]!)
  }

  const play = (idx: number | null = null) => {
    const target = idx ?? sel
    const file = files[target]
    if (!resolved || !file || !file.is_video) return
    // Mismo contrato de `PlayerState` que usa Torrents.tsx —
    // Player arranca el stream él solo al montar. Sin tmdbId /
    // imdbId → no habrá backdrop; sin subs → el user abre el
    // panel de subs manualmente si quiere.
    nav('/player', {
      state: {
        magnet: resolved.magnet,
        title: resolved.name,
        imdbId: null,
        tmdbId: null,
        subPath: null,
        subRelease: null,
        startSeconds: 0,
        season: null,
        episode: null,
        isSeries: false,
        fileHint: file.file_id,
      },
    })
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: t('hotkey.move'), run: () => move(1) },
    { key: 'ArrowDown', hint: '', run: () => move(1) },
    { key: 'k', hint: '', run: () => move(-1) },
    { key: 'ArrowUp', hint: '', run: () => move(-1) },
    { key: 'Enter', hint: t('hotkey.play'), run: () => play() },
    { key: 'Escape', hint: t('hotkey.back'), run: () => nav('/') },
  ]
  useHotkeys(hotkeys, [sel, resolved])

  const barKeys: Hotkey[] = [
    { key: 'j', hint: t('hotkey.move'), run: () => {} },
    { key: 'Enter', hint: t('hotkey.play'), run: () => {} },
    { key: 'Escape', hint: t('hotkey.back'), run: () => {} },
  ]

  if (!resolved) return null

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        <button
          onClick={() => nav('/')}
          className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-[13px] text-body hover:border-border-strong"
        >
          {t('hotkey.back')}
        </button>
      </TopNav>

      <main className="mx-auto flex w-full max-w-[960px] flex-1 flex-col px-8 py-6">
        <div className="mb-4 flex items-baseline justify-between gap-4">
          <h1 className="flex min-w-0 items-center gap-2 text-[20px] font-semibold text-ink">
            <span className="text-accent">
              <MagnetIcon size={22} weight="duotone" />
            </span>
            <span className="shrink-0">{t('dropped.title')}</span>
            <span className="shrink-0 text-dim"> · </span>
            <span className="truncate text-body" title={resolved.name}>
              {resolved.name}
            </span>
          </h1>
          <span className="shrink-0 text-[11px] tabular-nums text-dim">
            {t('dropped.filesCount', {
              videos: String(videoCount),
              total: String(files.length),
            })}
          </span>
        </div>

        {videoCount === 0 ? (
          <div className="glass rounded-lg p-8 text-center">
            <p className="text-[14px] text-ink">{t('dropped.emptyTitle')}</p>
            <p className="mt-2 text-[13px] text-muted">{t('dropped.emptyHint')}</p>
          </div>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {files.map((f, i) => {
              const active = i === sel
              const playable = f.is_video
              return (
                <li key={f.file_id}>
                  <button
                    type="button"
                    onClick={() => {
                      if (!playable) return
                      setSel(i)
                      play(i)
                    }}
                    onMouseEnter={() => {
                      if (playable) setSel(i)
                    }}
                    disabled={!playable}
                    className={`focus-ring glass flex w-full items-center gap-4 rounded-lg px-4 py-3 text-left transition-transform ${
                      playable
                        ? active
                          ? 'scale-[1.005] outline outline-1 outline-white/30'
                          : 'hover:scale-[1.003]'
                        : 'opacity-40'
                    }`}
                  >
                    <span
                      className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-md ${
                        playable ? 'bg-accent/20 text-accent' : 'bg-surface text-dim'
                      }`}
                      aria-hidden
                    >
                      <FilmSlate size={16} weight="duotone" />
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-[14px] text-ink" title={f.name}>
                        {f.name}
                      </div>
                      {(f.season != null || f.episode != null) && (
                        <div className="mt-0.5 text-[11px] text-muted">
                          {f.season != null && f.episode != null
                            ? `S${String(f.season).padStart(2, '0')}E${String(f.episode).padStart(2, '0')}`
                            : f.season != null
                              ? `S${String(f.season).padStart(2, '0')}`
                              : `E${String(f.episode).padStart(2, '0')}`}
                        </div>
                      )}
                    </div>
                    <span className="shrink-0 text-[12px] tabular-nums text-dim">
                      {formatSize(f.size)}
                    </span>
                  </button>
                </li>
              )
            })}
          </ul>
        )}
      </main>

      <HotkeyBar hotkeys={barKeys} />
    </div>
  )
}

