import { useCallback, useEffect, useRef, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { ContextMenu, type ContextMenuItem } from '../components/ContextMenu'
import { HotkeyBar } from '../components/HotkeyBar'
import { ResumeDialog } from '../components/ResumeDialog'
import { SearchBox } from '../components/SearchBox'
import { StreamPanel } from '../components/StreamPanel'
import { TopNav } from '../components/TopNav'
import {
  audioFlag,
  ffmpegAvailable,
  formatSize,
  getMovieView,
  getPreferences,
  getResume,
  isTauri,
  openMagnet,
  searchTorrentsByTmdb,
  searchTorrentsDirect,
  searchTorrentsSeries,
  startStreamWithSub,
  stopStream,
  tmdbBackdrop,
  tmdbPoster,
  type MovieView,
  type ProviderStatus,
  type Resume,
  type StreamInfo,
  type Torrent,
  type TorrentSearchResult,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'
import { useT } from '../lib/i18n'

/**
 * Vista `View::Torrents` de la TUI. Recibe `mode` por props:
 * - `tmdb`: viene de Recs, hace `search_torrents_by_tmdb` con detalles TMDB.
 * - `direct`: viene de Search, hace `search_torrents_direct` con la query.
 * - `series`: viene de SeriesDetail; lee season/episode de la URL y
 *   dispara `search_torrents_series`. §7 audit series.
 *
 * Hotkeys:
 *   j/k mover · Enter proyectar (pregunta por subs → stream) ·
 *   s abre magnet en cliente BT externo · m toggle panel ·
 *   b Esc volver
 */
export function Torrents({ mode }: { mode: 'tmdb' | 'direct' | 'series' }) {
  const nav = useNavigate()
  const t = useT()
  const { tmdbId } = useParams<{ tmdbId?: string }>()
  const [params] = useSearchParams()
  // Series: season/episode desde la URL. En mode !== 'series' quedan
  // como null y no participan en ninguna llamada.
  const seasonParam = params.get('season')
  const episodeParam = params.get('episode')
  const season = seasonParam ? Number(seasonParam) : null
  const episode = episodeParam ? Number(episodeParam) : null

  const [result, setResult] = useState<TorrentSearchResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [sel, setSel] = useState(0)

  // Stream / VLC state
  const [stream, setStream] = useState<StreamInfo | null>(null)
  const [streamMsg, setStreamMsg] = useState<string | null>(null)

  // Subs state (modal sheet + gate dialog)
  // — eliminado: la selección de subs vive dentro del player HTML
  // ahora (panel embebido con tabs por idioma). Aquí solo iniciamos
  // el stream; el Player pide OpenSubtitles al arrancar.

  // Menú contextual (click derecho) sobre una fila de torrent.
  const [menu, setMenu] = useState<{
    x: number
    y: number
    index: number
  } | null>(null)

  // Resume state: se pregunta ANTES del stream cuando la caché tiene
  // una posición previa reproducible (fraction en 2%–95% y runtime
  // conocido). Ya no arrastra subPath/subRelease — la selección de
  // subs vive dentro del player HTML.
  const [resumePrompt, setResumePrompt] = useState<{
    fraction: number
    seconds: number
  } | null>(null)

  // Panel toggle: false = stream progress, true = magnet raw text
  const [showMagnet, setShowMagnet] = useState(false)

  // Detalle TMDB de la película (solo mode='tmdb'). Pintamos un
  // header estilo SeriesHeader con backdrop + poster + sinopsis por
  // encima de la lista de torrents. Antes esto vivía en un
  // `MovieDetailModal` que se abría al clicar una card en Recs;
  // pasarlo aquí elimina un paso intermedio y unifica el shape con
  // el flujo de series (donde SeriesDetail ya hace este papel).
  const [movieView, setMovieView] = useState<MovieView | null>(null)
  useEffect(() => {
    if (mode !== 'tmdb') return
    const id = Number(tmdbId ?? '')
    if (!Number.isFinite(id) || id <= 0) return
    let cancelled = false
    // No reseteamos `movieView` de forma síncrona aquí: react-hooks
    // v7 marca setState en cuerpo de efecto como "cascading render".
    // Cada ruta /torrents/tmdb/:id remonta el componente (react-router
    // trata los path params como key), así que no hay stale data
    // entre distintos ids. La primera carga simplemente ve `null`
    // hasta que el fetch resuelve.
    getMovieView(id)
      .then((v) => {
        if (!cancelled) setMovieView(v)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [mode, tmdbId])

  // Reproductor por defecto (preferencia del user). Cuando es `html`
  // y ffmpeg está en PATH, Enter/click enruta al player embebido en
  // vez de spawnear VLC. Si el user tiene `html` pero no tiene
  // ffmpeg, caemos silenciosamente a VLC con un aviso en el toast.
  const [defaultPlayer, setDefaultPlayer] = useState<'html' | 'vlc'>('html')
  const [ffmpegOk, setFfmpegOk] = useState<boolean | null>(null)
  useEffect(() => {
    if (!isTauri()) return
    void getPreferences()
      .then((p) => setDefaultPlayer(p.default_player))
      .catch(() => {})
    void ffmpegAvailable()
      .then(setFfmpegOk)
      .catch(() => setFfmpegOk(false))
  }, [])

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
    } else if (mode === 'series') {
      const id = Number(tmdbId ?? '')
      if (!Number.isFinite(id) || !season) {
        setError('Faltan tmdbId o temporada en la URL.')
        setLoading(false)
        return
      }
      searchTorrentsSeries(id, season, episode)
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
  }, [mode, tmdbId, params, season, episode])

  useEffect(() => {
    // react-hooks v7 marca `runSearch()` como setState-in-effect por
    // los `setLoading(true) / setError(null) / setResult(null) /
    // setSel(0)` síncronos. La alternativa oficial (`useEffectEvent`)
    // sigue experimental; los patrones de fetch-on-param-change no
    // tienen una forma limpia en React 19 sin librerías externas
    // (tanstack query, etc.). Disable con justificación hasta que
    // useEffectEvent estabilice.
    // eslint-disable-next-line react-hooks/set-state-in-effect
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
      setStreamMsg(t('torrents.magnet.sent', { name: current.title }))
    } catch (e) {
      setStreamMsg(t('torrents.magnet.error', { err: String(e) }))
    }
  }

  const goStream = async (
    subPath: string | null = null,
    subRelease: string | null = null,
    resumeSeconds: number | null = null,
    forceVlc = false,
    torrentOverride: Torrent | null = null,
  ) => {
    // React setState es asíncrono: si el caller viene de un onClick
    // que hizo `setSel(i)` + `startStreamFlow()`, `current` (que
    // depende de `sel`) todavía tiene el valor viejo del closure.
    // El caller puede pasar `torrentOverride` con el Torrent
    // recién clickeado para saltarse ese race. Los callers vía
    // hotkey/menú contextual no necesitan override porque `sel`
    // ya está sincronizado antes de que se lance el flow.
    const target = torrentOverride ?? current
    if (!target) return
    const useHtml = !forceVlc && defaultPlayer === 'html' && ffmpegOk !== false
    // Ruta preferida: player HTML embebido. La reproducción, subs y
    // resume viven en la view `/player`. Torrents queda como
    // seleccionador de fuente; el streaming lo arranca el propio Player.
    if (useHtml) {
      // `tmdbId` viaja al Player solo en modo tmdb/series (viene de
      // Recs, Search o SeriesDetail). El Player lo usa para pedir el
      // backdrop al arrancar y pintarlo detrás del loader al estilo
      // Stremio. En modo direct no lo tenemos → fondo negro sin
      // backdrop.
      const tmdbIdNum =
        (mode === 'tmdb' || mode === 'series') && tmdbId ? Number(tmdbId) : null
      nav('/player', {
        state: {
          magnet: target.magnet,
          title: result?.title ?? target.title,
          imdbId: result?.imdb_id ?? null,
          tmdbId: Number.isFinite(tmdbIdNum) ? tmdbIdNum : null,
          subPath,
          subRelease,
          startSeconds: resumeSeconds ?? 0,
          // Series: season/episode viajan al Player para que
          //   - seleccione el fichero correcto dentro de packs,
          //   - filtre subs por episodio,
          //   - persista resume con la clave file_id compuesta,
          //   - habilite el botón "siguiente episodio".
          season: mode === 'series' ? season : null,
          episode: mode === 'series' ? episode : null,
          isSeries: mode === 'series',
          // fileHint de Torrentio (§): índice pre-resuelto del
          // fichero dentro del torrent. Cuando existe, el backend
          // salta la heurística de parseo y sirve directamente ese
          // file — clave para packs con numeración de anime.
          fileHint: target.file_hint ?? null,
        },
      })
      return
    }
    // Fallback VLC (preferencia explícita del user o ffmpeg no
    // disponible en PATH). Mantiene la ruta legacy con proceso
    // externo. El aviso al user no menciona ffmpeg — la app decide
    // internamente cómo reproducir; si algo falla en la config
    // (dep faltante) usamos el player externo sin explicar el
    // motivo, para no exponer plumbing.
    if (defaultPlayer === 'html' && ffmpegOk === false && !forceVlc) {
      setStreamMsg(t('torrents.stream.vlcFallback'))
    } else {
      setStreamMsg(t('torrents.stream.starting', { name: target.title }))
    }
    if (stream) {
      await stopStream(stream.id).catch(() => {})
      setStream(null)
    }
    try {
      const info = await startStreamWithSub(
        target.magnet,
        subPath,
        resumeSeconds,
        mode === 'series' ? season : null,
        mode === 'series' ? episode : null,
        target.file_hint ?? null,
      )
      setStream(info)
      const subNote = subRelease ? `  \u00b7  sub: ${subRelease}` : ''
      const resumeNote = resumeSeconds
        ? `  \u00b7  reanudado en ${formatMinutes(resumeSeconds)}`
        : ''
      setStreamMsg(`${t('torrents.stream.started', { name: info.file_name })}${subNote}${resumeNote}`)
    } catch (e) {
      setStreamMsg(t('torrents.stream.error', { err: String(e) }))
    }
  }

  /**
   * Antes de arrancar el stream, consulta la caché a ver si hay un
   * resume guardado para este magnet. Prioridad de fuentes de tiempo,
   * de más precisa a menos:
   *
   *   1. `resume.seconds` + `resume.duration_seconds` — reportado por
   *      el player HTML. Precisión exacta y funciona sin TMDB.
   *   2. `resume.seconds` + `result.runtime_minutes * 60` — cuando
   *      el player reportó posición pero la duración de ffprobe
   *      llegó como 0/null. `runtime_minutes` de TMDB hace el papel.
   *   3. `resume.fraction` byte-based + `result.runtime_minutes` —
   *      camino legacy (path VLC / cachés antiguas). Necesita TMDB
   *      para convertir a segundos.
   *
   * En todos los casos abrimos el ResumeDialog si la posición cae
   * entre el 2% y el 95% del runtime. Fuera de ese rango o sin
   * datos, empezamos desde el principio.
   */
  const maybePromptResume = async (torrentOverride: Torrent | null = null) => {
    const target = torrentOverride ?? current
    if (!target) return
    let resume: Resume | null
    try {
      // Series: pasamos S/E → backend filtra a la entrada de ese
      // episodio dentro del store multi-file. Si no hay match
      // (nunca reproducido), devuelve null y saltamos el prompt.
      resume = await getResume(
        target.magnet,
        mode === 'series' ? season : null,
        mode === 'series' ? episode : null,
      )
    } catch {
      // Backend falla leyendo el resume → empezar limpio.
      resume = null
    }
    if (resume) {
      // Duración conocida por cualquier vía: la reportada por el
      // player (preferida por exactitud) o la de TMDB como respaldo.
      const knownDuration =
        resume.duration_seconds && resume.duration_seconds > 0
          ? resume.duration_seconds
          : result?.runtime_minutes
            ? result.runtime_minutes * 60
            : null

      // Fuente 1 y 2: si tenemos `seconds` y una duración por algún
      // lado, calculamos la fracción y decidimos.
      if (resume.seconds != null && knownDuration != null && knownDuration > 0) {
        const fraction = resume.seconds / knownDuration
        if (fraction > 0.02 && fraction < 0.95) {
          setResumePrompt({
            fraction,
            seconds: Math.round(resume.seconds),
          })
          return
        }
      } else if (
        // Fuente 3: solo `fraction` byte-based + runtime de TMDB.
        result?.runtime_minutes &&
        resume.fraction > 0.02 &&
        resume.fraction < 0.95
      ) {
        setResumePrompt({
          fraction: resume.fraction,
          seconds: Math.round(resume.fraction * result.runtime_minutes * 60),
        })
        return
      }
    }
    await goStream(null, null, null, false, target)
  }

  // Enter en la lista de torrents: prompt de resume (si hay caché) y
  // luego al player. La elección de subtítulos vive dentro del player
  // ahora (panel embebido estilo Stremio) — antes había un diálogo
  // intermedio ("¿con subs?" → SubsSheet → resume → player) que
  // añadía fricción sin valor.
  const startStreamFlow = (torrentOverride: Torrent | null = null) => {
    const target = torrentOverride ?? current
    if (!target) return
    void maybePromptResume(target)
  }

  const copyMagnet = async () => {
    if (!current) return
    try {
      await navigator.clipboard.writeText(current.magnet)
      setStreamMsg(t('torrents.magnet.copied'))
    } catch (e) {
      setStreamMsg(t('torrents.magnet.copyError', { err: String(e) }))
    }
  }

  const move = (delta: number) => {
    const n = torrents.length
    if (n === 0) return
    setSel((i) => (i + delta + n) % n)
  }

  const backTo =
    mode === 'tmdb'
      ? '/recs'
      : mode === 'series'
        ? tmdbId
          ? `/series/${tmdbId}`
          : '/search'
        : '/search'
  const goBack = () => {
    // En modo tmdb el user puede venir de /recs o de /search/results.
    // history.back respeta ambos flujos sin acoplar la vista al origen.
    if ((mode === 'tmdb' || mode === 'series') && window.history.length > 1) {
      nav(-1)
    } else {
      nav(backTo)
    }
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: '', run: () => move(1) },
    { key: 'ArrowDown', hint: '', run: () => move(1) },
    { key: 'k', hint: t('hotkey.move'), run: () => move(-1) },
    { key: 'ArrowUp', hint: '', run: () => move(-1) },
    { key: 'Enter', hint: t('hotkey.play'), run: startStreamFlow },
    { key: 's', hint: t('hotkey.magnet'), run: goMagnet },
    {
      key: 'm',
      hint: t('hotkey.panel'),
      run: () => setShowMagnet((v) => !v),
    },
    { key: 'b', hint: '', run: goBack },
    { key: 'Escape', hint: t('common.back'), run: goBack },
  ]
  // Cuando el prompt de resume o el menú contextual están abiertos,
  // sus hotkeys locales toman el control y las de la vista se
  // desactivan para no disparar handlers dobles.
  useHotkeys(hotkeys, [current, stream, backTo], {
    enabled: !resumePrompt && !menu,
  })

  const label = result?.title
    ? `${result.title}${result.year ? ` (${result.year})` : ''}${
        mode === 'series' && season
          ? episode
            ? ` · S${String(season).padStart(2, '0')}E${String(episode).padStart(2, '0')}`
            : ` · Temporada ${season}`
          : ''
      }`
    : mode === 'direct'
      ? params.get('q') ?? ''
      : ''

  return (
    <div className="flex h-[100dvh] flex-col bg-canvas">
      <TopNav>
        <SearchBox compact />
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
              ? t('torrents.searching')
              : result
                ? t('torrents.results', { n: torrents.length })
                : ''}
          </p>
        </div>

        {/* Fase 1b — línea de estado por provider. Se pinta bajo el
            título con la misma jerarquía visual que el contador de
            resultados: información secundaria pero visible. Cuando un
            provider timeoutea/falla, la lista corta ya no queda
            explicada como "no hay releases" sino como "knaben cayó". */}
        {result?.providers && result.providers.length > 0 && (
          <ProviderStatusLine providers={result.providers} />
        )}

        {/* Header rico de película (solo mode='tmdb'). Emula el
            SeriesHeader de SeriesDetail — backdrop de fondo + poster
            + metadata + sinopsis. Se pinta también sin resultados de
            torrents para dar contexto al "0 resultados". */}
        {mode === 'tmdb' && movieView && (
          <MovieHeader view={movieView} />
        )}

        {error && (
          <div className="rounded-sm border border-danger/40 bg-danger/10 p-4 text-[14px] text-danger">
            {error}
          </div>
        )}

        {loading && !error && (
          <TorrentSearchLoader />
        )}

        {!error && torrents.length === 0 && !loading && result && (
          <EmptyResultsPanel result={result} onBack={goBack} />
        )}

        {torrents.length > 0 && (
          <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-hairline">
            <div className="grid grid-cols-[3rem_1fr_5rem_4.5rem_4.5rem_4rem_4rem_5rem] items-center gap-x-3 border-b border-hairline bg-surface px-4 py-2 text-[11px] uppercase tracking-wide text-dim">
              <span>#</span>
              <span>{t('torrents.col.release')}</span>
              <span className="text-right">{t('torrents.col.size')}</span>
              <span className="text-right">{t('torrents.col.seeds')}</span>
              <span className="text-right">{t('torrents.col.leech')}</span>
              <span>{t('torrents.col.quality')}</span>
              <span>{t('torrents.col.audio')}</span>
              <span>{t('torrents.col.source')}</span>
            </div>
            <ul className="min-h-0 flex-1 overflow-y-auto">
              {torrents.map((tor, i) => (
                <TorrentRow
                  key={tor.magnet + i}
                  ref={(el: HTMLLIElement | null) => { rowsRef.current[i] = el }}
                  t={tor}
                  active={i === sel}
                  targetSeason={mode === 'series' ? season : null}
                  targetEpisode={mode === 'series' ? episode : null}
                  onClick={() => {
                    setSel(i)
                    // Un solo clic lanza el flujo (pregunta subs → stream).
                    // PASAMOS el torrent explícitamente: React setState es
                    // async, sin esto `startStreamFlow` leería `sel` viejo
                    // del closure y abriría siempre el que estaba
                    // seleccionado ANTES del click.
                    startStreamFlow(tor)
                  }}
                  onContextMenu={(x, y) => {
                    setSel(i)
                    setMenu({ x, y, index: i })
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

      {resumePrompt && (
        <ResumeDialog
          fraction={resumePrompt.fraction}
          seconds={resumePrompt.seconds}
          onResume={async () => {
            const p = resumePrompt
            setResumePrompt(null)
            await goStream(null, null, p.seconds)
          }}
          onRestart={async () => {
            setResumePrompt(null)
            await goStream(null, null, null)
          }}
          onClose={() => setResumePrompt(null)}
        />
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={((): ContextMenuItem[] => {
            const tor = torrents[menu.index]
            if (!tor) return []
            const usingHtml =
              defaultPlayer === 'html' && ffmpegOk !== false
            const primaryLabel = usingHtml
              ? t('torrents.menu.playHtml')
              : t('torrents.menu.playVlc')
            const items: ContextMenuItem[] = [
              {
                label: primaryLabel,
                hint: '\u21b5',
                onClick: startStreamFlow,
              },
            ]
            // Escape hatch: cuando la preferencia es player HTML,
            // ofrecer "Abrir en VLC" para este torrent puntual.
            // Al revés no tiene sentido: si ya es VLC el default,
            // la entrada primaria ya es VLC.
            if (usingHtml) {
              items.push({
                label: t('torrents.menu.playVlcOnce'),
                onClick: () => {
                  void goStream(null, null, null, true)
                },
              })
            }
            items.push(
              {
                label: t('torrents.menu.openClient'),
                hint: 's',
                onClick: goMagnet,
              },
              {
                label: t('torrents.menu.copyMagnet'),
                onClick: () => {
                  void copyMagnet()
                },
              },
            )
            // `tor` no se usaba aparte del early-return; mantenemos
            // el shadow por si futuras entradas necesitan datos del
            // torrent seleccionado (calidad, source, etc.).
            void tor
            return items
          })()}
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
  targetSeason,
  targetEpisode,
  onClick,
  onContextMenu,
}: {
  ref: (el: HTMLLIElement | null) => void
  t: Torrent
  active: boolean
  /** Cuando el user vino del flujo de serie con episodio pedido,
   * (S,E) — se pinta un chip `S02E03` delante del release name en
   * los packs para dejar claro que "aunque el release es un pack,
   * al proyectar te llevas ESE episodio". */
  targetSeason: number | null
  targetEpisode: number | null
  onClick: () => void
  onContextMenu: (x: number, y: number) => void
}) => {
  const flag = audioFlag(t.audio)
  // Sólo tiene sentido el chip si el user pidió un episodio concreto
  // Y el release es pack (los episodios sueltos ya se identifican con
  // su propio nombre + badge EP). Series-pack también gana el chip
  // porque el user perdería el foco si sólo viera "Complete Series".
  const showEpisodeChip =
    targetSeason != null &&
    targetEpisode != null &&
    (t.match_kind === 'season_pack' || t.match_kind === 'series_pack')
  const episodeLabel = showEpisodeChip
    ? `S${String(targetSeason).padStart(2, '0')}E${String(targetEpisode).padStart(2, '0')}`
    : null
  return (
    <li
      ref={ref}
      onClick={onClick}
      onContextMenu={(e) => {
        e.preventDefault()
        onContextMenu(e.clientX, e.clientY)
      }}
      className={`grid cursor-pointer grid-cols-[3rem_1fr_5rem_4.5rem_4.5rem_4rem_4rem_5rem] items-center gap-x-3 border-t border-hairline-soft px-4 py-2.5 text-[13px] transition-colors ${
        active ? 'bg-surface-hi text-ink' : 'text-body hover:bg-surface'
      }`}
    >
      <span
        className={`text-[11px] ${active ? 'text-accent' : 'text-dim'}`}
      >
        {active ? '▶' : ''}
      </span>
      <span className="flex min-w-0 items-center gap-2">
        {episodeLabel && (
          <span
            className="shrink-0 rounded-sm border border-accent/50 bg-accent/10 px-1.5 py-0 text-[10px] font-semibold tracking-wide text-accent"
            title={`Proyectarás este episodio dentro del pack`}
          >
            {episodeLabel}
          </span>
        )}
        <span className={`truncate ${episodeLabel ? 'text-muted' : ''}`}>
          {t.title}
        </span>
      </span>
      <span className="text-right tabular-nums text-warn">
        {formatSize(t.size_bytes)}
      </span>
      <span className="text-right tabular-nums text-good">↑{t.seeders}</span>
      <span className="text-right tabular-nums text-danger">↓{t.leechers}</span>
      <span className="text-info">
        {t.quality ?? '?'}
        <MatchKindBadge kind={t.match_kind} />
      </span>
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

/** Badge visual del `match_kind` del release. Muestra el tipo de
 * paquete cuando el user busca series: episodio suelto, pack de
 * temporada, o pack de serie completa. Para movies queda invisible
 * (el default `movie` no necesita distinción). §7 audit series. */
function MatchKindBadge({ kind }: { kind: Torrent['match_kind'] }) {
  if (kind === 'movie') return null
  const label =
    kind === 'episode'
      ? 'EP'
      : kind === 'season_pack'
        ? 'PACK'
        : 'SERIE'
  const cls =
    kind === 'episode'
      ? 'border-good/40 text-good'
      : kind === 'season_pack'
        ? 'border-warn/40 text-warn'
        : 'border-info/40 text-info'
  return (
    <span
      className={`ml-2 rounded-sm border ${cls} px-1 py-0 text-[9px] font-semibold uppercase tracking-wide`}
    >
      {label}
    </span>
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

/** Loader con mensajes rotatorios que reflejan las tres fases reales
 * del pipeline: (1) providers en paralelo, (2) filtros anti-basura,
 * (3) sondeo de seeders. El user ve que "está pasando algo" en vez
 * de un "Buscando…" estático. Los mensajes rotan cada 1.8s para no
 * marear. */
function TorrentSearchLoader() {
  const messages = [
    'Consultando indexadores (YTS, PirateBay, Knaben)\u2026',
    'Filtrando series de TV, CAMs y torrents muertos\u2026',
    'Refinando resultados por calidad y seeders\u2026',
  ]
  const [msgIdx, setMsgIdx] = useState(0)
  useEffect(() => {
    const id = window.setInterval(() => {
      setMsgIdx((i) => (i + 1) % messages.length)
    }, 1800)
    return () => window.clearInterval(id)
  }, [messages.length])
  return (
    <div className="flex flex-col items-center justify-center rounded-lg border border-hairline bg-surface px-6 py-12">
      <div className="h-8 w-8 animate-spin rounded-full border-2 border-accent border-t-transparent" />
      <p className="mt-4 text-[14px] font-medium text-ink">
        {messages[msgIdx]}
      </p>
      <p className="mt-1 text-[11px] text-muted">
        Los indexadores tardan unos segundos. Consultamos varios en
        paralelo y luego descartamos la basura antes de mostrarte la lista.
      </p>
    </div>
  )
}

/**
 * Línea de estado por provider (Fase 1b). Formato compacto tipo
 * `knaben ✓ 34 · apibay ✗ timeout · yts ✓ 5`, sin ruido: solo el
 * nombre, un tick / cruz, y el número o el motivo. Se pinta en la
 * cabecera de la vista para que el user pueda leerla de un vistazo
 * sin bloquear la lista de resultados.
 *
 * Diseño intencional: pequeño (12px), muted, tabular-nums para que
 * los conteos alineen entre búsquedas consecutivas. No pintamos
 * `elapsed_ms` por defecto para no saturar; el tooltip del span lo
 * expone si el user pasa el ratón.
 */
function ProviderStatusLine({ providers }: { providers: ProviderStatus[] }) {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] tabular-nums text-dim">
      {providers.map((p) => {
        const cached = p.from_cache === true
        const tooltip = p.ok
          ? `${p.hits} hits en ${p.elapsed_ms} ms${p.retried ? ' (con retry)' : ''}${cached ? ' · caché' : ''}`
          : `${p.error ?? 'error'} · ${p.elapsed_ms} ms`
        return (
          <span key={p.name} title={tooltip} className="flex items-center gap-1">
            <span className="text-muted">{p.name}</span>
            {p.ok ? (
              <span className={cached ? 'text-info' : 'text-good'}>
                {cached ? '↺' : '✓'} {p.hits}
              </span>
            ) : (
              <span className="text-danger">✗ {p.error ?? 'error'}</span>
            )}
            {p.retried && (
              <span className="text-warn" title="Reintento aplicado">
                ↻
              </span>
            )}
          </span>
        )
      })}
    </div>
  )
}

/**
 * Panel de estado vacío (Fase 4b — audit).
 *
 * En vez del genérico "Sin resultados", distinguimos tres casos:
 *
 *   1. Peli reciente / futura (release_date dentro de los últimos ~10
 *      semanas o en el futuro) → mensaje "en cines, sin releases
 *      digitales todavía". Es el caso más frecuente de vacío legítimo
 *      (blockbusters recién estrenados como Spider-Man Brand New Day).
 *   2. Todos los providers fallaron / timeout → la lista está vacía
 *      porque no llegamos a los indexadores. Sugerimos reintentar.
 *   3. Fallback genérico: los indexadores respondieron pero nada pasó
 *      los filtros. Sugerimos revisar tipo/año.
 *
 * El objetivo es que el user entienda POR QUÉ está vacío sin pensar
 * que la app está rota.
 */
function EmptyResultsPanel({
  result,
  onBack,
}: {
  result: TorrentSearchResult
  onBack: () => void
}) {
  const providers = result.providers ?? []
  const allProvidersFailed =
    providers.length > 0 && providers.every((p) => !p.ok)

  // `Date.now()` viola `react-hooks/purity` en el cuerpo del componente.
  // Cacheamos el "ahora" en un useState lazy — se calcula una sola vez
  // al montar y sirve para comparar contra `release_date` sin
  // reactividad (el panel es de vida corta: aparece cuando no hay
  // resultados y desaparece con el próximo re-search).
  const [now] = useState(() => Date.now())

  // Peli en cines o próxima: `release_date` de TMDB dentro de los
  // últimos 70 días o en el futuro. Usamos 10 semanas ≈ 70 días: es
  // el gap típico entre estreno en cines y ventana digital / VOD.
  // Si no tenemos fecha (búsqueda directa, o TMDB sin datos), no
  // podemos afirmar nada — fallback genérico.
  const stillInCinemas = (() => {
    if (!result.release_date) return false
    const d = new Date(result.release_date)
    if (Number.isNaN(d.getTime())) return false
    const seventyDaysMs = 70 * 24 * 3600 * 1000
    return d.getTime() > now - seventyDaysMs
  })()

  const formattedDate = (() => {
    if (!result.release_date) return null
    const d = new Date(result.release_date)
    if (Number.isNaN(d.getTime())) return null
    return d.toLocaleDateString('es-ES', {
      day: 'numeric',
      month: 'long',
      year: 'numeric',
    })
  })()

  const label = result.title || 'esta película'

  return (
    <div className="rounded-lg border border-hairline bg-surface p-8 text-center">
      {stillInCinemas ? (
        <>
          <p className="text-[16px] text-ink">Aún no hay releases digitales.</p>
          <p className="mt-2 text-[13px] text-body">
            <span className="font-medium text-ink">{label}</span>{' '}
            {formattedDate ? `se estrenó en cines el ${formattedDate}.` : 'está actualmente en cines.'}
          </p>
          <p className="mt-1 text-[13px] text-muted">
            La ventana entre estreno y digital suele ser de 6-10 semanas.
            Los grupos de scene esperan a que salga la copia limpia
            (WEB-DL / BluRay) — cualquier cosa que aparezca antes es un
            CAM y videodrome los filtra siempre. Vuelve en unas semanas.
          </p>
        </>
      ) : allProvidersFailed ? (
        <>
          <p className="text-[16px] text-ink">Los indexadores no respondieron.</p>
          <p className="mt-2 text-[13px] text-muted">
            Todos los proveedores fallaron (timeout, red o servidor).
            Prueba a reintentar en unos segundos.
          </p>
        </>
      ) : (
        <>
          <p className="text-[16px] text-ink">Sin resultados para "{label}".</p>
          <p className="mt-2 text-[13px] text-muted">
            Los indexadores respondieron pero nada pasó los filtros
            (series de TV, CAMs y torrents con &lt;3 seeders se
            descartan). Prueba otro título o revisa el año.
          </p>
        </>
      )}
      <button
        onClick={onBack}
        className="mt-5 rounded-full border border-hairline px-4 py-1.5 text-[13px] text-body hover:border-border-strong"
      >
        Volver
      </button>
    </div>
  )
}

/** Header rico de película: backdrop de fondo + poster + metadata +
 * sinopsis. Diseño alineado con `SeriesHeader` (SeriesDetail.tsx) para
 * que ambos flujos —serie y peli— den la misma sensación al llegar a
 * la selección de release. Antes esta info vivía en un modal previo
 * que forzaba un click extra por peli; ahora es contexto inline.
 *
 * `line-clamp-4` porque la lista de torrents es el foco de la vista:
 * si el user quiere la sinopsis completa la puede leer con hover del
 * navegador (title attr no aplica bien a párrafos, así que mostramos
 * lo que cabe). En panel colapsable evitamos crecer la vista y
 * empujar la tabla fuera de viewport.
 */
function MovieHeader({ view }: { view: MovieView }) {
  const backdrop = tmdbBackdrop(view.backdrop_path)
  const poster = tmdbPoster(view.poster_path)
  const year = view.release_date?.slice(0, 4) ?? null
  const runtime = view.runtime
  const rating = view.vote_average
  const genres = view.genres

  return (
    <div className="relative overflow-hidden rounded-lg border border-hairline bg-surface">
      {backdrop && (
        <div
          className="absolute inset-0 bg-cover bg-center opacity-25"
          style={{ backgroundImage: `url(${backdrop})` }}
          aria-hidden
        />
      )}
      <div className="relative flex gap-5 p-5">
        {poster && (
          <img
            src={poster}
            alt={view.title}
            className="h-[168px] w-[112px] shrink-0 rounded-poster object-cover shadow-lg"
            draggable={false}
          />
        )}
        <div className="min-w-0 flex-1">
          <div className="mb-1 flex items-baseline gap-3">
            <h2 className="truncate text-[18px] font-semibold text-ink">
              {view.title}
            </h2>
            {year && (
              <span className="shrink-0 text-[13px] text-muted">{year}</span>
            )}
          </div>
          <div className="mb-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-dim">
            {runtime != null && runtime > 0 && (
              <span>{formatRuntime(runtime)}</span>
            )}
            {rating > 0 && (
              <>
                {runtime != null && runtime > 0 && <span aria-hidden>·</span>}
                <span className="inline-flex items-center gap-1">
                  <span className="text-accent">★</span>
                  {rating.toFixed(1)}
                </span>
              </>
            )}
            {genres.length > 0 && (
              <>
                <span aria-hidden>·</span>
                <span className="truncate">{genres.slice(0, 4).join(' · ')}</span>
              </>
            )}
          </div>
          {view.tagline && (
            <p className="mb-2 truncate text-[12px] italic text-muted">
              {view.tagline}
            </p>
          )}
          {view.overview && (
            <p className="line-clamp-4 text-[12.5px] leading-relaxed text-body">
              {view.overview}
            </p>
          )}
        </div>
      </div>
    </div>
  )
}

function formatRuntime(mins: number): string {
  const h = Math.floor(mins / 60)
  const m = mins % 60
  if (h === 0) return `${m} min`
  if (m === 0) return `${h} h`
  return `${h} h ${m} min`
}

