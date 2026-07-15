import { useCallback, useEffect, useRef, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { AskSubsDialog } from '../components/AskSubsDialog'
import { HotkeyBar } from '../components/HotkeyBar'
import { ResumeDialog } from '../components/ResumeDialog'
import { StreamPanel } from '../components/StreamPanel'
import { SubsSheet } from '../components/SubsSheet'
import { TopNav } from '../components/TopNav'
import {
  audioFlag,
  downloadSubtitle,
  formatSize,
  getResume,
  isTauri,
  openMagnet,
  searchSubtitles,
  searchTorrentsByTmdb,
  searchTorrentsDirect,
  startStreamWithSub,
  stopStream,
  type Resume,
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
 * Hotkeys:
 *   j/k mover · Enter proyectar (pregunta por subs → stream) ·
 *   s abre magnet en cliente BT externo · m toggle panel ·
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

  // Subs state (modal sheet + gate dialog)
  const [subsOpen, setSubsOpen] = useState(false)
  const [askSubsOpen, setAskSubsOpen] = useState(false)
  const [subsLoading, setSubsLoading] = useState(false)
  const [subs, setSubs] = useState<Subtitle[]>([])
  const [pendingSubPath, setPendingSubPath] = useState<string | null>(null)

  // Resume state: se pregunta ANTES del stream cuando la caché tiene
  // una posición previa reproducible (fraction en 2%–95% y runtime
  // conocido).
  const [resumePrompt, setResumePrompt] = useState<{
    fraction: number
    seconds: number
    subPath: string | null
    subRelease: string | null
  } | null>(null)

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

  const goStream = async (
    subPath: string | null = null,
    subRelease: string | null = null,
    resumeSeconds: number | null = null,
  ) => {
    if (!current) return
    setStreamMsg(`Iniciando stream: ${current.title}…`)
    if (stream) {
      await stopStream(stream.id).catch(() => {})
      setStream(null)
    }
    try {
      const info = await startStreamWithSub(current.magnet, subPath, resumeSeconds)
      setStream(info)
      const subNote = subRelease ? `  ·  sub: ${subRelease}` : ''
      const resumeNote = resumeSeconds
        ? `  ·  reanudado en ${formatMinutes(resumeSeconds)}`
        : ''
      setStreamMsg(`Streaming ${info.file_name}${subNote}${resumeNote}`)
    } catch (e) {
      setStreamMsg(`Error stream: ${String(e)}`)
    }
  }

  /**
   * Antes de arrancar el stream, consulta la caché a ver si hay un
   * resume guardado para este magnet. Si la fracción está en el rango
   * "útil" (2%–95%) y conocemos el runtime de TMDB (solo modo `tmdb`),
   * abrimos el ResumeDialog en lugar de arrancar directo — el usuario
   * decide reanudar o empezar de cero. Sin runtime no podemos convertir
   * bytes→segundos, así que empezamos siempre desde el principio.
   */
  const maybePromptResume = async (
    subPath: string | null,
    subRelease: string | null,
  ) => {
    if (!current) return
    if (result?.runtime_minutes) {
      try {
        const r: Resume | null = await getResume(current.magnet)
        if (r && r.fraction > 0.02 && r.fraction < 0.95) {
          const seconds = Math.round(r.fraction * result.runtime_minutes * 60)
          setResumePrompt({
            fraction: r.fraction,
            seconds,
            subPath,
            subRelease,
          })
          return
        }
      } catch {
        // Si el backend falla leyendo el resume, fallback a empezar
        // desde el principio en lugar de bloquear al user.
      }
    }
    await goStream(subPath, subRelease, null)
  }

  // Enter en la lista de torrents: preguntar por subs antes de streamear.
  // Si no hay torrent seleccionado, no-op.
  const startStreamFlow = () => {
    if (!current) return
    setAskSubsOpen(true)
  }

  const confirmStreamWithSubs = () => {
    setAskSubsOpen(false)
    openSubs()
  }

  const confirmStreamWithoutSubs = async () => {
    setAskSubsOpen(false)
    setPendingSubPath(null)
    await maybePromptResume(null, null)
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
      const release = sub.release || sub.file_name || 'sub'
      setPendingSubPath(path)
      setSubsOpen(false)
      // Encadenar con el stream: el usuario ya confirmó "con subs" en
      // el diálogo previo, no hay que volver a pedir Enter.
      await maybePromptResume(path, release)
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
    { key: 'Enter', hint: 'Proyectar', run: startStreamFlow },
    { key: 's', hint: 'Magnet', run: goMagnet },
    {
      key: 'm',
      hint: 'Panel',
      run: () => setShowMagnet((v) => !v),
    },
    { key: 'b', hint: '', run: goBack },
    { key: 'Escape', hint: 'Volver', run: goBack },
  ]
  // Cuando cualquier modal (subs sheet, diálogo de subs o el prompt de
  // resume) está abierto, sus hotkeys locales toman el control y las de
  // la vista se desactivan para no disparar handlers dobles.
  useHotkeys(hotkeys, [current, stream, pendingSubPath, backTo], {
    enabled: !subsOpen && !askSubsOpen && !resumePrompt,
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
                  onClick={() => {
                    setSel(i)
                    // Un solo clic lanza el flujo (pregunta subs → stream).
                    // Los power-users siguen usando j/k + Enter con teclado.
                    startStreamFlow()
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
            onPlayerDied={() => {
              // El backend ya se limpió a sí mismo cuando detectó que
              // VLC murió, aquí solo tenemos que actualizar la UI.
              if (stream) {
                stopStream(stream.id).catch(() => {})
              }
              setStream(null)
              setStreamMsg('Stream detenido: VLC cerrado.')
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

      {askSubsOpen && (
        <AskSubsDialog
          title={current?.title ?? ''}
          onYes={confirmStreamWithSubs}
          onNo={confirmStreamWithoutSubs}
          onClose={() => setAskSubsOpen(false)}
        />
      )}

      {resumePrompt && (
        <ResumeDialog
          fraction={resumePrompt.fraction}
          seconds={resumePrompt.seconds}
          onResume={async () => {
            const p = resumePrompt
            setResumePrompt(null)
            await goStream(p.subPath, p.subRelease, p.seconds)
          }}
          onRestart={async () => {
            const p = resumePrompt
            setResumePrompt(null)
            await goStream(p.subPath, p.subRelease, null)
          }}
          onClose={() => setResumePrompt(null)}
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
}: {
  ref: (el: HTMLLIElement | null) => void
  t: Torrent
  active: boolean
  onClick: () => void
}) => {
  const flag = audioFlag(t.audio)
  return (
    <li
      ref={ref}
      onClick={onClick}
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

/** Formatea segundos como `MM:SS` o `H:MM:SS` — usado en el toast de
 * "streaming reanudado en …". Deliberadamente duplicado de
 * `ResumeDialog.formatHms` para no crear un módulo utils compartido
 * solo por dos usos. */
function formatMinutes(total: number): string {
  const s = Math.max(0, Math.floor(total))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  const pad = (n: number) => n.toString().padStart(2, '0')
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`
}