import { useEffect, useMemo, useRef, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { getCurrentWindow } from '@tauri-apps/api/window'
import {
  stopStream,
  type MediaStream,
} from '../lib/api'
import { useT } from '../lib/i18n'
import { isBitmapSubCodec } from './player/utils'
import { StatsPanel } from './player/controls'
import { AudioPanel, SubsPanel } from './player/panels'
import {
  ErrorOverlay,
  PlayerLoader,
  SubDragErrorToast,
  SubDragFlash,
  SubDragOverlay,
  SyncHud,
  VolumeHud,
} from './player/overlays'
import { PlayerControlBar, PlayerTopBar } from './player/bars'
import { useAudioSwitch, useHlsAttach } from './player/useHlsAttach'
import { useMediaControls } from './player/useMediaControls'
import { useResumePosition } from './player/useResumePosition'
import { useStreamLifecycle } from './player/useStreamLifecycle'
import { useSubtitles, type ActiveSub } from './player/useSubtitles'
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

export function Player() {
  const nav = useNavigate()
  const t = useT()
  const location = useLocation()
  const state = (location.state ?? null) as PlayerState | null

  const videoRef = useRef<HTMLVideoElement | null>(null)
  const containerRef = useRef<HTMLDivElement | null>(null)

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

  // Ref al `reportPositionNow` para el cleanup del hook de lifecycle
  // (flush final antes de `stopStream`). Va por ref para romper la
  // dependencia circular: `useResumePosition` consume estado que
  // `useStreamLifecycle` produce (posterPathRaw / backdropPathRaw /
  // yearFromView), así que no puede recibir `reportPositionNow`
  // como valor. El sync ref ← callback vive más abajo, tras la
  // llamada a `useResumePosition`.
  const reportPositionRef = useRef<() => Promise<void>>(
    async () => {},
  )

  const {
    stream,
    media,
    error,
    errorBackTo,
    directFailed,
    setError,
    setErrorBackTo,
    setDirectFailed,
    backdropUrl,
    logoUrl,
    posterPathRaw,
    backdropPathRaw,
    yearFromView,
    stats,
  } = useStreamLifecycle({
    state,
    torrentsRoute,
    reportPositionRef,
    t,
  })

  // Controles de reproducción: estado + refs + callbacks (seekTo,
  // seekBy, togglePlay, toggleFullscreen, onTimeUpdate, bumpControls,
  // autohide, poll de fullscreen, sync volume/muted) extraído a
  // `useMediaControls`. `currentTimeRef` / `isFullscreenRef` los
  // devuelve el hook para que el reporter periódico y las hotkeys
  // los lean sin re-suscribirse.
  const {
    paused,
    setPaused,
    currentTime,
    volume,
    setVolume,
    muted,
    setMuted,
    buffering,
    setBuffering,
    isFullscreen,
    setIsFullscreen,
    controlsVisible,
    seeking,
    setSeeking,
    hasStartedPlayback,
    setHasStartedPlayback,
    stalledLong,
    currentTimeRef,
    isFullscreenRef,
    volumeHud,
    bumpVolumeHud,
    primeAudio,
    seekTo,
    seekBy,
    togglePlay,
    toggleFullscreen,
    onTimeUpdate,
    bumpControls,
  } = useMediaControls({
    videoRef,
    initialSeconds: state?.startSeconds ?? 0,
  })
  // `durationRef` + `streamIdRef` se leen desde el timer de report y
  // desde el cleanup del useEffect de mount — ambos no reactivos.
  const durationRef = useRef<number | null>(null)
  const streamIdRef = useRef<number | null>(null)
  // Ref al `activeSub` último para que el reporter periódico lo
  // persista sin re-crear el callback cada vez que el user cambia
  // de pista. Se declara aquí (antes de `useResumePosition` y de
  // `useSubtitles`) para romper cualquier orden de creación entre
  // los dos hooks. `useSubtitles` es el propietario y lo sincroniza
  // con `activeSub` vía effect; `useResumePosition` solo lee.
  const activeSubRef = useRef<ActiveSub | null>(null)

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

  // Sincroniza el ref pasado a `useStreamLifecycle` con el
  // `reportPositionNow` actual — el cleanup del mount effect del
  // hook lo lee al desmontar para hacer un flush final antes de
  // `stopStream`. Ver comentario en `useStreamLifecycle.ts` sobre
  // la circularidad.
  useEffect(() => {
    reportPositionRef.current = reportPositionNow
  }, [reportPositionNow])

  // Subtítulos: modelo Stremio-like. Pipeline completo (fetch,
  // hidratación desde movie_progress, VTT blob, shift de cues por
  // subOffset/subSpeed, drag&drop de subs locales) extraído a
  // `useSubtitles`. Player.tsx solo consume los valores devueltos.
  const {
    activeSub,
    subsList,
    subsLoading,
    subsPanelOpen,
    setSubsPanelOpen,
    subDownloading,
    vttUrl,
    setSubOffset,
    subSpeed,
    setSubSpeed,
    syncHud,
    showSyncHud,
    dragActive,
    dragFlash,
    dragError,
    pickSub,
    pickEmbeddedSub,
    clearSub,
  } = useSubtitles({
    stream,
    state,
    videoRef,
    activeSubRef,
    t,
  })

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
  // `true` durante el cambio de pista: backend está purgando
  // segmentos y respawneando ffmpeg. La UI pinta el StremioLoader
  // mientras dura para que el user vea que "está cambiando", en vez
  // de un playback frozen sin explicación.
  const [audioSwitching, setAudioSwitching] = useState(false)

  // Cambio de pista de audio → POST /hls/audio + respawn ffmpeg +
  // destroy&mount hls.js (extraído a `useAudioSwitch`).
  const {
    activeAudioIdx,
    postAudioSwitchSeekRef,
    switchAudioTrack,
  } = useAudioSwitch({
    videoRef,
    stream,
    setAudioSwitching,
    setBuffering,
  })

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
    bumpVolumeHud,
    setVolume,
    setMuted,
    setSubsPanelOpen,
    setSubOffset,
    setSubSpeed,
    setIsFullscreen,
    handleBack,
    stream,
  })

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
  // Attach del `<video>` src + hls.js: `useHlsAttach` decide entre
  // DIRECT (`stream.url` raw) y HLS transmux (`hls.js` cuando no
  // hay soporte nativo). Devuelve el `videoSrc` para el JSX y la
  // resolución final de `canGoDirect` (que también aplica el
  // fallback runtime para HEVC en WebView2 sin la extensión).
  const { videoSrc, canGoDirect: directOk } = useHlsAttach({
    videoRef,
    stream,
    media,
    directFailed,
    activeAudioIdx,
    torrentsRoute,
    setError,
    setErrorBackTo,
    t,
  })

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
            // Pre-cablear el grafo Web Audio ANTES de que el
            // decoder de audio arranque. Si lo hacemos más tarde
            // (p.ej. al primer ArrowUp), WKWebView resetea el
            // pipeline en caliente → ~1s de silencio + micro-pause.
            // Aquí el element ya tiene metadata pero aún no ha
            // decodificado audio, así que el swap es transparente.
            primeAudio()
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
            if (code === 4 && directOk && !directFailed) {
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

      <PlayerLoader
        error={error}
        stream={stream}
        hasStartedPlayback={hasStartedPlayback}
        seeking={seeking}
        audioSwitching={audioSwitching}
        buffering={buffering}
        stalledLong={stalledLong}
        title={state.title}
        backdropUrl={backdropUrl}
        logoUrl={logoUrl}
        stats={stats}
      />

      <SyncHud text={syncHud} />
      <VolumeHud value={volumeHud} />

      <ErrorOverlay error={error} onBack={handleBack} />

      <SubDragOverlay active={dragActive} />
      <SubDragFlash active={dragFlash} />
      <SubDragErrorToast message={dragError} />

      <PlayerTopBar
        title={state.title}
        subRelease={state.subRelease}
        isSeries={!!state.isSeries}
        season={state.season ?? null}
        episode={state.episode ?? null}
        tmdbId={state.tmdbId ?? null}
        duration={duration}
        currentTime={currentTime}
        controlsVisible={controlsVisible}
        onBack={handleBack}
        onNextEpisode={(nextEp) => {
          // §6 audit: "siguiente episodio" — dispara una navegación
          // al Torrents/series con E+1. La ruta reutilizará la caché
          // de sesión torrent del pack si es el mismo magnet, así
          // que la transición es rápida.
          void reportPositionNow().finally(() => {
            nav(
              `/torrents/series/${state.tmdbId}?season=${state.season}&episode=${nextEp}`,
              { replace: true },
            )
          })
        }}
      />

      <PlayerControlBar
        videoRef={videoRef}
        currentTime={currentTime}
        duration={duration}
        paused={paused}
        volume={volume}
        muted={muted}
        isFullscreen={isFullscreen}
        controlsVisible={controlsVisible}
        seekHover={seekHover}
        setSeekHover={setSeekHover}
        onSeek={seekTo}
        onTogglePlay={togglePlay}
        onSetVolume={setVolume}
        onToggleMute={() => {
          setMuted((m) => !m)
          bumpVolumeHud()
        }}
        onToggleFullscreen={toggleFullscreen}
        statsPanelOpen={statsPanelOpen}
        setStatsPanelOpen={setStatsPanelOpen}
        audioTracks={audioTracks}
        activeAudioIdx={activeAudioIdx}
        audioPanelOpen={audioPanelOpen}
        setAudioPanelOpen={setAudioPanelOpen}
        activeSub={activeSub}
        setSubsPanelOpen={setSubsPanelOpen}
      />

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
