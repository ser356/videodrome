import { useCallback, useEffect, useRef, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { getCurrentWindow } from '@tauri-apps/api/window'
import Hls from 'hls.js'
import {
  ArrowDown,
  ArrowsIn,
  ArrowsOut,
  CaretLeft,
  ClosedCaptioning,
  Gauge,
  MusicNotes,
  Pause,
  Play,
  SpeakerHigh,
  SpeakerNone,
  SpeakerX,
  UsersThree,
  X,
} from '@phosphor-icons/react'
import {
  downloadSubtitle,
  fetchEmbeddedSubtitle,
  formatSize,
  getMovieView,
  getPreferences,
  hlsUrl,
  probeStream,
  reportPosition,
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
import { getLocale, mergeSubtitleLangs } from '../lib/i18n'

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
  const location = useLocation()
  const state = (location.state ?? null) as PlayerState | null

  const videoRef = useRef<HTMLVideoElement | null>(null)
  const containerRef = useRef<HTMLDivElement | null>(null)

  const [stream, setStream] = useState<StreamInfo | null>(null)
  const [media, setMedia] = useState<MediaInfo | null>(null)
  const [error, setError] = useState<string | null>(null)

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

  // Backdrop de TMDB (URL absoluta al CDN). Se pinta como fondo del
  // StremioLoader durante arranque y seek. `null` mientras no
  // tengamos tmdbId o la petición esté en vuelo — el loader cae a
  // fondo negro plano.
  const [backdropUrl, setBackdropUrl] = useState<string | null>(null)

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

  // Reporta la posición absoluta al backend para persistir el
  // resume. Best-effort: silenciamos errores porque no queremos que
  // un fallo de IPC/persistencia rompa la reproducción. El backend
  // aplica la regla de completado (>95% del runtime → borra el
  // resume) por su cuenta.
  const reportPositionNow = useCallback(async () => {
    const id = streamIdRef.current
    const t = currentTimeRef.current
    const d = durationRef.current
    if (id == null || d == null || d <= 0) return
    try {
      // §6 audit series: acompañamos con episode meta cuando aplica
      // para que el backend guarde tmdb_id/season/episode dentro de
      // la entrada del resume — habilita el "continuar viendo" y el
      // "siguiente episodio" sin re-parsear el nombre del fichero.
      const s = state?.isSeries ? (state.season ?? null) : null
      const e = state?.isSeries ? (state.episode ?? null) : null
      const tid = state?.isSeries ? (state.tmdbId ?? null) : null
      await reportPosition(id, t, d, s, e, tid)
    } catch {
      /* best-effort */
    }
  }, [state])

  // Timer: reporta cada 15s mientras haya stream Y duración conocida.
  // Cuando la peli acaba (o se ve el 95%+), el backend borra el
  // resume dentro de `save_position`, así que no necesitamos parar
  // el timer explícitamente — reportar sobre un slot ya borrado
  // simplemente no lo recrea (`save_position` no crea la entrada
  // si el dir no existe, pero mientras el stream vive el dir sí).
  useEffect(() => {
    if (!stream || duration == null || duration <= 0) return
    const id = window.setInterval(() => {
      void reportPositionNow()
    }, 15000)
    return () => window.clearInterval(id)
  }, [stream, duration, reportPositionNow])

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
      setError('Sin magnet. Vuelve a la lista y proyecta un torrent.')
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
        setError(`No se pudo arrancar el stream: ${String(e)}`)
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
        // Probe puede fallar por: ffmpeg/ffprobe no instalado,
        // timeout, o CSP bloqueando 127.0.0.1. Sin `media` el
        // <video> nunca se monta (videoSrc = null) → onError nunca
        // dispara → spinner infinito. Hay que decidirle al user.
        if (!cancelled) {
          const msg = String(e)
          const looksLikeMissingFfmpeg = /ffmpeg/i.test(msg)
          setError(
            looksLikeMissingFfmpeg
              ? `ffmpeg no está disponible. ${ffmpegInstallHint()} Alternativa: cambia el reproductor a VLC en Ajustes.`
              : `No se pudo analizar el stream: ${msg}. Comprueba que ffmpeg está instalado, o cambia el reproductor a VLC en Ajustes.`,
          )
        }
      }
    })()
    return () => {
      cancelled = true
    }
  }, [stream])

  // Backdrop de TMDB para el StremioLoader. Se pide UNA vez al
  // montar (el tmdbId no cambia durante la vida del componente).
  // Fallo silencioso — el loader funciona sin backdrop.
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
      } catch {
        /* silencioso: sin backdrop el loader cae a fondo negro */
      }
    })()
    return () => {
      cancelled = true
    }
    // Solo depende del tmdbId — evitamos re-pedir cuando cambia el
    // resto del state (que puede pasar en cada re-render).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state?.tmdbId])

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

  // Auto-fetch del catálogo de subs en cuanto tenemos stream (para
  // que al abrir el panel ya estén listos). No bloquea la
  // reproducción — corre en paralelo con el probe.
  useEffect(() => {
    if (!stream) return
    let cancelled = false
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

  // Ajustes de <video> según state React.
  useEffect(() => {
    const v = videoRef.current
    if (!v) return
    v.volume = volume
    v.muted = muted
  }, [volume, muted])

  // Hotkeys globales. Se enganchan al document para funcionar aunque el
  // foco esté fuera del video (típico tras hacer click en un botón).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // No queremos que teclear en un input dispare hotkeys.
      const t = e.target as HTMLElement | null
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA')) return

      const v = videoRef.current
      if (!v) return
      switch (e.key) {
        case ' ':
        case 'k':
          e.preventDefault()
          if (v.paused) void v.play()
          else v.pause()
          break
        case 'j':
        case 'ArrowLeft':
          e.preventDefault()
          seekBy(-10)
          break
        case 'l':
        case 'ArrowRight':
          e.preventDefault()
          seekBy(10)
          break
        case 'ArrowUp':
          e.preventDefault()
          setVolume((x) => Math.min(1, x + 0.05))
          break
        case 'ArrowDown':
          e.preventDefault()
          setVolume((x) => Math.max(0, x - 0.05))
          break
        case 'm':
          setMuted((m) => !m)
          break
        case 'f':
          void toggleFullscreen()
          break
        case 'c':
          setSubsPanelOpen((o) => !o)
          break
        // Ajuste de sync del sub. Solo activos si hay sub cargado.
        //   [  ] retrasan/adelantan 500ms
        //   {  } (Shift) hacen ±100ms para ajuste fino
        //   ,  . rotan el factor de velocidad entre 3 valores
        //        conocidos (PAL, sin cambio, NTSC) para arreglar
        //        desfase creciente por frame rate mismatch.
        case '[':
          if (activeSub) {
            const delta = e.shiftKey ? -0.1 : -0.5
            setSubOffset((v) => {
              const next = +(v + delta).toFixed(2)
              showSyncHud(`Sub offset ${next > 0 ? '+' : ''}${next.toFixed(2)}s`)
              return next
            })
          }
          break
        case ']':
          if (activeSub) {
            const delta = e.shiftKey ? 0.1 : 0.5
            setSubOffset((v) => {
              const next = +(v + delta).toFixed(2)
              showSyncHud(`Sub offset ${next > 0 ? '+' : ''}${next.toFixed(2)}s`)
              return next
            })
          }
          break
        case ',':
        case '.': {
          if (activeSub) {
            // Rota entre 3 valores conocidos:
            //   0.95904 = 23.976/25 (sub PAL sobre video NTSC)
            //   1.00000 = sin cambio
            //   1.04271 = 25/23.976 (sub NTSC sobre video PAL)
            const steps = [0.95904, 1.0, 1.04271]
            const cur = steps.findIndex((s) => Math.abs(s - subSpeed) < 0.001)
            const dir = e.key === '.' ? 1 : -1
            const next = steps[(cur + dir + steps.length) % steps.length]
            setSubSpeed(next)
            const label =
              next === 1.0 ? 'sin cambio' : next < 1 ? 'sub PAL → NTSC' : 'sub NTSC → PAL'
            showSyncHud(`Sub speed ${next.toFixed(4)}× (${label})`)
          }
          break
        }
        case 'Escape':
          // Si estamos en fullscreen (Tauri window), salimos;
          // si no, volvemos atrás. Leemos de la ref para no
          // depender de re-suscribir el listener al entrar/salir
          // de fullscreen (que además no dispara re-render en las
          // deps actuales del efecto).
          if (isFullscreenRef.current) {
            void getCurrentWindow().setFullscreen(false).catch(() => {})
            setIsFullscreen(false)
          } else {
            handleBack()
          }
          break
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stream, activeSub, subSpeed])

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
    nav(-1)
  }

  const onTimeUpdate = () => {
    const v = videoRef.current
    if (!v) return
    setCurrentTime(v.currentTime)
  }

  // Ratón sobre la seek bar: hover-preview del tiempo.
  const [seekHover, setSeekHover] = useState<number | null>(null)

  if (!state) {
    return (
      <div className="flex h-full items-center justify-center text-body">
        <div className="text-center">
          <p className="text-[15px]">Sin datos de reproducci&oacute;n.</p>
          <button
            onClick={() => nav(-1)}
            className="mt-4 rounded-sm border border-hairline px-4 py-2 text-[13px] hover:bg-surface"
          >
            Volver
          </button>
        </div>
      </div>
    )
  }

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
        setError(
          'Tu navegador/webview no soporta HLS. Cambia el reproductor a VLC en Ajustes.',
        )
        return
      }
      const hls = new Hls({
        // VOD con segmentos bajo demanda: subir el timeout de carga
        // de fragmento por encima del peor caso de "ffmpeg
        // arrancando en frío" (serve_hls_segment espera hasta 30s
        // en el backend).
        fragLoadingTimeOut: 45_000,
        manifestLoadingTimeOut: 20_000,
        // Reintentos: piezas frías de librqbit pueden dar 404/wait
        // legítimo. hls.js aborta por defecto en 3 intentos.
        fragLoadingMaxRetry: 6,
      })
      hls.loadSource(videoSrc)
      hls.attachMedia(v)
      hls.on(Hls.Events.ERROR, (_evt, data) => {
        if (!data.fatal) return
        // Mapeamos los fatales al mismo canal de error del player.
        // Si es de tipo NETWORK (típicamente 404 tras muchos
        // reintentos) o MEDIA (transmux fallido), damos un mensaje
        // útil. `startLoad()` recupera algunos NETWORK / MEDIA no
        // fatales — pero cuando llega `fatal: true` no hay vuelta.
        console.warn('[hls] fatal', data.type, data.details)
        setError(
          `Fallo de HLS (${data.type}/${data.details}). ` +
            'Prueba a cambiar el reproductor a VLC en Ajustes.',
        )
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
    // para pedir la playlist de cero.
  }, [videoSrc, needsHls, activeAudioIdx])

  return (
    <div
      ref={containerRef}
      className={`relative h-screen w-full overflow-hidden bg-black ${
        controlsVisible ? '' : 'cursor-none'
      }`}
      onMouseMove={bumpControls}
      onClick={() => {
        // Click en el fondo: toggle play. Los controles interceptan
        // sus propios eventos con stopPropagation.
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
            // Con HLS (o `/video` direct) el error del `<video>` es
            // ya definitivo — no hay fallback más agresivo que el
            // player HTML pueda montar. Si falla aquí, el usuario
            // puede volver a Ajustes y cambiar a VLC externo.
            console.warn(`<video> error code ${code}: ${msg}`)
            setError(
              'No se pudo reproducir esta pel\u00edcula en el player. ' +
                'Prueba a cambiar el reproductor a VLC desde Ajustes.',
            )
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

      {/* Loader minimalista estilo Stremio: pantalla de arranque
          limpia con el backdrop de la peli de fondo (si TMDB nos lo
          dio), título y un spinner. Se pinta en tres casos:
            1. Arranque: aún no hemos tenido ni un `playing` (falta
               `stream` o `hasStartedPlayback = false`).
            2. Seek: user movió la seekbar y estamos esperando que
               el buffer se rellene en el offset nuevo.
            3. Cambio de pista de audio: backend está purgando
               segmentos y respawneando ffmpeg → tapa la transición.
          Nada de MiB/s / ETA / peers — el user eligió reproducir
          esta peli, no quiere ver plumbing. Las stats siguen
          disponibles bajo demanda desde el botón `Gauge`. */}
      {(!stream || !hasStartedPlayback || seeking || audioSwitching) &&
        !error && <StremioLoader title={state.title} backdropUrl={backdropUrl} />}
      {buffering && hasStartedPlayback && !seeking && !audioSwitching && !error && (
        <div className="pointer-events-none absolute right-6 top-6">
          <div className="h-6 w-6 animate-spin rounded-full border-2 border-white/70 border-t-transparent" />
        </div>
      )}

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
              Volver
            </button>
          </div>
        </div>
      )}

      {/* Gradiente superior + top bar */}
      <div
        className={`pointer-events-none absolute inset-x-0 top-0 h-32 bg-gradient-to-b from-black/80 to-transparent transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0'
        }`}
      />
      <div
        className={`absolute inset-x-0 top-0 flex items-center gap-3 px-5 pt-5 transition-opacity ${
          controlsVisible ? 'opacity-100' : 'opacity-0 pointer-events-none'
        }`}
        onClick={(e) => e.stopPropagation()}
      >
        <button
          onClick={handleBack}
          className="flex h-9 w-9 items-center justify-center rounded-full bg-black/40 text-ink hover:bg-black/60"
          title="Volver (Esc)"
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
              Subs: {state.subRelease}
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
              title="Siguiente episodio"
            >
              Siguiente episodio →
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
            title={paused ? 'Play (Space)' : 'Pause (Space)'}
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
            title="Estadísticas del stream"
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
              title="Pista de audio"
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
            title={activeSub ? `Sub: ${activeSub.release}` : 'Subtítulos (C)'}
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
            title="Fullscreen (F)"
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

// ── Sub-components ─────────────────────────────────────────────────────────

function SeekBar({
  currentTime,
  duration,
  videoRef,
  onSeek,
  hover,
  setHover,
}: {
  currentTime: number
  duration: number | null
  videoRef: React.RefObject<HTMLVideoElement | null>
  onSeek: (t: number) => void
  hover: number | null
  setHover: (t: number | null) => void
}) {
  const barRef = useRef<HTMLDivElement | null>(null)
  const [buffered, setBuffered] = useState<[number, number][]>([])

  // Refresca los rangos bufferizados cada 500ms mientras el player
  // está montado. `TimeRanges` no dispara eventos, hay que pollear.
  // `v.buffered` está ya en tiempo absoluto (el modelo VOD no
  // remapea el timeline entre modos DIRECT y TRANSMUX).
  useEffect(() => {
    const id = window.setInterval(() => {
      const v = videoRef.current
      if (!v) return
      const rs: [number, number][] = []
      for (let i = 0; i < v.buffered.length; i++) {
        rs.push([v.buffered.start(i), v.buffered.end(i)])
      }
      setBuffered(rs)
    }, 500)
    return () => window.clearInterval(id)
  }, [videoRef])

  const timeFromEvent = (e: React.MouseEvent) => {
    const bar = barRef.current
    if (!bar || !duration) return null
    const rect = bar.getBoundingClientRect()
    const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))
    return ratio * duration
  }

  const pct = (t: number) =>
    duration && duration > 0 ? (t / duration) * 100 : 0

  return (
    <div
      ref={barRef}
      className="group relative h-6 cursor-pointer"
      onClick={(e) => {
        const t = timeFromEvent(e)
        if (t != null) onSeek(t)
      }}
      onMouseMove={(e) => setHover(timeFromEvent(e))}
      onMouseLeave={() => setHover(null)}
    >
      {/* Base track */}
      <div className="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 rounded-full bg-white/15" />
      {/* Buffered rangos */}
      {buffered.map(([s, e], i) => (
        <div
          key={i}
          className="absolute top-1/2 h-1 -translate-y-1/2 rounded-full bg-white/30"
          style={{
            left: `${pct(s)}%`,
            width: `${Math.max(0, pct(e) - pct(s))}%`,
          }}
        />
      ))}
      {/* Playhead */}
      <div
        className="absolute top-1/2 h-1 -translate-y-1/2 rounded-full bg-accent"
        style={{ width: `${pct(currentTime)}%` }}
      />
      {/* Thumb */}
      <div
        className="absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-accent shadow-md transition-transform group-hover:scale-125"
        style={{ left: `${pct(currentTime)}%` }}
      />
      {/* Hover tooltip */}
      {hover != null && duration && (
        <div
          className="pointer-events-none absolute -top-8 -translate-x-1/2 rounded-sm bg-black/85 px-2 py-1 text-[11px] tabular-nums text-ink"
          style={{ left: `${pct(hover)}%` }}
        >
          {formatTime(hover)}
        </div>
      )}
    </div>
  )
}

function VolumeControl({
  volume,
  muted,
  onVolume,
  onToggleMute,
}: {
  volume: number
  muted: boolean
  onVolume: (v: number) => void
  onToggleMute: () => void
}) {
  const Icon = muted || volume === 0 ? SpeakerX : volume < 0.5 ? SpeakerNone : SpeakerHigh
  return (
    <div className="group flex items-center gap-2">
      <button
        onClick={onToggleMute}
        className="flex h-9 w-9 items-center justify-center rounded-full text-ink hover:bg-surface"
        title={muted ? 'Unmute (M)' : 'Mute (M)'}
      >
        <Icon size={18} weight="bold" />
      </button>
      <input
        type="range"
        min={0}
        max={1}
        step={0.01}
        value={muted ? 0 : volume}
        onChange={(e) => {
          const v = parseFloat(e.target.value)
          onVolume(v)
          if (v > 0 && muted) onToggleMute()
        }}
        className="h-1 w-20 cursor-pointer appearance-none rounded-full bg-white/15 accent-accent opacity-0 transition-opacity group-hover:opacity-100"
      />
    </div>
  )
}

function formatTime(s: number): string {
  if (!isFinite(s) || s < 0) return '0:00'
  const hh = Math.floor(s / 3600)
  const mm = Math.floor((s % 3600) / 60)
  const ss = Math.floor(s % 60)
  const pad = (n: number) => n.toString().padStart(2, '0')
  return hh > 0 ? `${hh}:${pad(mm)}:${pad(ss)}` : `${mm}:${pad(ss)}`
}

/** Instrucción de instalación de ffmpeg específica del SO en el que
 * corre la WebView. Se usa en el mensaje de error del player cuando
 * `probeStream` falla por falta del binario. Detección por
 * `navigator.userAgent` — Tauri no expone el OS al frontend sin el
 * plugin `@tauri-apps/plugin-os`, y este helper es suficiente para
 * los tres SO que soportamos. */
function ffmpegInstallHint(): string {
  const ua = navigator.userAgent
  if (ua.includes('Windows')) {
    return 'Instálalo con `winget install Gyan.FFmpeg` (o `scoop install ffmpeg`).'
  }
  if (ua.includes('Mac OS X') || ua.includes('Macintosh')) {
    return 'Instálalo con `brew install ffmpeg`.'
  }
  if (ua.includes('Linux')) {
    return 'Instálalo con el gestor de paquetes de tu distro (`sudo apt install ffmpeg`, `sudo dnf install ffmpeg`, `sudo pacman -S ffmpeg`).'
  }
  return 'Instala ffmpeg y asegúrate de que esté en el PATH.'
}

/** Detecta codecs de subtítulos de imagen (bitmap) que ffmpeg no
 * puede convertir a WebVTT sin OCR. La UI oculta estas pistas del
 * panel — si el user las eligiera, el endpoint `/subs/embedded/N.vtt`
 * devolvería HTTP 415 y el error saldría por consola. Mejor no
 * ofrecerlas. Lista basada en los codecs de subs de ffmpeg. */
function isBitmapSubCodec(codec: string): boolean {
  return (
    codec === 'hdmv_pgs_subtitle' ||
    codec === 'pgssub' ||
    codec === 'pgs' ||
    codec === 'dvd_subtitle' ||
    codec === 'dvdsub' ||
    codec === 'dvb_subtitle' ||
    codec === 'dvbsub' ||
    codec === 'xsub'
  )
}

// ── Panel de subtítulos embebido en el player (estilo Stremio) ─────────────

/** Nombre legible del idioma según código ISO 639-1 (`"es"`, `"en"`,
 * `"pt-BR"`…). Usa `Intl.DisplayNames` del navegador con fallback al
 * código en mayúsculas si el runtime no lo conoce. */
function languageLabel(code: string): string {
  if (!code) return '—'
  try {
    const dn = new Intl.DisplayNames(['es'], { type: 'language' })
    const name = dn.of(code)
    if (name && name !== code) {
      // Capitaliza primera letra ("español" → "Español").
      return name.charAt(0).toUpperCase() + name.slice(1)
    }
  } catch {
    // Runtime sin Intl.DisplayNames o código desconocido → fallback.
  }
  return code.toUpperCase()
}

/**
 * Panel lateral de pistas de audio. Análogo al `SubsPanel` pero más
 * simple: solo una lista plana, sin tabs por idioma (los torrents
 * multi-audio suelen tener 2-4 pistas, no cientos). Cada pista
 * muestra idioma + codec + canales; el user pulsa una y el player:
 *
 *   1. Guarda `currentTime` + `paused` antes del switch.
 *   2. POST `/hls/audio?idx=N` al backend → mata ffmpeg, purga
 *      segmentos, guarda la selección.
 *   3. El `useEffect` de hls.js se re-ejecuta al cambiar
 *      `activeAudioIdx` → destroy + new Hls sobre la misma URL de
 *      playlist → hls.js pide el manifest de cero y ffmpeg
 *      respawnea con `-map 0:a:<idx>`.
 *   4. `onLoadedMetadata` restaura `currentTime` y reanuda si
 *      estaba playing.
 *
 * Durante la transición el `StremioLoader` tapa el `<video>` para
 * que el user vea "cargando" y no un frame frozen.
 */
function AudioPanel({
  tracks,
  activeIdx,
  switching,
  onPick,
  onClose,
}: {
  tracks: MediaStream[]
  activeIdx: number
  /** `true` mientras el backend está purgando + respawneando.
   * Deshabilita clicks para evitar switches concurrentes. */
  switching: boolean
  onPick: (idx: number) => void
  onClose: () => void
}) {
  return (
    <div
      className="absolute inset-y-0 right-0 z-30 flex w-full max-w-[420px] flex-col border-l border-hairline bg-black/95 backdrop-blur-lg"
      onClick={(e) => e.stopPropagation()}
    >
      <header className="flex items-center justify-between border-b border-hairline px-5 py-4">
        <div>
          <h2 className="text-[15px] font-semibold text-ink">Pista de audio</h2>
          <p className="mt-0.5 text-[11px] text-muted">
            {tracks.length} disponible{tracks.length === 1 ? '' : 's'}
          </p>
        </div>
        <button
          onClick={onClose}
          className="flex h-8 w-8 items-center justify-center rounded-full text-muted hover:bg-surface hover:text-ink"
          aria-label="Cerrar"
        >
          <X size={16} weight="bold" />
        </button>
      </header>

      <ul className="flex-1 divide-y divide-hairline-soft overflow-y-auto">
        {tracks.map((t, idx) => {
          const isActive = idx === activeIdx
          // El label del track del contenedor suele traer info útil
          // (ej: "English 5.1 Commentary"). Si no, componemos con
          // idioma + codec.
          const label =
            t.title ||
            [t.language ? languageLabel(t.language) : null, t.codec]
              .filter(Boolean)
              .join(' · ') ||
            `Pista ${idx + 1}`
          return (
            <li key={`audio-${idx}`}>
              <button
                onClick={() => onPick(idx)}
                disabled={switching || isActive}
                className={`flex w-full items-start justify-between gap-3 px-5 py-3 text-left transition-colors ${
                  isActive
                    ? 'bg-accent/10'
                    : 'hover:bg-surface disabled:opacity-50'
                }`}
              >
                <div className="min-w-0 flex-1">
                  <p className="truncate text-[13px] text-ink">{label}</p>
                  <p className="mt-0.5 text-[11px] text-muted">
                    {t.language ? languageLabel(t.language) : 'Idioma desconocido'}
                    <span className="mx-1.5 text-dim">·</span>
                    <span className="text-dim">{t.codec}</span>
                  </p>
                </div>
                {isActive && (
                  <span className="mt-0.5 text-[11px] font-medium text-accent">
                    {switching ? 'Cargando…' : 'Activo'}
                  </span>
                )}
              </button>
            </li>
          )
        })}
      </ul>
    </div>
  )
}

/**
 * Panel lateral con los subtítulos disponibles agrupados por idioma.
 * Tabs de idioma arriba (ordenadas por número de subs disponibles);
 * abajo, la lista de releases para el idioma seleccionado ordenados
 * por descargas.
 */
function SubsPanel({
  subs,
  loading,
  activeFileId,
  downloadingFileId,
  onPick,
  onClear,
  onClose,
  embeddedSubs,
  activeEmbeddedIdx,
  onPickEmbedded,
}: {
  subs: Subtitle[] | null
  loading: boolean
  activeFileId: number | null
  downloadingFileId: number | null
  onPick: (sub: Subtitle) => void
  onClear: () => void
  onClose: () => void
  /** Subs embebidos (extraídos del contenedor con ffmpeg). Ya
   * vienen filtrados por el caller para excluir bitmap (PGS/DVBSUB).
   * Si está vacío, la sección "Del fichero" no se pinta. */
  embeddedSubs: MediaStream[]
  /** Índice activo dentro de `embeddedSubs` (0-based), o `null` si
   * el sub activo no es embedded. */
  activeEmbeddedIdx: number | null
  onPickEmbedded: (stream: MediaStream, subIdx: number) => void
}) {
  // Idiomas presentes en la lista + conteo. Se ordenan por count
  // descendente y luego alfabético → el idioma con más opciones
  // aparece primero (típicamente inglés).
  const [langs, defaultLang] = (() => {
    if (!subs || subs.length === 0) return [[] as { code: string; count: number }[], null]
    const map = new Map<string, number>()
    for (const s of subs) {
      map.set(s.language, (map.get(s.language) ?? 0) + 1)
    }
    const arr = Array.from(map, ([code, count]) => ({ code, count })).sort(
      (a, b) => b.count - a.count || a.code.localeCompare(b.code),
    )
    // Prioriza español si está entre los 3 primeros idiomas (aunque
    // no sea el que más subs tiene) — mejor default para el usuario
    // hispanohablante que abrir siempre en inglés.
    const es = arr.findIndex((l) => l.code === 'es')
    if (es > 0 && es < 3) {
      const [esItem] = arr.splice(es, 1)
      arr.unshift(esItem)
    }
    return [arr, arr[0]?.code ?? null]
  })()

  const [selectedLang, setSelectedLang] = useState<string | null>(defaultLang)
  // Sincroniza selectedLang si cambia la lista (nueva peli, refetch).
  useEffect(() => {
    if (selectedLang && langs.some((l) => l.code === selectedLang)) return
    setSelectedLang(defaultLang)
  }, [defaultLang, langs, selectedLang])

  const filtered = subs?.filter((s) => s.language === selectedLang) ?? []

  return (
    <div
      className="absolute inset-y-0 right-0 z-30 flex w-full max-w-[420px] flex-col border-l border-hairline bg-black/95 backdrop-blur-lg"
      onClick={(e) => e.stopPropagation()}
    >
      <header className="flex items-center justify-between border-b border-hairline px-5 py-4">
        <div>
          <h2 className="text-[15px] font-semibold text-ink">Subtítulos</h2>
          {(activeFileId != null || activeEmbeddedIdx != null) && (
            <button
              onClick={onClear}
              className="mt-0.5 text-[11px] text-muted hover:text-ink"
            >
              Quitar el actual
            </button>
          )}
        </div>
        <button
          onClick={onClose}
          className="flex h-8 w-8 items-center justify-center rounded-full text-muted hover:bg-surface hover:text-ink"
          aria-label="Cerrar"
        >
          <X size={16} weight="bold" />
        </button>
      </header>

      {/* Sección "Del fichero" — subs embedded del contenedor
          (SRT/ASS/SSA extraídos con ffmpeg). Aparece SIEMPRE arriba
          si hay pistas; el user ve las pistas nativas antes que las
          descargadas, que es lo que hace Stremio. */}
      {embeddedSubs.length > 0 && (
        <div className="border-b border-hairline">
          <p className="px-5 pt-3 text-[10px] uppercase tracking-[0.14em] text-dim">
            Del fichero
          </p>
          <ul className="divide-y divide-hairline-soft">
            {embeddedSubs.map((sub, idx) => {
              const isActive = idx === activeEmbeddedIdx
              const label = sub.title || `Pista ${idx + 1}`
              return (
                <li key={`emb-${idx}`}>
                  <button
                    onClick={() => onPickEmbedded(sub, idx)}
                    className={`flex w-full items-start justify-between gap-3 px-5 py-3 text-left transition-colors ${
                      isActive ? 'bg-accent/10' : 'hover:bg-surface'
                    }`}
                  >
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-[13px] text-ink">{label}</p>
                      <p className="mt-0.5 text-[11px] text-muted">
                        {sub.language
                          ? languageLabel(sub.language)
                          : 'Idioma desconocido'}
                        <span className="mx-1.5 text-dim">·</span>
                        <span className="text-dim">{sub.codec}</span>
                      </p>
                    </div>
                    {isActive && (
                      <span className="mt-0.5 text-[11px] font-medium text-accent">
                        Activo
                      </span>
                    )}
                  </button>
                </li>
              )
            })}
          </ul>
        </div>
      )}

      {loading && (
        <div className="flex flex-1 items-center justify-center">
          <div className="h-6 w-6 animate-spin rounded-full border-2 border-accent border-t-transparent" />
        </div>
      )}

      {!loading &&
        (subs === null || subs.length === 0) &&
        embeddedSubs.length === 0 && (
          <div className="flex flex-1 flex-col items-center justify-center px-6 text-center">
            <p className="text-[14px] text-body">Sin subtítulos disponibles.</p>
            <p className="mt-1 text-[12px] text-muted">
              OpenSubtitles no tiene resultados para este título y el
              contenedor no lleva subs embebidos.
            </p>
          </div>
        )}

      {!loading && subs && subs.length > 0 && (
        <>
          <div className="flex gap-1 overflow-x-auto border-b border-hairline px-3 py-2">
            {langs.map((l) => (
              <button
                key={l.code}
                onClick={() => setSelectedLang(l.code)}
                className={`shrink-0 rounded-full px-3 py-1.5 text-[12px] transition-colors ${
                  selectedLang === l.code
                    ? 'bg-accent text-on-accent'
                    : 'bg-surface text-body hover:bg-surface-hi'
                }`}
              >
                {languageLabel(l.code)}{' '}
                <span
                  className={
                    selectedLang === l.code ? 'opacity-70' : 'text-muted'
                  }
                >
                  {l.count}
                </span>
              </button>
            ))}
          </div>

          <ul className="flex-1 divide-y divide-hairline-soft overflow-y-auto">
            {filtered.map((sub) => {
              const isActive = sub.file_id === activeFileId
              const isDownloading = sub.file_id === downloadingFileId
              return (
                <li key={sub.file_id}>
                  <button
                    disabled={isDownloading || downloadingFileId !== null}
                    onClick={() => onPick(sub)}
                    className={`flex w-full items-start justify-between gap-3 px-5 py-3 text-left transition-colors ${
                      isActive
                        ? 'bg-accent/10'
                        : 'hover:bg-surface disabled:opacity-50'
                    }`}
                  >
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-[13px] text-ink">
                        {sub.release || sub.file_name || 'Subtítulo'}
                      </p>
                      <p className="mt-0.5 flex items-center gap-2 text-[11px] text-muted">
                        <span>{sub.downloads.toLocaleString()} descargas</span>
                        {sub.from_trusted && (
                          <span
                            className="rounded-sm border border-good/40 bg-good/10 px-1.5 py-0.5 text-[10px] font-medium text-good"
                            title="Verificado por moderador de OpenSubtitles"
                          >
                            Trusted
                          </span>
                        )}
                        {sub.hearing_impaired && (
                          <span
                            className="rounded-sm border border-hairline px-1.5 py-0.5 text-[10px]"
                            title="Transcripción para sordos"
                          >
                            SDH
                          </span>
                        )}
                      </p>
                    </div>
                    {isActive && (
                      <span className="mt-0.5 text-[11px] font-medium text-accent">
                        Activo
                      </span>
                    )}
                    {isDownloading && (
                      <div className="mt-0.5 h-4 w-4 animate-spin rounded-full border-2 border-accent border-t-transparent" />
                    )}
                  </button>
                </li>
              )
            })}
          </ul>
        </>
      )}
    </div>
  )
}

/**
 * Loader minimalista al estilo Stremio: fondo con el backdrop de la
 * peli (o negro plano si no lo tenemos) + gradiente que oscurece el
 * centro-inferior para dar contraste al título + spinner sutil bajo
 * él. Nada de estadísticas de descarga durante el arranque — el
 * usuario ya eligió la peli y no quiere ver plumbing del protocolo.
 * Las stats siguen accesibles bajo demanda desde el botón `Gauge`.
 *
 * Se reutiliza para dos estados:
 *   - Arranque inicial (torrent + probe + primer buffer HLS).
 *   - Seek en vuelo (esperando que el buffer se rellene en el
 *     offset nuevo tras `v.currentTime = t`).
 *
 * Diseño:
 *   - Backdrop en `absolute inset-0` con `background-image` inline
 *     (evita reescribir Tailwind config). Fondo negro por defecto
 *     debajo por si la imagen tarda / falla.
 *   - Overlay de gradiente radial para que el título respire sin
 *     competir con el poster.
 *   - Título centrado, tipografía media-grande, tracking apretado.
 *   - Spinner delgado + "Cargando…" en uppercase con tracking wide.
 *   - `pointer-events-none` para no bloquear los controles debajo
 *     (por si el user quiere pulsar back/fullscreen durante seek).
 */
function StremioLoader({
  title,
  backdropUrl,
}: {
  title: string
  backdropUrl: string | null
}) {
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
        <h1 className="text-balance text-[26px] font-medium tracking-tight text-ink drop-shadow-[0_2px_8px_rgba(0,0,0,0.9)] sm:text-[32px]">
          {title}
        </h1>
        <div className="flex items-center gap-3 text-[12px] uppercase tracking-[0.18em] text-dim drop-shadow-[0_1px_4px_rgba(0,0,0,0.9)]">
          <span className="h-4 w-4 animate-spin rounded-full border-2 border-white/40 border-t-white" />
          <span>
            Cargando<LoadingDots />
          </span>
        </div>
      </div>
    </div>
  )
}

/**
 * Popover flotante con las estadísticas en vivo del torrent. Se
 * toggle desde el botón `Gauge` en la barra de controles. Anclado
 * arriba a la derecha por encima del control bar; no es modal
 * (`pointer-events` propios pero fondo transparente).
 *
 * Muestra velocidad, peers, progreso descargado y ETA. ETA es
 * tiempo estimado para descargar el fichero entero al ritmo
 * actual — pesimista para el user (la peli suele arrancar antes),
 * pero es el único número honesto que sabemos computar.
 */
function StatsPanel({
  stats,
  onClose,
}: {
  stats: StreamStats | null
  onClose: () => void
}) {
  const hasProgress = stats != null && stats.total_bytes > 0
  const pct = hasProgress
    ? (stats!.progress_bytes / stats!.total_bytes) * 100
    : 0
  const remaining = hasProgress
    ? Math.max(0, stats!.total_bytes - stats!.progress_bytes)
    : 0
  const bytesPerSec = stats ? stats.down_mbps * 1024 * 1024 : 0
  const etaSec = bytesPerSec > 0 ? remaining / bytesPerSec : null

  return (
    <div
      className="absolute bottom-24 right-6 z-30 w-[280px] rounded-xl border border-white/10 bg-black/80 p-4 shadow-[0_20px_60px_-20px_rgba(0,0,0,0.6)] backdrop-blur-xl backdrop-saturate-150"
      onClick={(e) => e.stopPropagation()}
    >
      <header className="mb-3 flex items-center justify-between">
        <p className="text-[11px] uppercase tracking-[0.14em] text-dim">
          Stream
        </p>
        <button
          onClick={onClose}
          className="flex h-6 w-6 items-center justify-center rounded-full text-muted hover:bg-surface hover:text-ink"
          aria-label="Cerrar"
        >
          <X size={12} weight="bold" />
        </button>
      </header>

      {!stats && (
        <div className="flex items-center gap-2 py-2 text-[12px] text-muted">
          <span className="h-3 w-3 animate-spin rounded-full border-2 border-accent border-t-transparent" />
          <span>Esperando datos…</span>
        </div>
      )}

      {stats && (
        <>
          {hasProgress && (
            <div className="mb-3 h-1 w-full overflow-hidden rounded-full bg-white/10">
              <div
                className="h-full rounded-full bg-gradient-to-r from-accent to-good transition-all duration-700 ease-out"
                style={{ width: `${Math.max(2, pct).toFixed(1)}%` }}
              />
            </div>
          )}
          <div className="grid grid-cols-2 gap-x-6 gap-y-2 text-[12px] tabular-nums text-body">
            <Metric
              icon={<ArrowDown size={13} weight="bold" />}
              label="Velocidad"
              value={
                <span className="text-good">
                  {stats.down_mbps.toFixed(2)}{' '}
                  <span className="text-[10px] uppercase text-dim">MiB/s</span>
                </span>
              }
            />
            <Metric
              icon={<UsersThree size={13} weight="bold" />}
              label="Peers"
              value={stats.live_peers.toString()}
            />
            <Metric
              label="ETA"
              value={etaSec != null ? formatEta(etaSec) : '—'}
            />
            <Metric
              label="Progreso"
              value={hasProgress ? `${pct.toFixed(1)} %` : '—'}
            />
            {hasProgress && (
              <Metric
                label="Descargado"
                value={
                  <span>
                    {formatSize(stats.progress_bytes)}{' '}
                    <span className="text-dim">/ {formatSize(stats.total_bytes)}</span>
                  </span>
                }
                span={2}
              />
            )}
          </div>
        </>
      )}
    </div>
  )
}

/** Métrica del HUD: label pequeña + valor con opción a icono
 * inline. `span=2` la hace ocupar toda la fila del grid. */
function Metric({
  icon,
  label,
  value,
  span = 1,
}: {
  icon?: React.ReactNode
  label: string
  value: React.ReactNode
  span?: 1 | 2
}) {
  return (
    <div
      className={`flex flex-col items-start gap-0.5 ${span === 2 ? 'col-span-2' : ''}`}
    >
      <span className="flex items-center gap-1 text-[10px] uppercase tracking-[0.1em] text-dim">
        {icon}
        {label}
      </span>
      <span className="text-[13px] font-medium text-ink">{value}</span>
    </div>
  )
}

/** Tres puntitos animados en secuencia — imita el ellipsis de
 * carga clásico pero controlado (sin depender de fuentes/emoji). */
function LoadingDots() {
  return (
    <span className="ml-0.5 inline-flex">
      <span
        className="inline-block animate-pulse"
        style={{ animationDelay: '0ms' }}
      >
        .
      </span>
      <span
        className="inline-block animate-pulse"
        style={{ animationDelay: '180ms' }}
      >
        .
      </span>
      <span
        className="inline-block animate-pulse"
        style={{ animationDelay: '360ms' }}
      >
        .
      </span>
    </span>
  )
}

/** Formatea segundos como "12s", "3m 45s" o "1h 12m". */
function formatEta(sec: number): string {
  if (!Number.isFinite(sec) || sec <= 0) return '—'
  if (sec < 60) return `${Math.ceil(sec)}s`
  if (sec < 3600) {
    const m = Math.floor(sec / 60)
    const s = Math.floor(sec % 60)
    return `${m}m ${s.toString().padStart(2, '0')}s`
  }
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  return `${h}h ${m.toString().padStart(2, '0')}m`
}
