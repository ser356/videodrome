import { useCallback, useEffect, useRef, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { HotkeyBar } from '../components/HotkeyBar'
import { StreamPanel } from '../components/StreamPanel'
import { SubsSheet } from '../components/SubsSheet'
import { TopNav } from '../components/TopNav'
import {
  audioFlag,
  downloadSubtitle,
  formatSize,
  isTauri,
  openMagnet,
  searchSubtitles,
  searchTorrentsByTmdb,
  searchTorrentsDirect,
  startStreamWithSub,
  stopStream,
  type StreamInfo,
  type Subtitle,
  type Torrent,
  type TorrentSearchResult,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

/**
 * Vista `View::Torrents` de la TUI. Recibe `mode` por props:
 * - `tmdb`: viene de Recs, hace `search_torrents_by_tmdb` con detalles TMDB.
 * - `direct`: viene de Search, hace `search_torrents_direct` con la query.
 *
 * Hotkeys 1:1 con la TUI:
 *   j/k mover · Enter magnet · s stream · x subtítulos · m toggle panel
 *   b Esc volver
 */
export function Torrents({ mode }: { mode: 'tmdb' | 'direct' }) {
  const nav = useNavigate()
  const { tmdbId } = useParams<{ tmdbId?: string }>()
  const [params] = useSearchParams()

  const [result, setResult] = useState<TorrentSearchResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [sel, setSel] = useState(0)

  // Stream / VLC state
  const [stream, setStream] = useState<StreamInfo | null>(null)
  const [streamMsg, setStreamMsg] = useState<string | null>(null)

  // Subs state (modal sheet)
  const [subsOpen, setSubsOpen] = useState(false)
  const [subsLoading, setSubsLoading] = useState(false)
  const [subs, setSubs] = useState<Subtitle[]>([])
  const [pendingSubPath, setPendingSubPath] = useState<string | null>(null)
  const [pendingSubRelease, setPendingSubRelease] = useState<string | null>(null)

  // Panel toggle: false = stream progress, true = magnet raw text
  const [showMagnet, setShowMagnet] = useState(false)

  // Scroll selected row into view whenever selection changes
  const rowsRef = useRef<Array<HTMLLIElement | null>>([])

  const runSearch = useCallback(() => {
    if (!isTauri()) {
      setError('Esta vista requiere la app de escritorio (Tauri).')
      return
    }
    setLoading(true)
    setError(null)
    setResult(null)
    setSel(0)

    if (mode === 'tmdb') {
      const id = Number(tmdbId ?? '')
      const title = params.get('title') ?? ''
      const year = params.get('year')
      searchTorrentsByTmdb(id, title, year ? Number(year) : null)
        .then(setResult)
        .catch((e) => setError(String(e)))
        .finally(() => setLoading(false))
    } else {
      const q = params.get('q') ?? ''
      searchTorrentsDirect(q)
        .then(setResult)
        .catch((e) => setError(String(e)))
        .finally(() => setLoading(false))
    }
  }, [mode, tmdbId, params])

  useEffect(() => {
    runSearch()
  }, [runSearch])

  useEffect(() => {
    rowsRef.current[sel]?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [sel])

  // Clean up the stream handle if the user navigates away while
  // streaming. Backend `Drop` also cancels, but this releases the
  // slot in the streams HashMap.
  useEffect(() => {
    return () => {
      if (stream) stopStream(stream.id).catch(() => {})
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const torrents = result?.results ?? []
  const current = torrents[sel]

  const goMagnet = async () => {
    if (!current) return
    try {
      await openMagnet(current.magnet)
      setStreamMsg(`Magnet enviado al cliente por defecto: ${current.title}`)
    } catch (e) {
      setStreamMsg(`Error abriendo magnet: ${String(e)}`)
    }
  }

  const goStream = async () => {
    if (!current) return
    setStreamMsg(`Iniciando stream: ${current.title}…`)
    if (stream) {
      await stopStream(stream.id).catch(() => {})
      setStream(null)
    }
    try {
      const info = await startStreamWithSub(current.magnet, pendingSubPath)
      setStream(info)
      const subNote = pendingSubRelease ? `  ·  sub: ${pendingSubRelease}` : ''
      setStreamMsg(`Streaming ${info.file_name}${subNote}`)
    } catch (e) {
      setStreamMsg(`Error stream: ${String(e)}`)
    }
  }

  const openSubs = async () => {
    if (!current) return
    setSubsOpen(true)
    setSubsLoading(true)
    setSubs([])
    try {
      const list = await searchSubtitles(result?.imdb_id ?? null, current.title)
      setSubs(list)
    } catch (e) {
      setStreamMsg(`Error subs: ${String(e)}`)
      setSubsOpen(false)
    } finally {
      setSubsLoading(false)
    }
  }

  const chooseSub = async (sub: Subtitle) => {
    setSubsLoading(true)
    try {
      const path = await downloadSubtitle(sub)
      setPendingSubPath(path)
      setPendingSubRelease(sub.release || sub.file_name || 'sub')
      setStreamMsg(`Sub cargado (${sub.language}): pulsa S para stream con subs.`)
      setSubsOpen(false)
    } catch (e) {
      setStreamMsg(`Error descargando sub: ${String(e)}`)
    } finally {
      setSubsLoading(false)
    }
  }

  const move = (delta: number) => {
    const n = torrents.length
    if (n === 0) return
    setSel((i) => (i + delta + n) % n)
  }

  const backTo = mode === 'tmdb' ? '/recs' : '/search'
  const goBack = () => {
    // En modo tmdb el user puede venir de /recs o de /search/results.
    // history.back respeta ambos flujos sin acoplar la vista al origen.
    if (mode === 'tmdb' && window.history.length > 1) {
      nav(-1)
    } else {
      nav(backTo)
    }
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: '', run: () => move(1) },
    { key: 'ArrowDown', hint: '', run: () => move(1) },
    { key: 'k', hint: 'Mover', run: () => move(-1) },
    { key: 'ArrowUp', hint: '', run: () => move(-1) },
    { key: 'Enter', hint: 'Magnet', run: goMagnet },
    { key: 's', hint: 'Stream', run: goStream },
    { key: 'x', hint: 'Subtítulos', run: openSubs },
    {
      key: 'm',
      hint: 'Panel',
      run: () => setShowMagnet((v) => !v),
    },
    { key: 'b', hint: '', run: goBack },
    { key: 'Escape', hint: 'Volver', run: goBack },
  ]
  // Cuando la SubsSheet está abierta, sus hotkeys (Enter, j/k, Esc) toman
  // el control. Si dejamos las de Torrents activas, Enter dispara AMBOS
  // handlers → se abre qBittorrent al elegir un subtítulo.
  useHotkeys(hotkeys, [current, stream, pendingSubPath, backTo], {
    enabled: !subsOpen,
  })

  const label = result?.title
    ? `${result.title}${result.year ? ` (${result.year})` : ''}`
    : mode === 'direct'
      ? params.get('q') ?? ''
      : ''

  return (
    <div className="flex h-[100dvh] flex-col bg-canvas">
      <TopNav>
        {pendingSubPath && (
          <span className="rounded-full border border-good/40 bg-good/10 px-3 py-1 text-[12px] text-good">
            Sub listo
          </span>
        )}
        <button
          onClick={goBack}
          className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-body hover:border-border-strong"
        >
          Volver
        </button>
      </TopNav>

      <main className="mx-auto flex h-full min-h-0 w-full max-w-[1400px] flex-1 flex-col gap-4 px-8 py-6">
        <div className="flex items-baseline justify-between">
          <h1 className="text-[20px] font-semibold text-ink">
            🧲 Torrents{' '}
            <span className="text-muted">
              {label ? '· ' : ''}
              {label}
            </span>
          </h1>
          <p className="text-[12px] tabular-nums text-dim">
            {loading
              ? 'Buscando…'
              : result
                ? `${torrents.length} resultados`
                : ''}
          </p>
        </div>

        {error && (
          <div className="rounded-sm border border-danger/40 bg-danger/10 p-4 text-[14px] text-danger">
            {error}
          </div>
        )}

        {!error && torrents.length === 0 && !loading && result && (
          <div className="rounded-sm border border-hairline bg-surface p-10 text-center">
            <p className="text-[16px] text-ink">Sin resultados.</p>
            <p className="mt-1 text-[13px] text-muted">
              Los indexadores no encontraron nada para "{label}".
            </p>
          </div>
        )}

        {torrents.length > 0 && (
          <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-hairline">
            <div className="grid grid-cols-[3rem_1fr_5rem_4.5rem_4.5rem_4rem_4rem_5rem] items-center gap-x-3 border-b border-hairline bg-surface px-4 py-2 text-[11px] uppercase tracking-wide text-dim">
              <span>#</span>
              <span>Release</span>
              <span className="text-right">Tamaño</span>
              <span className="text-right">Seeds</span>
              <span className="text-right">Leech</span>
              <span>Calidad</span>
              <span>Audio</span>
              <span>Fuente</span>
            </div>
            <ul className="min-h-0 flex-1 overflow-y-auto">
              {torrents.map((t, i) => (
                <TorrentRow
                  key={t.magnet + i}
                  ref={(el: HTMLLIElement | null) => { rowsRef.current[i] = el }}
                  t={t}
                  active={i === sel}
                  onClick={() => setSel(i)}
                  onDoubleClick={() => {
                    setSel(i)
                    goMagnet()
                  }}
                />
              ))}
            </ul>
          </div>
        )}

        <div className="shrink-0">
          <StreamPanel
            showMagnet={showMagnet}
            magnet={current?.magnet}
            stream={stream}
            message={streamMsg}
            onStopStream={async () => {
              if (stream) {
                await stopStream(stream.id).catch(() => {})
                setStream(null)
                setStreamMsg('Stream detenido.')
              }
            }}
          />
        </div>
      </main>

      <HotkeyBar hotkeys={hotkeys.filter((h) => h.hint)} />

      {subsOpen && (
        <SubsSheet
          subs={subs}
          loading={subsLoading}
          onPick={chooseSub}
          onClose={() => setSubsOpen(false)}
        />
      )}
    </div>
  )
}

// ------- Row -------

const TorrentRow = ({
  ref,
  t,
  active,
  onClick,
  onDoubleClick,
}: {
  ref: (el: HTMLLIElement | null) => void
  t: Torrent
  active: boolean
  onClick: () => void
  onDoubleClick: () => void
}) => {
  const flag = audioFlag(t.audio)
  return (
    <li
      ref={ref}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      className={`grid cursor-pointer grid-cols-[3rem_1fr_5rem_4.5rem_4.5rem_4rem_4rem_5rem] items-center gap-x-3 border-t border-hairline-soft px-4 py-2.5 text-[13px] transition-colors ${
        active ? 'bg-surface-hi text-ink' : 'text-body hover:bg-surface'
      }`}
    >
      <span
        className={`text-[11px] ${active ? 'text-accent' : 'text-dim'}`}
      >
        {active ? '▶' : ''}
      </span>
      <span className="truncate">{t.title}</span>
      <span className="text-right tabular-nums text-warn">
        {formatSize(t.size_bytes)}
      </span>
      <span className="text-right tabular-nums text-good">↑{t.seeders}</span>
      <span className="text-right tabular-nums text-danger">↓{t.leechers}</span>
      <span className="text-info">{t.quality ?? '?'}</span>
      <span
        className="inline-flex items-center gap-1 text-[12px] text-body"
        title={flag.label}
      >
        <span className="text-[14px] leading-none">{flag.flag}</span>
        <span className="text-[10px] uppercase tracking-wide text-dim">
          {flag.label}
        </span>
      </span>
      <span className="text-[11px] text-dim">{t.source}</span>
    </li>
  )
}