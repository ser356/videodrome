import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import Hls from 'hls.js'
import {
  ArrowsIn,
  ArrowsOut,
  CaretLeft,
  ClosedCaptioning,
  DownloadSimple,
  Gauge,
  MusicNotes,
  Pause,
  Play,
} from '@phosphor-icons/react'
import {
  downloadSubtitle,
  fetchEmbeddedSubtitle,
  getMovieView,
  getPreferences,
  getResume,
  hlsUrl,
  probeStream,
  ProbeStalledError,
  searchSubtitles,
  setAudioTrack,
  startStreamHtml,
  stopStream,
  streamStats,
  subtitleToVtt,
  tmdbBackdrop,
  type MediaInfo,
  type MediaStream,
  type StreamInfo,
  type StreamStats,
  type Subtitle,
} from '../lib/api'
import { getLocale, mergeSubtitleLangs, useT } from '../lib/i18n'
import { ffmpegInstallHint, isBitmapSubCodec } from './player/utils'
import { formatSpeed, formatTime } from './player/utils'
import { SeekBar, VolumeControl, StatsPanel } from './player/controls'
import { AudioPanel, SubsPanel } from './player/panels'
import { StremioLoader } from './player/StremioLoader'
import { useResumePosition } from './player/useResumePosition'
import { useHotkeys } from './player/useHotkeys'

/**
 * Player HTML embebido. Reemplaza el spawn de VLC cuando la
 * preferencia `default_player = "html"`. La navegación llega vía
 * `nav('/player', { state: PlayerState })` desde `Torrents.tsx`, que
 * mantiene el resto del flujo (elección de subs, prompt de resume)
 * intacto — este componente solo se ocupa de reproducir.
 *
 * Arquitectura:
 *
 *   [librqbit] --stream--> /video (raw MKV/MP4 con Range)
 *                                          |
 *                                          | HTTP interno
 *                                          v
 *   ┌─ direct_playable (MP4/MOV + H.264/HEVC 8-bit + AAC/MP3) ──┐
 *   │                                                            │
 *   │   <video src="…/video">   ← path DIRECT, seek nativo       │
 *   │                                                            │
 *   └─ resto (MKV, HEVC 10-bit, VP9, opus…) ─────────────────────┘
 *                                          |
 *                                          v
 *   [ffmpeg] --transmux on demand--> /hls/playlist.m3u8 (VOD)
 *                                          |
 *                                          v
 *   <video src="…/hls/playlist.m3u8">  ← este componente
 *
 * En modo TRANSMUX el playlist es VOD estático (función pura de la
 * duración; enumera TODOS los segmentos desde arranque con
 * `#EXT-X-ENDLIST`). Los segmentos `.ts` los materializa ffmpeg
 * bajo demanda en el backend cuando el `<video>` los pide. El seek
 * es 100% nativo: `v.currentTime = t` — sin reasignar `src`, sin
 * cache-busting, sin prefetch. Los PTS del TS son tiempos absolutos
 * (`-output_ts_offset` en el spawn) → `currentTime`, `<track>` de
 * subs y timeline coherentes tras cualquier seek.
 */

interface PlayerState {
  magnet: string
  title: string
  imdbId: string | null
  /** TMDB id opcional para pedir backdrop (peli sale desde Recs o
   * Search por texto). `null` en modo directo o si no lo conocemos
   * — el loader cae a fondo negro sin backdrop. */
  tmdbId?: number | null
  /** Path local del `.srt` ya descargado por el flujo previo. `null`
   * si el usuario eligió reproducir sin subs. TODO: pasar por el
   * endpoint SRT→VTT del backend cuando esté (Fase 2). */
  subPath: string | null
  subRelease: string | null
  /** Segundo de arranque para reanudar. `0` = empezar de cero. */
  startSeconds: number
  /** Metadata de episodio (§7 audit). Cuando `isSeries` es true y
   * viene `season`/`episode`, el Player:
   *   - selecciona el fichero correcto dentro de packs (backend),
   *   - filtra subs por parent_imdb+S+E,
   *   - persiste resume con clave file_id compuesta y episode meta,
   *   - habilita el botón "siguiente episodio". */
  season?: number | null
  episode?: number | null
  isSeries?: boolean
  /** Índice de fichero pre-resuelto por el provider (Torrentio.fileIdx).
   * Cuando está presente, backend salta el `select_file` heurístico y
   * sirve directamente `files[fileHint]` — crítico para packs con
   * numeración de anime u otras rarezas donde parsear el nombre falla. */
  fileHint?: number | null
}

const CONTROLS_HIDE_MS = 2500

export function Player() {
  const nav = useNavigate()
  const t = useT()
  const location = useLocation()
  const state = (location.state ?? null) as PlayerState | null

  const videoRef = useRef<HTMLVideoElement | null>(null)
  const containerRef = useRef<HTMLDivElement | null>(null)

  const [stream, setStream] = useState<StreamInfo | null>(null)
  const [media, setMedia] = useState<MediaInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  /** Ruta a la que apunta el botón "Volver" cuando estamos en
   * pantalla de error. Los errores por swarm muerto
   * (`probe_stalled` del backend, `swarm_stalled` de los segmentos
   * HLS) fijan aquí la ruta de la lista de torrents del título para
   * que el user pueda elegir OTRO release sin tener que navegar a
   * mano. Para errores "reales" (ffmpeg missing, HLS unsupported,
   * MediaError code 4) se deja `null` → cae al `nav(-1)` clásico. */
  const [errorBackTo, setErrorBackTo] = useState<string | null>(null)
  /** Ruta canónica de la lista de torrents del título actual —
   * derivada de `state.tmdbId` + episodio/temporada cuando aplica.
   * `null` si no tenemos tmdbId (flujo directo por magnet, TUI):
   * en ese caso `handleBack` cae al `nav(-1)` estándar, que en la
   * práctica también lleva a Torrents porque es la página anterior
   * en la historia de navegación normal. */
  const torrentsRoute = useMemo<string | null>(() => {
    const s = state
    if (!s?.tmdbId) return null
    if (s.isSeries && s.season != null && s.episode != null) {
      return `/torrents/series/${s.tmdbId}?season=${s.season}&episode=${s.episode}`
    }
    return `/torrents/tmdb/${s.tmdbId}?title=${encodeURIComponent(s.title)}`
  }, [state])
  /** `true` cuando el `<video>` con `src=/video` (path DIRECT) dio
   *  `MEDIA_ERR_SRC_NOT_SUPPORTED`. En ese caso reescribimos
   *  `canGoDirect` a `false` para el siguiente render → el effect
   *  del src apunta a HLS transmux y ffmpeg entrega H.264/AAC
   *  compatible. WKWebView / WebView2 a veces dicen "puedo con
   *  esto" en `direct_playable=true` pero al montar el fichero
   *  fallan (perfil raro, moov al final con problemas, audio no
   *  soportado, etc.). Este fallback los cubre a todos. */
  const [directFailed, setDirectFailed] = useState(false)

  const [paused, setPaused] = useState(true)
  const [currentTime, setCurrentTime] = useState(state?.startSeconds ?? 0)
  const [volume, setVolume] = useState(1)
  const [muted, setMuted] = useState(false)
  const [buffering, setBuffering] = useState(true)
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [controlsVisible, setControlsVisible] = useState(true)

  // Estadísticas de descarga del torrent — polled cada 1s mientras
  // haya `stream`. Se pintan en el overlay de arranque ("cargando lo
  // mínimo para poder reproducir…") con velocidad, ETA y % progreso.
  // Ceden protagonismo a las controls normales una vez que la peli
  // arranca (los primeros `onPlaying`), pero seguimos polleando por
  // si vuelve el buffering a mitad y queremos mostrar el mismo HUD.
  const [stats, setStats] = useState<StreamStats | null>(null)
  // Se pone a `true` la primera vez que el `<video>` dispara `onPlaying`.
  // Sirve para distinguir el arranque inicial (overlay rico con ETA /
  // speed / "cargando lo mínimo…") de un re-buffer a mitad de la peli
  // (spinner escueto: el user ya vio la primera imagen).
  const [hasStartedPlayback, setHasStartedPlayback] = useState(false)

  // `seeking` = true entre `v.currentTime = t` y el momento en que
  // WKWebView vuelve a emitir `playing` en el nuevo offset. Se usa
  // para pintar el mismo StremioLoader (backdrop + spinner) que en
  // el arranque, en vez de un mini-spinner en la esquina — el user
  // reportaba que al hacer seek "la barra se mueve pero el vídeo /
  // audio no cortan": el flag pausa explícitamente el `<video>` y
  // muestra un overlay opaco que tapa el frame antiguo hasta que el
  // buffer se rellene en el offset nuevo.
  const [seeking, setSeeking] = useState(false)

  // Escalado de loader: seek / re-buffer de MENOS de 2s se ven con
  // un spinner ligero (menos ruidoso, respeta la sensación de
  // "esto va rápido"); a partir de 2s montamos el StremioLoader
  // completo con backdrop + logo + stats — la carga se ha vuelto
  // "hay que explicar por qué el user está esperando".
  //
  // Timer arranca cuando aparece `seeking` o `buffering` y se
  // cancela en cuanto ambos vuelven a false. Independiente del
  // arranque inicial (donde el StremioLoader se pinta de golpe
  // sin delay: allí el user espera esa pantalla, es información).
  const [stalledLong, setStalledLong] = useState(false)
  const stalledTimerRef = useRef<number | null>(null)
  useEffect(() => {
    const stalling = seeking || buffering
    if (!stalling) {
      if (stalledTimerRef.current) {
        window.clearTimeout(stalledTimerRef.current)
        stalledTimerRef.current = null
      }
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono al salir de estado stalling.
      setStalledLong(false)
      return
    }
    // Ya está en curso un timer o ya estamos en modo long — no
    // reinicies (el user quiere ver el StremioLoader lo antes
    // posible en cuanto pasamos el umbral, no que se resetee el
    // contador con cada `waiting` intermedio).
    if (stalledTimerRef.current || stalledLong) return
    stalledTimerRef.current = window.setTimeout(() => {
      setStalledLong(true)
      stalledTimerRef.current = null
    }, 2000)
  }, [seeking, buffering, stalledLong])

  // Backdrop de TMDB (URL absoluta al CDN). Se pinta como fondo del
  // StremioLoader durante arranque y seek. `null` mientras no
  // tengamos tmdbId o la petición esté en vuelo — el loader cae a
  // fondo negro plano.
  const [backdropUrl, setBackdropUrl] = useState<string | null>(null)
  // Poster + backdrop paths "raw" (relativos de TMDB, sin CDN prefix)
  // que viajan con cada `reportPosition` al store movie-level. Los
  // declaramos AQUÍ (antes de `useResumePosition`) porque el hook los
  // consume como props — moverlos abajo daría Temporal Dead Zone.
  const [posterPathRaw, setPosterPathRaw] = useState<string | null>(null)
  const [backdropPathRaw, setBackdropPathRaw] = useState<string | null>(null)
  const [yearFromView, setYearFromView] = useState<number | null>(null)

  // Logo art (PNG con transparencia del rótulo oficial de la peli).
  // Servido por Metahub — el mismo mirror que usa Stremio para su
  // "loading screen con el título estilizado". URL construida por
  // imdb_id; 404 si no hay logo → el StremioLoader detecta el
  // `onError` y cae al `<h1>` de texto plano. `null` cuando aún no
  // tenemos imdb_id o el user vino por búsqueda directa sin TMDB.
  const [logoUrl, setLogoUrl] = useState<string | null>(null)

  // Refs "volátiles" para valores que cambian a alta frecuencia
  // (`currentTime`, cada `timeupdate` ≈ 250ms) o cuyo estado
  // necesita ser leído por handlers que NO deben re-suscribirse en
  // cada cambio (hotkeys, timer de report_position, cleanup). Sin
  // esto, el efecto keydown capturaría valores obsoletos: `j/l/←/→`
  // saltaban desde el `currentTime` del render en que se enganchó
  // (no el actual), y `Esc` podía ejecutar `handleBack()` en lugar
  // de salir del fullscreen.
  const currentTimeRef = useRef(currentTime)
  const isFullscreenRef = useRef(isFullscreen)
  // `durationRef` + `streamIdRef` se leen desde el timer de report y
  // desde el cleanup del useEffect de mount — ambos no reactivos.
  const durationRef = useRef<number | null>(null)
  const streamIdRef = useRef<number | null>(null)
  // Ref al `activeSub` último para que el reporter periódico lo
  // persista sin re-crear el callback cada vez que el user cambia
  // de pista. Se declara aquí (antes de `useResumePosition`) para
  // esquivar la Temporal Dead Zone; el `useState<ActiveSub>` real
  // vive más abajo, y un effect debajo sincroniza este ref.
  //
  // El tipo `ActiveSub` no está declarado todavía en este punto
  // (vive dentro del cuerpo de la función, más abajo). Usamos aquí
  // el shape estructural (union anónima) — TS lo unifica con el
  // `type ActiveSub` local por compatibilidad estructural, y el
  // `LastSubDto` del api.ts tiene EXACTAMENTE el mismo shape, así
  // que el ref se pasa como `activeSubRef` al hook sin conversión.
  const activeSubRef = useRef<
    | { source: 'openSubs'; path: string; release: string; language: string }
    | { source: 'embedded'; idx: number; release: string; language: string }
    | null
  >(null)
  useEffect(() => {
    currentTimeRef.current = currentTime
  }, [currentTime])
  useEffect(() => {
    isFullscreenRef.current = isFullscreen
  }, [isFullscreen])

  const duration = media?.duration_seconds ?? null

  // Sincroniza refs volátiles para el reporter de posición.
  useEffect(() => {
    durationRef.current = duration
  }, [duration])
  useEffect(() => {
    streamIdRef.current = stream?.id ?? null
  }, [stream])

  const { reportPositionNow } = useResumePosition({
    stream,
    duration,
    streamIdRef,
    currentTimeRef,
    durationRef,
    isSeries: state?.isSeries,
    season: state?.season,
    episode: state?.episode,
    tmdbId: state?.tmdbId,
    title: state?.title,
    imdbId: state?.imdbId,
    posterPath: posterPathRaw,
    backdropPath: backdropPathRaw,
    year: yearFromView,
    magnet: state?.magnet,
    activeSubRef,
  })

  // Poll de `stream_stats` cada 1s mientras el stream esté vivo.
  // Alimenta el overlay de arranque (speed, ETA, %). No hace falta
  // parar cuando ya está reproduciendo — el coste es despreciable
  // (una IPC por segundo) y así los mismos datos están disponibles
  // si volvemos a bufferear a mitad. Sí paramos si el backend
  // devuelve error o el componente se desmonta.
  useEffect(() => {
    if (!stream) return
    let cancelled = false
    const poll = async () => {
      try {
        const s = await streamStats(stream.id)
        if (!cancelled) setStats(s)
      } catch {
        // Silencioso: si el handle ya no está en el mapa (stop_stream
        // o crash del backend), simplemente dejamos de pintar HUD.
        if (!cancelled) setStats(null)
      }
    }
    void poll()
    const id = window.setInterval(() => {
      void poll()
    }, 1000)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [stream])

  // Arranca el stream al montar; para al desmontar.
  useEffect(() => {
    if (!state?.magnet) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Gate no-magnet: setState síncrona única antes de retornar.
      setError(t('player.noMagnet'))
      return
    }
    let cancelled = false
    let localStream: StreamInfo | null = null
    ;(async () => {
      try {
        // §4 audit series: si el user viene del flujo de serie
        // pasamos S/E → el backend selecciona el fichero del
        // episodio dentro del pack (`select_file`). Si el provider
        // ya nos dio el índice (Torrentio.fileIdx), lo pasamos como
        // `fileHint` — bypasa `select_file` y es más preciso.
        const info = await startStreamHtml(
          state.magnet,
          state.isSeries ? (state.season ?? null) : null,
          state.isSeries ? (state.episode ?? null) : null,
          state.fileHint ?? null,
        )
        if (cancelled) {
          await stopStream(info.id).catch(() => {})
          return
        }
        localStream = info
        setStream(info)
      } catch (e) {
        setError(t('player.startError', { err: String(e) }))
      }
    })()
    return () => {
      cancelled = true
      if (localStream) {
        // Flush final de la posición ANTES del stopStream: el
        // backend resuelve `report_position` mirando el stream_id
        // en el mapa vivo; tras `stop_stream` el slot se libera y
        // no habría dónde escribir el infohash. Fire-and-forget:
        // el cleanup no puede ser async, pero encolar el await es
        // suficiente porque Tauri serializa las invocaciones IPC.
        const id = localStream.id
        void (async () => {
          await reportPositionNow()
          await stopStream(id).catch(() => {})
        })()
      }
    }
    // Solo arrancamos una vez al montar. El magnet nunca cambia
    // durante la vida del componente (navegación crea un mount nuevo).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // ffprobe: sacamos duración + streams tras tener el stream.
  useEffect(() => {
    if (!stream) return
    let cancelled = false
    ;(async () => {
      try {
        const info = await probeStream(stream.url)
        if (!cancelled) setMedia(info)
      } catch (e) {
        // Probe puede fallar por: swarm sin seeders (backend firma
        // `probe_stalled` con 504+JSON → ProbeStalledError),
        // ffmpeg/ffprobe no instalado, timeout, CSP bloqueando
        // 127.0.0.1. Sin `media` el <video> nunca se monta
        // (videoSrc = null) → onError nunca dispara → spinner
        // infinito. Hay que decidirle al user.
        if (cancelled) return
        if (e instanceof ProbeStalledError) {
          // Firma clara de "este torrent no tiene seeders vivos":
          // mensaje específico + botón Volver → lista de torrents
          // del título. Antes esto se ocultaba bajo el mensaje
          // genérico "comprueba ffmpeg", que era engañoso — el
          // binario está OK, el problema es el swarm.
          setError(t('player.probeStalled', { elapsed: String(e.elapsedS) }))
          setErrorBackTo(torrentsRoute)
          return
        }
        const msg = String(e)
        const looksLikeMissingFfmpeg = /ffmpeg/i.test(msg)
        setError(
          looksLikeMissingFfmpeg
            ? t('player.ffmpegMissing', { hint: ffmpegInstallHint(t) })
            : t('player.probeFailed', { err: msg }),
        )
      }
    })()
    return () => {
      cancelled = true
    }
    // `stream` es el disparador real; `t` y `torrentsRoute` los
    // usamos en el catch pero son estables durante la vida del
    // componente (t re-genera cadena vacía tras cambio de locale,
    // y torrentsRoute depende de `state` que nunca cambia mid-vida
    // de la view — un mount nuevo se crea al navegar). Añadirlos
    // al array re-dispararía el probe sin motivo.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stream])

  // Backdrop de TMDB para el StremioLoader. Se pide UNA vez al
  // montar (el tmdbId no cambia durante la vida del componente).
  // Fallo silencioso — el loader funciona sin backdrop.
  //
  // Además guardamos `posterPath`/`backdropPath` "raw" (path relativo
  // de TMDB, sin CDN) para el snapshot de metadata que viaja con
  // cada `reportPosition` al store movie-level. La sección "Seguir
  // viendo" en Home pinta la card con esos paths tal cual sin
  // necesitar re-consultar TMDB.
  useEffect(() => {
    const id = state?.tmdbId
    if (!id) return
    let cancelled = false
    ;(async () => {
      try {
        const view = await getMovieView(id)
        if (cancelled) return
        // Prioridad al backdrop (16:9, encaja con el player); si no
        // hay, caemos al poster (2:3) — se ve descentrado pero es
        // mejor que negro plano.
        const url =
          tmdbBackdrop(view?.backdrop_path ?? null, 'w1280') ??
          tmdbBackdrop(view?.poster_path ?? null, 'w780')
        setBackdropUrl(url)
        setPosterPathRaw(view?.poster_path ?? null)
        setBackdropPathRaw(view?.backdrop_path ?? null)
        // TMDB devuelve fecha como "YYYY-MM-DD" — nos quedamos con
        // el año numérico si parsea.
        const y = view?.release_date?.slice(0, 4)
        setYearFromView(y ? Number(y) || null : null)
      } catch {
        /* silencioso: sin backdrop el loader cae a fondo negro */
      }
    })()
    return () => {
      cancelled = true
    }
    // Solo depende del tmdbId — evitamos re-pedir cuando cambia el
    // resto del state (que puede pasar en cada re-render).
  }, [state?.tmdbId])

  // Logo art via Metahub (el mismo CDN que usa Stremio). URL directa
  // por imdb_id, sin API key: si el título tiene "HD Movie Logo" en
  // Fanart.tv, Metahub lo sirve; si no, 404 y el `<img onError>` del
  // loader cae al `<h1>` de texto. Series usan el imdb_id del show
  // (parent), que también funciona. Normalizamos el prefijo `tt`
  // por si algún caller nos manda el id pelado.
  useEffect(() => {
    const raw = state?.imdbId
    if (!raw) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono cuando no hay imdb.
      setLogoUrl(null)
      return
    }
    const id = raw.startsWith('tt') ? raw : `tt${raw}`
    setLogoUrl(`https://images.metahub.space/logo/medium/${id}/img`)
  }, [state?.imdbId])

  // Subtítulos: modelo Stremio-like. Al arrancar el player pedimos
  // TODOS los subs de OpenSubtitles para este imdb_id (sin filtro de
  // idioma — el user reportaba pelis sin subs en videodrome pero con
  // decenas en Stremio; el filtro de `es,en,fr,de,it` los ocultaba).
  // Los agrupamos en el panel `SubsPanel` por idioma con tabs. El
  // user pinza uno → download → VTT blob → track activo.
  //
  // `initialSubPath` cubre el flujo legacy: si algún caller (viejo
  // Torrents antes del refactor) todavía pasa un subPath, se activa
  // por defecto. En el flujo nuevo `state.subPath` es null y el
  // panel se abre con "Sin subs" hasta que el user elija.
  const [subsList, setSubsList] = useState<Subtitle[] | null>(null)
  const [subsLoading, setSubsLoading] = useState(false)
  const [subsPanelOpen, setSubsPanelOpen] = useState(false)
  const [subDownloading, setSubDownloading] = useState<number | null>(null)
  // Panel de stats del stream (velocidad, peers, progreso, ETA). Se
  // toggle desde el botón `Gauge` en la barra de controles, al lado
  // del botón de subs. Info-only, no bloquea la reproducción.
  const [statsPanelOpen, setStatsPanelOpen] = useState(false)

  // Panel de pistas de audio (estilo Stremio). El botón aparece
  // solo si el probe detecta MÁS DE UNA pista de audio en el
  // contenedor. `activeAudioIdx` es el índice dentro del sub-array
  // filtrado por `kind === 'audio'` (0-based); coincide con el
  // `-map 0:a:<idx>` que el backend usa.
  const [audioPanelOpen, setAudioPanelOpen] = useState(false)
  const [activeAudioIdx, setActiveAudioIdx] = useState(0)
  // `true` durante el cambio de pista: backend está purgando
  // segmentos y respawneando ffmpeg. La UI pinta el StremioLoader
  // mientras dura para que el user vea que "está cambiando", en vez
  // de un playback frozen sin explicación.
  const [audioSwitching, setAudioSwitching] = useState(false)
  // Sub activo: unión discriminada entre "descargado de
  // OpenSubtitles" (fichero local que el backend convierte a VTT)
  // y "extraído del contenedor" (ffmpeg extrae la pista `idx` del
  // torrent como VTT en un endpoint one-shot). Los dos casos se
  // colapsan en el mismo `rawVtt` → mismo blob → mismo `<track>` en
  // el `<video>`, así el resto del pipeline no cambia.
  type ActiveSub =
    | { source: 'openSubs'; path: string; release: string; language: string }
    | { source: 'embedded'; idx: number; release: string; language: string }
  const [activeSub, setActiveSub] = useState<ActiveSub | null>(
    state?.subPath
      ? {
          source: 'openSubs',
          path: state.subPath,
          release: state.subRelease ?? 'Subs',
          language: 'es',
        }
      : null,
  )
  // Mantiene sincronizado el ref (declarado arriba, antes del
  // `useResumePosition`) con el `activeSub` actual. El reporter
  // periódico lo lee para persistir la pista de subs en el store
  // movie-level sin re-crear el callback en cada cambio.
  useEffect(() => {
    activeSubRef.current = activeSub
  }, [activeSub])

  // Hidratación de la pista de subs al montar: si el store
  // movie-level (`movie_progress.json`) guarda un `last_sub` para
  // esta peli/episodio, lo activamos automáticamente. Respeta la
  // elección explícita del user cuando viene por el flujo viejo
  // (Torrents pasa `subPath` en `state`) → no lo pisamos.
  //
  // La consulta usa el mismo `getResume` que ya se llamaba en
  // `Torrents.tsx` para pintar el ResumeDialog; nos apoyamos en el
  // cache del backend. Falla silenciosa — sin subs es la UX
  // original.
  useEffect(() => {
    if (state?.subPath) return
    if (!state?.magnet) return
    let cancelled = false
    ;(async () => {
      try {
        const r = await getResume(
          state.magnet,
          state.isSeries ? (state.season ?? null) : null,
          state.isSeries ? (state.episode ?? null) : null,
          state.tmdbId ?? null,
        )
        if (cancelled || !r?.last_sub) return
        setActiveSub(r.last_sub)
      } catch {
        /* silencioso: sin sub previo es el estado normal */
      }
    })()
    return () => {
      cancelled = true
    }
    // Solo al mount — dependemos de valores que no cambian mid-vida.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Auto-fetch del catálogo de subs en cuanto tenemos stream (para
  // que al abrir el panel ya estén listos). No bloquea la
  // reproducción — corre en paralelo con el probe.
  useEffect(() => {
    if (!stream) return
    let cancelled = false
    // eslint-disable-next-line react-hooks/set-state-in-effect -- Loading flag síncrono; el resto se resuelve async.
    setSubsLoading(true)
    // Idiomas: `UI locale + prefs.subtitle_languages` con dedupe.
    // Así OpenSubtitles ordena los subs del idioma de la app arriba
    // (el user con la app en ES ve los ES primero aunque sus prefs
    // históricas fueran "en,es"). Si getPreferences falla — no-Tauri,
    // backend caído — sólo enviamos el UI locale.
    ;(async () => {
      let langs: string = getLocale()
      try {
        const prefs = await getPreferences()
        langs = mergeSubtitleLangs(getLocale(), prefs.subtitle_languages)
      } catch {
        /* best-effort */
      }
      if (cancelled) return
      // Pasamos también `stream.id`: el backend usa el StreamHandle
      // asociado para calcular el hash OpenSubtitles del fichero de
      // vídeo y buscar subs SYNC-VERIFIED (match exacto de release).
      // Si el hash no da resultados, cae a imdb_id/query como antes.
      try {
        const subs = await searchSubtitles(
          stream.id,
          state?.imdbId ?? null,
          state?.title ?? null,
          langs,
          // §5 audit series: parent_imdb_id + season + episode habilita
          // la ruta canónica de OpenSubtitles para subs de episodio.
          state?.isSeries ? (state.season ?? null) : null,
          state?.isSeries ? (state.episode ?? null) : null,
        )
        if (!cancelled) setSubsList(subs)
      } catch (e) {
        console.warn('searchSubtitles failed:', e)
        if (!cancelled) setSubsList([])
      } finally {
        if (!cancelled) setSubsLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [stream, state?.imdbId, state?.title, state?.isSeries, state?.season, state?.episode])

  // VTT raw (texto original tal cual sale del backend).
  const [rawVtt, setRawVtt] = useState<string | null>(null)
  useEffect(() => {
    if (!activeSub) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono cuando no hay sub activo.
      setRawVtt(null)
      return
    }
    let cancelled = false
    ;(async () => {
      try {
        const vtt =
          activeSub.source === 'openSubs'
            ? await subtitleToVtt(activeSub.path)
            : await fetchEmbeddedSubtitle(stream?.url ?? '', activeSub.idx)
        if (!cancelled) setRawVtt(vtt)
      } catch (e) {
        console.warn('vtt fetch failed:', e)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [activeSub, stream])

  // Blob URL del VTT — ESTABLE mientras no cambie el sub. El shift
  // de timestamps por `subOffset` / `subSpeed` se hace in-place
  // sobre `textTracks[0].cues` en un useEffect aparte, sin tocar el
  // blob (cambiar `<track src>` a mitad de carga del `<video>` hace
  // que WKWebView aborte la reproducción).
  const [vttUrl, setVttUrl] = useState<string | null>(null)
  useEffect(() => {
    if (!rawVtt) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono cuando no hay vtt.
      setVttUrl(null)
      return
    }
    const blob = new Blob([rawVtt], { type: 'text/vtt' })
    const url = URL.createObjectURL(blob)
    setVttUrl(url)
    return () => URL.revokeObjectURL(url)
  }, [rawVtt])

  // Guarda los timestamps ORIGINALES de cada cue la primera vez que
  // se parsean. Los necesitamos porque cada re-shift parte del
  // original, no del shifted anterior (si no, aplicaríamos offsets
  // acumulados). Map de índice → [startOriginal, endOriginal].
  const cueOriginalTimesRef = useRef<Map<number, [number, number]>>(new Map())
  useEffect(() => {
    // Reset al cambiar el sub: los índices apuntan a cues nuevas.
    cueOriginalTimesRef.current = new Map()
  }, [rawVtt])

  // Ajuste manual de sync del sub. `subOffset` desplaza en segundos
  // (positivo = sub sale más tarde). `subSpeed` multiplica los
  // timestamps (útil para reconciliar frame rate: 25/23.976 ≈ 1.04271
  // corrige un sub 23.976fps sobre un release PAL 25fps).
  // Fórmula final: `cue_final = cue_original * subSpeed + subOffset`
  // — ya no hay shift por `srcStart` porque en el modelo VOD el
  // timeline del `<video>` es tiempo absoluto de la peli.
  const [subOffset, setSubOffset] = useState(0)
  const [subSpeed, setSubSpeed] = useState(1)
  // HUD que aparece 2s cuando el user ajusta sync. Muestra los
  // valores actuales para feedback inmediato.
  const [syncHud, setSyncHud] = useState<string | null>(null)
  const syncHudTimerRef = useRef<number | null>(null)
  const showSyncHud = useCallback((text: string) => {
    setSyncHud(text)
    if (syncHudTimerRef.current) window.clearTimeout(syncHudTimerRef.current)
    syncHudTimerRef.current = window.setTimeout(() => setSyncHud(null), 1500)
  }, [])
  // Reset del sync al cambiar de sub (los timestamps del sub nuevo
  // son de otra fuente, mantener el offset del anterior no tiene
  // sentido).
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset síncrono de offset/speed al cambiar de sub.
    setSubOffset(0)
    setSubSpeed(1)
  }, [activeSub])

  // Shift de cues in-place según subOffset + subSpeed. Se dispara
  // cuando cambian: `vttUrl` (nuevo sub), `subOffset` / `subSpeed`
  // (ajuste manual). Guardamos los timestamps ORIGINALES en
  // `cueOriginalTimesRef` para poder re-aplicar transformaciones
  // desde cero (no acumular offsets).
  useEffect(() => {
    const v = videoRef.current
    if (!v || !vttUrl) return
    const applyShift = () => {
      const tracks = v.textTracks
      if (tracks.length === 0) return
      const cues = tracks[0].cues
      if (!cues) return
      for (let i = 0; i < cues.length; i++) {
        const cue = cues[i]
        // Guarda original la primera vez.
        let original = cueOriginalTimesRef.current.get(i)
        if (!original) {
          original = [cue.startTime, cue.endTime]
          cueOriginalTimesRef.current.set(i, original)
        }
        const [origStart, origEnd] = original
        cue.startTime = Math.max(0, origStart * subSpeed + subOffset)
        cue.endTime = Math.max(0, origEnd * subSpeed + subOffset)
      }
    }
    // Aplica ya (si las cues ya están parseadas) y también cuando
    // el <track> completa la carga (`load` event dispara cuando
    // WKWebView acaba de parsear el VTT).
    applyShift()
    const trackEl = v.querySelector('track')
    trackEl?.addEventListener('load', applyShift)
    return () => {
      trackEl?.removeEventListener('load', applyShift)
    }
  }, [vttUrl, subOffset, subSpeed])


  // WKWebView / Safari NO respetan el atributo `default` del <track>
  // cuando el track se añade DESPUÉS de que el <video> ya cargó
  // (que es siempre nuestro caso: el user abre el panel de subs y
  // elige uno con el video ya reproduciendo). Hay que forzar el
  // modo `showing` en el TextTrack manualmente. Además, tras cada
  // `<video>` load (post-seek en modo transmux) el browser resetea
  // los textTracks a `disabled` — reaplicamos con listeners a
  // `loadedmetadata` / `loadeddata`.
  useEffect(() => {
    const v = videoRef.current
    if (!v || !vttUrl) return
    const applyMode = () => {
      const tracks = v.textTracks
      for (let i = 0; i < tracks.length; i++) {
        tracks[i].mode = i === 0 ? 'showing' : 'disabled'
      }
    }
    const raf = requestAnimationFrame(applyMode)
    v.addEventListener('loadedmetadata', applyMode)
    v.addEventListener('loadeddata', applyMode)
    return () => {
      cancelAnimationFrame(raf)
      v.removeEventListener('loadedmetadata', applyMode)
      v.removeEventListener('loadeddata', applyMode)
    }
  }, [vttUrl])

  const pickSub = async (sub: Subtitle) => {
    if (subDownloading !== null) return
    setSubDownloading(sub.file_id)
    try {
      const path = await downloadSubtitle(sub)
      setActiveSub({
        source: 'openSubs',
        path,
        release: sub.release || sub.file_name || 'Sub',
        language: sub.language,
      })
      setSubsPanelOpen(false)
    } catch (e) {
      console.warn('downloadSubtitle failed:', e)
    } finally {
      setSubDownloading(null)
    }
  }

  /** Selección de una pista de subs embebida en el contenedor. Se
   * dispara desde el SubsPanel al pulsar en la sección "Del fichero".
   * `idx` es el índice dentro del sub-array `kind === 'subtitle'` de
   * `MediaInfo.streams`, que coincide con el `-map 0:s:<idx>` que
   * usa el endpoint `/subs/embedded/<idx>.vtt` en el backend. */
  const pickEmbeddedSub = (streamInfo: MediaStream, subIdx: number) => {
    setActiveSub({
      source: 'embedded',
      idx: subIdx,
      release: streamInfo.title || `Track #${subIdx + 1}`,
      language: streamInfo.language || 'und',
    })
    setSubsPanelOpen(false)
  }

  const clearSub = () => {
    setActiveSub(null)
  }

  // ---- Drag & drop de subtítulos locales ----
  //
  // El user arrastra un `.srt` / `.vtt` del file manager al player
  // → activa esa pista de subs sin pasar por el panel de
  // OpenSubtitles. Útil para releases muy raros o para subs de
  // fansubs que OpenSubtitles no indexa.
  //
  // Tauri 2 gate: los eventos "drag/drop de fichero externo" NO
  // llegan como HTML5 dnd (el WebView los intercepta). Hay que
  // suscribirse a `getCurrentWebview().onDragDropEvent`, que
  // devuelve los paths absolutos del SO. Los HTML5 handlers
  // (`onDragOver`, `onDrop`) solo se disparan para drag INTERNO
  // (elementos con `draggable=true` dentro de la propia app) y no
  // dan acceso al filesystem — inútiles aquí.
  //
  // Estados:
  //   * `dragActive` — `true` mientras hay un fichero flotando
  //     sobre la ventana; pinta el overlay grande "Suelta para
  //     añadir subtítulos".
  //   * `dragFlash` — `true` durante ~800ms tras un drop exitoso;
  //     pinta un pulso verde alrededor del `<video>` para
  //     confirmar visualmente que se cargó.
  //   * `dragError` — mensaje de error transitorio si el fichero
  //     no es un sub válido.
  const [dragActive, setDragActive] = useState(false)
  const [dragFlash, setDragFlash] = useState(false)
  const [dragError, setDragError] = useState<string | null>(null)
  const dragErrorTimerRef = useRef<number | null>(null)
  useEffect(() => {
    let unlisten: (() => void) | null = null
    let cancelled = false
    ;(async () => {
      try {
        const webview = getCurrentWebview()
        const off = await webview.onDragDropEvent((event) => {
          const payload = event.payload
          if (payload.type === 'enter' || payload.type === 'over') {
            setDragActive(true)
          } else if (payload.type === 'leave') {
            setDragActive(false)
          } else if (payload.type === 'drop') {
            setDragActive(false)
            // Aceptamos el PRIMER path que tenga extensión de sub.
            // Si el user sueltó varios ficheros, ignoramos el resto:
            // el player solo muestra UNA pista simultánea, y elegir
            // en silencio "el que match" es menos frustrante que
            // pintar otro selector.
            const paths = (payload.paths ?? []) as string[]
            const sub = paths.find((p) =>
              /\.(srt|vtt|ass|ssa)$/i.test(p),
            )
            if (!sub) {
              setDragError(t('player.subDropInvalid'))
              if (dragErrorTimerRef.current)
                window.clearTimeout(dragErrorTimerRef.current)
              dragErrorTimerRef.current = window.setTimeout(
                () => setDragError(null),
                2500,
              )
              return
            }
            // Deriva release + idioma "adivinado" del nombre de
            // fichero para pintar labels útiles. Heurística barata:
            // basename sin extensión = release; token de 2-3 chars
            // rodeado de puntos/guiones = idioma. Fallback a la
            // pista del user actual o "es".
            const base = sub.split(/[\\/]/).pop() ?? sub
            const release = base.replace(/\.[^.]+$/, '')
            const langMatch = base.match(/[._-]([a-z]{2,3})[._-]/i)
            const language =
              (langMatch?.[1] ?? activeSub?.language ?? 'es').toLowerCase()
            setActiveSub({
              source: 'openSubs',
              path: sub,
              release,
              language,
            })
            setDragFlash(true)
            window.setTimeout(() => setDragFlash(false), 800)
          }
        })
        if (cancelled) off()
        else unlisten = off
      } catch (e) {
        console.warn('onDragDropEvent listener setup failed:', e)
      }
    })()
    return () => {
      cancelled = true
      unlisten?.()
      if (dragErrorTimerRef.current)
        window.clearTimeout(dragErrorTimerRef.current)
    }
    // Deps: t (i18n) puede cambiar con el locale — re-suscribir
    // para que el mensaje de error salga en el idioma actual.
    // `activeSub.language` se lee dentro del handler; usamos ref
    // implícita del closure sin re-suscribir en cada cambio.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t])

  // Ajustes de <video> según state React.
  useEffect(() => {
    const v = videoRef.current
    if (!v) return
    v.volume = volume
    v.muted = muted
  }, [volume, muted])

  // Autohide de controles: cualquier movimiento de ratón los muestra;
  // pasados CONTROLS_HIDE_MS sin actividad se ocultan (solo si el
  // vídeo está reproduciéndose — pausado se quedan siempre visibles).
  const hideTimerRef = useRef<number | null>(null)
  const bumpControls = useCallback(() => {
    setControlsVisible(true)
    if (hideTimerRef.current) window.clearTimeout(hideTimerRef.current)
    if (!paused) {
      hideTimerRef.current = window.setTimeout(() => {
        setControlsVisible(false)
      }, CONTROLS_HIDE_MS)
    }
  }, [paused])

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- bumpControls llama setControlsVisible síncrono al montar.
    bumpControls()
    return () => {
      if (hideTimerRef.current) window.clearTimeout(hideTimerRef.current)
    }
  }, [bumpControls])

  // Escucha cambios de fullscreen desde fuera (Esc, doble-click en
  // barra de título, o toggle de sistema). WKWebView no soporta el
  // `Fullscreen API` del DOM sobre <div>, así que trackeamos el
  // estado consultando la ventana Tauri.
  useEffect(() => {
    const w = getCurrentWindow()
    const check = () => {
      void w.isFullscreen().then(setIsFullscreen).catch(() => {})
    }
    check()
    // No hay evento nativo; polleamos al cambiar visibilidad de
    // controles (barato y suficiente para reflejar el icono).
    const id = window.setInterval(check, 1000)
    return () => window.clearInterval(id)
  }, [])

  const seekTo = useCallback((absoluteSeconds: number) => {
    // Modelo VOD (DIRECT o TRANSMUX): `v.currentTime` es tiempo
    // absoluto de la peli en ambos modos, y el backend produce
    // segmentos bajo demanda cuando el `<video>` los pide. Seek
    // 100% nativo — sin reasignar src, sin prefetch, sin cache-
    // busting.
    //
    // Bugfix (2026-07): en WKWebView, asignar `currentTime` mientras
    // el `<video>` está `playing` no siempre corta la salida de
    // audio/vídeo — el decoder sigue vaciando el buffer previo unos
    // cientos de ms, dando la sensación de que el seek no cortó el
    // stream (la barra salta pero el audio sigue). Pausamos
    // explícitamente antes del seek y marcamos `seeking = true`
    // para pintar el StremioLoader (backdrop + spinner) por encima
    // hasta que WKWebView vuelva a emitir `playing` en el offset
    // nuevo — momento en el que `onPlaying` limpia el flag y
    // resume la reproducción.
    const v = videoRef.current
    if (!v) return
    const target = Math.max(0, absoluteSeconds)
    setSeeking(true)
    setBuffering(true)
    // Actualiza el HUD del tiempo ya (no esperamos a `timeupdate`
    // — que en modo transmux puede tardar segundos si ffmpeg tiene
    // que respawnear en el offset nuevo).
    setCurrentTime(target)
    try {
      v.pause()
    } catch {
      /* algún browser tira sync si no hay src todavía */
    }
    v.currentTime = target
  }, [])

  const seekBy = (delta: number) => {
    // Leemos de la ref (no del state) para que las hotkeys que
    // llaman aquí no salten desde un `currentTime` obsoleto —
    // `timeupdate` se dispara a ~4Hz y no está en las deps del
    // efecto keydown a propósito (evitamos re-suscribir el listener
    // cada 250ms).
    seekTo(currentTimeRef.current + delta)
  }

  const togglePlay = () => {
    const v = videoRef.current
    if (!v) return
    if (v.paused) void v.play()
    else v.pause()
  }

  const toggleFullscreen = async () => {
    // Fullscreen a nivel de ventana Tauri (macOS: Split View / Space
    // dedicado; Windows/Linux: borderless fullscreen). WKWebView no
    // implementa `Element.requestFullscreen()` sobre `<div>`, así
    // que el path del DOM daría `undefined is not a function`.
    const w = getCurrentWindow()
    try {
      const current = await w.isFullscreen()
      await w.setFullscreen(!current)
      setIsFullscreen(!current)
    } catch (e) {
      console.warn('toggleFullscreen falló:', e)
    }
  }

  const handleBack = () => {
    // Al salir del player, restauramos la ventana a normal (evita
    // dejar la app en fullscreen al volver a la lista de torrents).
    void getCurrentWindow().setFullscreen(false).catch(() => {})
    // Flush final de posición ANTES de stopStream (el backend
    // necesita que el stream_id siga vivo en el mapa para resolver
    // el infohash). Fire-and-forget; nav(-1) puede ejecutarse en
    // paralelo sin problemas.
    if (stream) {
      const id = stream.id
      void (async () => {
        await reportPositionNow()
        await stopStream(id).catch(() => {})
      })()
    }
    // Errores por swarm muerto → volvemos EXPLÍCITAMENTE a la lista
    // de torrents del título (calculada desde `state.tmdbId` +
    // series/episodio) para que el user pueda elegir otro release,
    // en vez de un `nav(-1)` ciego que — si la historia tiene ruido
    // — puede dejar al user en Home o en Recomendaciones sin acceso
    // directo a los otros torrents que ya había mirado.
    // Para errores "reales" (ffmpeg roto, MediaError code 4) el
    // `errorBackTo` es null → mantenemos el `nav(-1)` clásico.
    if (errorBackTo) {
      nav(errorBackTo, { replace: true })
      return
    }
    nav(-1)
  }

  // Hotkeys globales — lógica extraída a useHotkeys (ver player/useHotkeys.ts).
  // Va DESPUÉS de declarar seekBy/toggleFullscreen/handleBack: al pasarlos
  // como argumentos al hook se leen en el render mismo, no en un efecto,
  // por lo que la Temporal Dead Zone de `const` los haría inaccesibles si
  // el hook se colocara antes. Deps del efecto interno: [stream, activeSub,
  // subSpeed] — idéntico al useEffect original.
  useHotkeys({
    videoRef,
    isFullscreenRef,
    seekBy,
    toggleFullscreen,
    activeSub,
    subSpeed,
    showSyncHud,
    setVolume,
    setMuted,
    setSubsPanelOpen,
    setSubOffset,
    setSubSpeed,
    setIsFullscreen,
    handleBack,
    stream,
  })

  const onTimeUpdate = () => {
    const v = videoRef.current
    if (!v) return
    setCurrentTime(v.currentTime)
  }

  // Ratón sobre la seek bar: hover-preview del tiempo.
  const [seekHover, setSeekHover] = useState<number | null>(null)

  // NOTA: la guarda `if (!state)` que renderiza el fallback "sin datos"
  // vive AL FINAL de la lista de hooks (justo antes del `return`
  // principal). react-hooks/rules-of-hooks obliga a que todos los
  // useRef/useCallback/useEffect posteriores se llamen en el mismo
  // orden en cada render — poner el early return aquí rompe esa
  // invariante para el ref/callback/efecto de switchAudioTrack + hls.js
  // que hay más abajo.

  // Ruta del `<video>`. Dos modos, ambos servidos con URLs estables
  // (sin query params) para que el WebView pueda cachear libremente:
  //
  //   * DIRECT: source ya es MP4/H.264/AAC compatible → apuntamos a
  //     `/video` raw. WKWebView/WebView2 lo reproducen nativamente
  //     con Range HTTP.
  //
  //   * TRANSMUX: cualquier otra cosa (MKV, HEVC 10-bit, VP9, opus…)
  //     → `/hls/playlist.m3u8` estático (playlist VOD que enumera
  //     todos los segmentos desde arranque). Los `.ts` los materializa
  //     ffmpeg bajo demanda en el backend. El seek es 100% nativo.
  //
  // El `src` se asigna UNA vez y no cambia — sin cache-busting, sin
  // reasignaciones tras seek. Antes teníamos `?start=<t>&_=<bust>`
  // que regresaba a `MEDIA_ERR_SRC_NOT_SUPPORTED` en WKWebView tras
  // seeks largos; el nuevo modelo lo elimina por construcción.
  //
  // Compatibilidad Windows/Linux (Fase B del audit Windows): WebView2
  // (Windows) y WebKitGTK (Linux) NO reproducen HLS nativo — solo
  // WKWebView (macOS) lo hace. Para la ruta TRANSMUX usamos hls.js
  // vía MSE cuando no hay soporte nativo (`canPlayType` vacío). El
  // attach del src ocurre en el useEffect de abajo — el JSX solo
  // pinta el `<video>` shell sin `src`, porque hls.js necesita
  // controlarlo con `attachMedia()`.
  //
  // Fase E del audit Windows: el backend marca `direct_playable = true`
  // para MP4/MOV con H.264 O HEVC + AAC. WKWebView (macOS) reproduce
  // HEVC nativo; WebView2 (Windows) SOLO si el user tiene instalada
  // la "HEVC Video Extensions" de la Microsoft Store. Sin ella el
  // `<video>` da MEDIA_ERR_SRC_NOT_SUPPORTED en un fichero que en
  // macOS reproduce perfecto. Aquí sobreescribimos la decisión del
  // backend: si el stream de vídeo es HEVC y `canPlayType('hvc1')`
  // devuelve vacío, forzamos ruta TRANSMUX (hls.js/ffmpeg lo baja a
  // H.264 en cliente/servidor).
  const canGoDirect = (() => {
    if (directFailed) return false
    if (!media?.direct_playable) return false
    const videoStream = media.streams.find((s) => s.kind === 'video')
    const codec = videoStream?.codec?.toLowerCase() ?? ''
    const isHevc = codec === 'hevc' || codec === 'h265' || codec === 'h.265'
    if (!isHevc) return true
    // Test real del navegador. `hvc1.1.6.L123.B0` = HEVC Main
    // Profile Level 4.1 — cubre la mayoría de releases 1080p; los
    // 4K L5.1 dan el mismo veredicto en la práctica (si el runtime
    // soporta L4.1 soporta L5.1). Usamos `.probably` como
    // aceptación estricta (no `.maybe`, que en WebView2 miente).
    if (typeof document === 'undefined') return true
    const probe = document.createElement('video')
    return (
      probe.canPlayType('video/mp4; codecs="hvc1.1.6.L123.B0"') === 'probably'
    )
  })()

  // Listas derivadas del probe. `audioTracks` incluye todas las
  // pistas de audio del contenedor (0..N-1). `embeddedSubs` filtra a
  // los subs de texto (SRT/ASS/SSA); los bitmap (PGS/DVBSUB) se
  // ocultan porque ffmpeg no puede convertirlos a VTT sin OCR.
  const audioTracks: MediaStream[] = media
    ? media.streams.filter((s) => s.kind === 'audio')
    : []
  const embeddedSubs: MediaStream[] = media
    ? media.streams.filter(
        (s) =>
          s.kind === 'subtitle' &&
          !isBitmapSubCodec(s.codec.toLowerCase()),
      )
    : []

  // Ref que guarda "seek pendiente tras un cambio de pista de audio".
  // El cambio dispara: (1) POST /hls/audio (backend purga + respawn),
  // (2) el efecto hls.js se re-run porque `activeAudioIdx` cambia,
  // (3) se monta un Hls nuevo con la misma URL de playlist (segmentos
  // limpios), (4) `onLoadedMetadata` consume esta ref para restaurar
  // `currentTime` y reanudar playback.
  const postAudioSwitchSeekRef = useRef<{ time: number; play: boolean } | null>(
    null,
  )

  const switchAudioTrack = useCallback(
    async (newIdx: number) => {
      if (!stream) return
      if (newIdx === activeAudioIdx) return
      const v = videoRef.current
      if (!v) return
      postAudioSwitchSeekRef.current = {
        time: v.currentTime,
        play: !v.paused,
      }
      setAudioSwitching(true)
      setBuffering(true)
      try {
        v.pause()
      } catch {
        /* pause sync error, ignore */
      }
      try {
        await setAudioTrack(stream.url, newIdx)
      } catch (e) {
        console.warn('setAudioTrack failed:', e)
        setAudioSwitching(false)
        postAudioSwitchSeekRef.current = null
        return
      }
      setActiveAudioIdx(newIdx)
      // El `useEffect` de hls.js observa `activeAudioIdx` en sus
      // deps (ver más abajo) y se re-run: destroy + new Hls con el
      // mismo URL; ffmpeg respawnea con `-map 0:a:<idx>` en la
      // primera petición de segmento del hls.js nuevo.
    },
    [stream, activeAudioIdx],
  )

  const videoSrc = stream && media
    ? canGoDirect
      ? stream.url
      : hlsUrl(stream.url)
    : null
  const needsHls = !!(stream && media && !canGoDirect)

  // Attach del src al `<video>`. Tres caminos:
  //   1. DIRECT (MP4 nativo) → `v.src = url`, WKWebView/WebView2/
  //      WebKitGTK lo reproducen con Range HTTP directamente.
  //   2. HLS nativo (Safari / WKWebView) → mismo `v.src = url`; el
  //      browser parsea el playlist y descarga los `.ts` solo.
  //   3. HLS via MSE (WebView2 / WebKitGTK) → hls.js hace el trabajo
  //      de transmux MPEG-TS → fMP4 en cliente y alimenta
  //      MediaSource. Timeouts subidos porque el backend materializa
  //      segmentos bajo demanda (ffmpeg puede tardar hasta 30s en
  //      arrancar en frío para el primer chunk).
  //
  // Cleanup: `hls.destroy()` es CRÍTICO al cambiar de stream o
  // desmontar — sin él quedan loaders vivos pidiendo segmentos de la
  // sesión anterior y confunden el backend.
  useEffect(() => {
    const v = videoRef.current
    if (!v || !videoSrc) return
    const nativeHls = v.canPlayType('application/vnd.apple.mpegurl') !== ''
    if (needsHls && !nativeHls) {
      if (!Hls.isSupported()) {
        // Ni HLS nativo ni MSE — plataforma sin soporte de vídeo
        // decente. Cae al mensaje de error genérico; el user puede
        // volver atrás y cambiar a VLC en Ajustes.
        // eslint-disable-next-line react-hooks/set-state-in-effect -- Gate sin soporte de vídeo: setState síncrona antes de retornar.
        setError(t('player.hlsUnsupported'))
        return
      }
      const hls = new Hls({
        // VOD con segmentos bajo demanda con progress-sensitive
        // deadline en el backend (audit §3.a): hard 120s, o 15s sin
        // progreso → 503 swarm_stalled. hls.js debe esperar al
        // backend (una sola fuente de verdad) → subimos los
        // timeouts por encima del hard deadline para que un abort
        // por timeout de hls.js NUNCA gane la carrera al backend.
        fragLoadingTimeOut: 130_000,
        manifestLoadingTimeOut: 20_000,
        // Reintentos: piezas frías de librqbit pueden dar 503/wait
        // legítimo. hls.js aborta por defecto en 3 intentos.
        fragLoadingMaxRetry: 6,
      })
      hls.loadSource(videoSrc)
      hls.attachMedia(v)
      hls.on(Hls.Events.ERROR, (_evt, data) => {
        if (!data.fatal) return
        // Backend firmó `swarm_stalled` (503 + JSON con datos
        // reales) → pintamos error honesto con velocidad y peers.
        // Cualquier otro fatal → mensaje genérico.
        console.warn('[hls] fatal', data.type, data.details)
        const resp = data.response
        if (resp && resp.code === 503) {
          try {
            const raw =
              typeof resp.data === 'string'
                ? resp.data
                : new TextDecoder().decode(resp.data as ArrayBuffer)
            const stall = JSON.parse(raw) as {
              reason?: string
              downloaded_pct?: number
              speed_bps?: number
              peers?: number
            }
            if (stall.reason === 'swarm_stalled') {
              setError(
                t('player.swarmStalled', {
                  speed: formatSpeed(stall.speed_bps ?? 0),
                  peers: String(stall.peers ?? 0),
                  pct: (stall.downloaded_pct ?? 0).toFixed(1),
                }),
              )
              // Simétrico a `probe_stalled`: si el swarm muere a
              // mitad de peli, el fix accionable es cambiar de
              // release; que Volver lleve a la lista, no atrás sin
              // más. Sin este override un enjambre que se queda sin
              // seeders acabaría mostrando el mensaje correcto pero
              // con un botón que devuelve a Home / Recs / donde
              // sea, y el user tenía que re-navegar a mano.
              setErrorBackTo(torrentsRoute)
              return
            }
          } catch {
            /* fallthrough al mensaje genérico */
          }
        }
        setError(t('player.hlsFatal', { type: data.type, details: data.details }))
      })
      return () => {
        try {
          hls.destroy()
        } catch {
          /* destroy es best-effort */
        }
      }
    }
    // Path nativo (DIRECT MP4 o HLS en Safari): asignar src.
    if (v.src !== videoSrc) {
      v.src = videoSrc
    }
    // `activeAudioIdx` incluido para forzar re-run al cambiar pista:
    // el backend ya purgó segmentos, hls.js necesita reconstruirse
    // para pedir la playlist de cero. `t` y `torrentsRoute` son
    // estables durante la vida del componente y solo se leen en el
    // catch fatal — meterlos en el array re-crearía hls.js sin motivo.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [videoSrc, needsHls, activeAudioIdx])

  // Guarda "sin datos" al final de la cadena de hooks (ver nota
  // arriba). state puede venir a null si el usuario llega a /player
  // por deep link sin location.state; no hay nada que reproducir.
  if (!state) {
    return (
      <div className="flex h-full items-center justify-center text-body">
        <div className="text-center">
          <p className="text-[15px]">{t('player.noData')}</p>
          <button
            onClick={() => nav(-1)}
            className="mt-4 rounded-sm border border-hairline px-4 py-2 text-[13px] hover:bg-surface"
          >
            {t('common.back')}
          </button>
        </div>
      </div>
    )
  }

  return (
    <div
      ref={containerRef}
      className={`relative h-screen w-full overflow-hidden bg-black ${
        controlsVisible ? '' : 'cursor-none'
      }`}
      onMouseMove={bumpControls}
      onClick={() => {
        // Click en el fondo del player:
        //   1. Si hay algún panel lateral abierto (subs/audio/stats),
        //      lo cerramos — patrón esperado: "click fuera = cerrar
        //      overlay". Los propios paneles paran la propagación
        //      con `stopPropagation`, así que este handler SOLO
        //      corre para clicks fuera de ellos.
        //   2. Si no hay ningún panel abierto, toggle play (comportamiento
        //      tipo Netflix/Stremio: click en el video = pausa/play).
        // Los controles (barra inferior, botones) interceptan sus
        // propios eventos con stopPropagation, así que un click en
        // el play/pause del control bar NO llega aquí.
        if (subsPanelOpen || audioPanelOpen || statsPanelOpen) {
          setSubsPanelOpen(false)
          setAudioPanelOpen(false)
          setStatsPanelOpen(false)
          return
        }
        togglePlay()
      }}
    >
      {videoSrc && (
        <video
          ref={videoRef}
          autoPlay
          crossOrigin="anonymous"
          className="absolute inset-0 h-full w-full object-contain bg-black"
          onPlay={() => setPaused(false)}
          onPause={() => setPaused(true)}
          onWaiting={() => setBuffering(true)}
          onCanPlay={() => {
            setBuffering(false)
            // Tras seek: WKWebView emite `canplay` cuando el
            // decoder ya tiene frame en el offset nuevo. Es aquí
            // donde arrancamos playback de vuelta — más fiable que
            // esperar a `seeked` (que a veces no dispara en HLS).
            if (seeking) {
              setSeeking(false)
              const v = videoRef.current
              if (v && v.paused) void v.play().catch(() => {})
            }
          }}
          onSeeking={() => {
            // Redundante con el `setSeeking(true)` de `seekTo`,
            // pero cubre el caso de seeks disparados por la propia
            // WebView (p.ej. el `currentTime = startSeconds` inicial
            // en `onLoadedMetadata`).
            setSeeking(true)
            setBuffering(true)
          }}
          onSeeked={() => {
            // Fallback si `canplay` no llega (raro con HLS pero
            // posible en modo DIRECT si el server tarda en abrir
            // el Range nuevo). Reanudamos aquí también.
            if (seeking) {
              setSeeking(false)
              const v = videoRef.current
              if (v && v.paused) void v.play().catch(() => {})
            }
          }}
          onPlaying={() => {
            setBuffering(false)
            setSeeking(false)
            setHasStartedPlayback(true)
          }}
          onLoadedMetadata={(e) => {
            const v = e.currentTarget
            // Prioridad: seek pendiente tras cambio de audio.
            const pending = postAudioSwitchSeekRef.current
            if (pending) {
              v.currentTime = pending.time
              postAudioSwitchSeekRef.current = null
              setAudioSwitching(false)
              if (pending.play) {
                void v.play().catch(() => {})
              }
              return
            }
            // Seek inicial de resume (`startSeconds > 0`). Aplica a
            // ambos modos porque el timeline del `<video>` es
            // tiempo absoluto en los dos: DIRECT usa Range sobre el
            // fichero completo; TRANSMUX usa un playlist VOD con
            // PTS absolutos vía `-output_ts_offset` en ffmpeg.
            if (state.startSeconds > 0) {
              v.currentTime = state.startSeconds
            }
          }}
          onTimeUpdate={onTimeUpdate}
          onError={() => {
            const v = videoRef.current
            const code = v?.error?.code ?? 0
            const msg = v?.error?.message ?? 'error desconocido'
            console.warn(`<video> error code ${code}: ${msg}`)
            // Fallback DIRECT → HLS: si estábamos en `<video src=/video>`
            // directo y WKWebView/WebView2 lo rechaza con
            // MEDIA_ERR_SRC_NOT_SUPPORTED (code 4), reintentamos por
            // HLS (ffmpeg transmux → H.264/AAC forzado, siempre
            // reproducible). El `direct_playable=true` del backend
            // es una PREDICCIÓN — a veces el WebView miente sobre
            // qué códecs soporta, o el fichero tiene un perfil raro
            // que el probe no detecta.
            //
            // Si el error viene YA de HLS (canGoDirect era false),
            // no hay más fallbacks posibles → mostrar mensaje al
            // user para que caiga a VLC.
            if (code === 4 && canGoDirect && !directFailed) {
              console.warn('<video> falló en DIRECT → reintentando por HLS')
              setDirectFailed(true)
              return
            }
            setError(t('player.videoFailed'))
          }}
        >
          {vttUrl && (
            <track
              default
              kind="subtitles"
              src={vttUrl}
              srcLang={activeSub?.language?.toLowerCase() ?? 'es'}
              label={activeSub?.release ?? 'Subs'}
            />
          )}
        </video>
      )}

      {/* Loader del player. Dos modos:
            * Ligero (spinner solo): re-buffers cortos y seeks de
              <2s. Suficiente para que el user note que "algo está
              cargando" sin tapar el frame con backdrop+logo+stats
              y sin dar sensación de "cuelgue".
            * Full StremioLoader (backdrop + logo + spinner +
              stats): arranque inicial, cambio de audio, y cuando
              el stall supera 2s (`stalledLong`) — a partir de ese
              umbral la espera se ha vuelto lo bastante larga como
              para justificar información completa.
          Los stats (speed / peers / %) SIEMPRE se pintan cuando NO
          es seek ni audio switch — es la única señal honesta de si
          el enjambre está vivo o muerto. Audit §3.b. */}
      {(() => {
        if (error) return null
        const showAny =
          !stream || !hasStartedPlayback || seeking || audioSwitching || buffering
        if (!showAny) return null
        // Full loader: arranque, audio switch, o stall largo.
        const showFull =
          !stream || !hasStartedPlayback || audioSwitching || stalledLong
        if (showFull) {
          return (
            <StremioLoader
              title={state.title}
              backdropUrl={backdropUrl}
              logoUrl={logoUrl}
              stats={!seeking && !audioSwitching ? stats : null}
            />
          )
        }
        // Modo ligero: solo spinner sobre el frame del video, sin
        // fondo opaco — el user sigue viendo la peli detrás. Se usa
        // en seek/rebuffer cortos (<2s).
        return (
          <div className="pointer-events-none absolute inset-0 z-30 flex items-center justify-center">
            <div className="rounded-full bg-black/50 p-3 backdrop-blur-sm">
              <div className="h-8 w-8 animate-spin rounded-full border-2 border-white/20 border-t-white" />
            </div>
          </div>
        )
      })()}

      {/* HUD de ajuste de sync del sub. Aparece 1.5s cuando el user
          usa `[` `]` `,` `.` para dar feedback inmediato. */}
      {syncHud && (
        <div className="pointer-events-none absolute left-1/2 top-16 -translate-x-1/2 rounded-md bg-black/80 px-4 py-2 text-[13px] text-ink">
          {syncHud}
        </div>
      )}

      {/* Error overlay */}
      {error && (
        <div className="absolute inset-0 flex items-center justify-center bg-black/80 px-6">
          <div className="max-w-md text-center">
            <p className="text-[15px] text-body">{error}</p>
            <button
              onClick={handleBack}
              className="mt-4 rounded-sm border border-hairline bg-surface px-4 py-2 text-[13px] hover:bg-surface-hi"
            >
              {t('common.back')}
            </button>
          </div>
        </div>
      )}

      {/* Overlay de drag&drop de subs. Se anima con fade+scale al
          entrar (via clases utilitarias animate-*) y sale suave.
          `pointer-events-none` explícito para que el webview siga
          recibiendo los eventos onDragDropEvent (si capturáramos
          eventos aquí Tauri no los vería). */}
      {dragActive && (
        <div className="pointer-events-none absolute inset-0 z-40 flex items-center justify-center bg-black/60 backdrop-blur-sm animate-drop-in">
          <div className="mx-6 flex max-w-md flex-col items-center gap-4 rounded-2xl border-2 border-dashed border-accent/70 bg-accent/10 px-10 py-8 shadow-[0_20px_60px_-20px_rgba(0,0,0,0.7)]">
            <div className="flex h-14 w-14 items-center justify-center rounded-full bg-accent/20 text-accent animate-bounce-slow">
              <DownloadSimple size={28} weight="bold" />
            </div>
            <p className="text-[16px] font-semibold text-ink text-center">
              {t('player.subDropTitle')}
            </p>
            <p className="text-[12px] text-muted text-center">
              {t('player.subDropHint')}
            </p>
          </div>
        </div>
      )}

      {/* Flash verde tras drop exitoso — 800ms de pulso alrededor
          del video para confirmar. `dragFlash` se autoresetea con
          setTimeout desde el handler. */}
      {dragFlash && (
        <div className="pointer-events-none absolute inset-0 z-40 animate-drop-flash rounded-lg ring-4 ring-good/70" />
      )}

      {/* Toast transitorio (2.5s) si el fichero soltado no era un
          sub reconocible. No usamos el componente <Toast> global
          porque queremos que viva DENTRO del player (fullscreen). */}
      {dragError && (
        <div className="pointer-events-none absolute left-1/2 bottom-28 z-40 -translate-x-1/2 rounded-full bg-black/85 px-5 py-2.5 text-[13px] text-ink shadow-lg">
          {dragError}
        </div>
      )}

      {/* Gradiente superior + top bar */}
      <div
        className={`pointer-events-none absolute inset-x-0 top-0 h-32 bg-gradient-to-b from-black/80 to-transparent transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0'
        }`}
      />
      {/* Drag region invisible SIEMPRE activa en el borde superior.
          `titleBarStyle: Overlay + hiddenTitle: true` esconde la
          barra del sistema, así que sin este strip la ventana no se
          puede arrastrar en modo player (el `<video>` ocupa el
          100%). Altura 28px = alto de los semaphore buttons en
          macOS; queda debajo del back button (que arranca en pt-5,
          y=20, con h-9 = ocupa hasta y=56 y captura sus clicks al
          estar en z-index mayor por orden DOM). Persiste con
          controles ocultos — no depende de `controlsVisible`. */}
      <div
        data-tauri-drag-region
        className="absolute inset-x-0 top-0 z-10 h-7"
        aria-hidden="true"
      />
      <div
        data-tauri-drag-region
        className={`absolute inset-x-0 top-0 z-20 flex items-center gap-3 px-5 pt-5 transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0 pointer-events-none'
        }`}
        onClick={(e) => e.stopPropagation()}
      >
        <button
          onClick={handleBack}
          className="flex h-9 w-9 items-center justify-center rounded-full bg-black/40 text-ink hover:bg-black/60"
          title={t('player.backTitle')}
        >
          <CaretLeft size={18} weight="bold" />
        </button>
        <div className="min-w-0 flex-1">
          <p className="truncate text-[15px] font-medium text-ink">
            {state.title}
            {state.isSeries && state.season != null && state.episode != null && (
              <span className="ml-2 text-[12px] font-normal text-muted">
                · S{String(state.season).padStart(2, '0')}E
                {String(state.episode).padStart(2, '0')}
              </span>
            )}
          </p>
          {state.subRelease && (
            <p className="truncate text-[12px] text-muted">
              {t('player.subs')}: {state.subRelease}
            </p>
          )}
        </div>
        {state.isSeries &&
          state.tmdbId != null &&
          state.season != null &&
          state.episode != null &&
          duration != null &&
          duration > 0 &&
          currentTime / duration > 0.9 && (
            <button
              onClick={() => {
                // §6 audit: "siguiente episodio" — dispara una
                // navegación al Torrents/series con E+1. La ruta ya
                // reutilizará la caché de sesión torrent del pack si
                // es el mismo magnet, así que la transición es rápida.
                const nextEp = state.episode! + 1
                void reportPositionNow().finally(() => {
                  nav(
                    `/torrents/series/${state.tmdbId}?season=${state.season}&episode=${nextEp}`,
                    { replace: true },
                  )
                })
              }}
              className="rounded-full border border-accent bg-accent/10 px-3 py-1.5 text-[12px] font-semibold text-accent hover:bg-accent/20"
              title={t('player.nextEpisodeTitle')}
            >
              {t('player.nextEpisode')}
            </button>
          )}
      </div>

      {/* Gradiente inferior + control bar */}
      <div
        className={`pointer-events-none absolute inset-x-0 bottom-0 h-40 bg-gradient-to-t from-black/85 to-transparent transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0'
        }`}
      />
      <div
        className={`absolute inset-x-0 bottom-0 px-6 pb-5 pt-2 transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0 pointer-events-none'
        }`}
        onClick={(e) => e.stopPropagation()}
      >
        <SeekBar
          currentTime={currentTime}
          duration={duration}
          videoRef={videoRef}
          onSeek={seekTo}
          hover={seekHover}
          setHover={setSeekHover}
        />
        <div className="mt-3 flex items-center gap-4">
          <button
            onClick={togglePlay}
            className="flex h-11 w-11 items-center justify-center rounded-full bg-accent text-on-accent transition-colors hover:bg-accent-hover"
            title={paused ? t('player.playTitle') : t('player.pauseTitle')}
          >
            {paused ? <Play size={20} weight="fill" /> : <Pause size={20} weight="fill" />}
          </button>

          <VolumeControl
            volume={volume}
            muted={muted}
            onVolume={setVolume}
            onToggleMute={() => setMuted((m) => !m)}
          />

          <div className="ml-2 text-[12px] tabular-nums text-body">
            {formatTime(currentTime)}
            {' / '}
            <span className="text-muted">
              {duration != null ? formatTime(duration) : '--:--'}
            </span>
          </div>

          <div className="flex-1" />

          <button
            onClick={() => setStatsPanelOpen((o) => !o)}
            className={`flex h-9 w-9 items-center justify-center rounded-full transition-colors ${
              statsPanelOpen
                ? 'bg-accent/20 text-accent'
                : 'text-ink hover:bg-surface'
            }`}
            title=""
            aria-label={t('player.stats')}
            aria-pressed={statsPanelOpen}
          >
            <Gauge size={18} weight={statsPanelOpen ? 'fill' : 'bold'} />
          </button>

          {audioTracks.length > 1 && (
            <button
              onClick={() => setAudioPanelOpen((o) => !o)}
              className={`flex h-9 items-center gap-1.5 rounded-full px-3 transition-colors ${
                audioPanelOpen
                  ? 'bg-accent/20 text-accent'
                  : 'text-ink hover:bg-surface'
              }`}
              title=""
              aria-label={t('player.audioTrack')}
              aria-pressed={audioPanelOpen}
            >
              <MusicNotes size={18} weight={audioPanelOpen ? 'fill' : 'bold'} />
              {audioTracks[activeAudioIdx]?.language && (
                <span className="text-[11px] font-medium uppercase">
                  {audioTracks[activeAudioIdx].language}
                </span>
              )}
            </button>
          )}

          <button
            onClick={() => setSubsPanelOpen((o) => !o)}
            className={`flex h-9 items-center gap-1.5 rounded-full px-3 transition-colors ${
              activeSub
                ? 'bg-accent/20 text-accent'
                : 'text-ink hover:bg-surface'
            }`}
            title={activeSub ? `${t('player.subs')}: ${activeSub.release}` : t('player.subtitlesTitle')}
          >
            <ClosedCaptioning size={18} weight={activeSub ? 'fill' : 'bold'} />
            {activeSub && (
              <span className="text-[11px] font-medium uppercase">
                {activeSub.language}
              </span>
            )}
          </button>

          <button
            onClick={toggleFullscreen}
            className="flex h-9 w-9 items-center justify-center rounded-full text-ink hover:bg-surface"
            title={t('player.fullscreenTitle')}
          >
            {isFullscreen ? (
              <ArrowsIn size={18} weight="bold" />
            ) : (
              <ArrowsOut size={18} weight="bold" />
            )}
          </button>
        </div>
      </div>

      {/* Panel lateral de subtítulos estilo Stremio. Sección
          "Del fichero" arriba (subs embedded del contenedor
          extraídos con ffmpeg), tabs de idioma de OpenSubtitles
          abajo, lista de releases del idioma seleccionado. Se abre
          desde el botón CC. Fuera del bloque de controles porque
          queremos que ocupe todo el alto del player, no solo la
          franja del control bar. */}
      {subsPanelOpen && (
        <SubsPanel
          subs={subsList}
          loading={subsLoading}
          activeFileId={
            activeSub?.source === 'openSubs'
              ? subsList?.find(
                  (s) => (s.release || s.file_name || '') === activeSub.release,
                )?.file_id ?? null
              : null
          }
          downloadingFileId={subDownloading}
          onPick={pickSub}
          onClear={clearSub}
          onClose={() => setSubsPanelOpen(false)}
          embeddedSubs={embeddedSubs}
          activeEmbeddedIdx={
            activeSub?.source === 'embedded' ? activeSub.idx : null
          }
          onPickEmbedded={pickEmbeddedSub}
        />
      )}

      {audioPanelOpen && (
        <AudioPanel
          tracks={audioTracks}
          activeIdx={activeAudioIdx}
          switching={audioSwitching}
          onPick={switchAudioTrack}
          onClose={() => setAudioPanelOpen(false)}
        />
      )}

      {/* Popover flotante con las stats en vivo del torrent — velocidad,
          peers, progreso, ETA. Se toggle desde el botón `Gauge` y se
          ancla al lado derecho por encima de la barra de controles.
          Solo info; no bloquea interacción con el resto del player. */}
      {statsPanelOpen && (
        <StatsPanel
          stats={stats}
          onClose={() => setStatsPanelOpen(false)}
        />
      )}
    </div>
  )
}
