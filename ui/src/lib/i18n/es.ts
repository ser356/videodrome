/**
 * Diccionario español (nativo del user). Traducción completa de las
 * claves canónicas de `en.ts`. Cualquier clave ausente cae al inglés
 * en `t()`.
 */
export const es: Record<string, string> = {
  // ── Common ────────────────────────────────────────────
  'common.back': 'Volver',
  'common.close': 'Cerrar',
  'common.cancel': 'Cancelar',
  'common.save': 'Guardar',
  'common.loading': 'Cargando…',
  'common.retry': 'Reintentar',
  'common.play': 'Proyectar',

  // ── Nav ───────────────────────────────────────────────
  'nav.home': 'Inicio',
  'nav.recs': 'Recomendaciones',
  'nav.search': 'Buscar',
  'nav.settings': 'Ajustes',
  'nav.session': 'Sesión',
  'nav.logout': 'Cerrar sesión',

  // ── Hotkey bar ────────────────────────────────────────
  'hotkey.move': 'Mover',
  'hotkey.play': 'Proyectar',
  'hotkey.magnet': 'Magnet',
  'hotkey.panel': 'Panel',
  'hotkey.back': 'Volver',
  'hotkey.torrents': 'Torrents',
  'hotkey.episode': 'Episodio',
  'hotkey.season': 'Temporada',
  'hotkey.seasonPack': 'Pack temporada',
  'hotkey.dismiss': 'Descartar',

  // ── Search ────────────────────────────────────────────
  'search.title': 'Buscar torrents',
  'search.hint': 'Escribe el título. Añade el año al final para desambiguar remakes (por ejemplo, "Funny Games 2007").',
  'search.placeholder': 'Título…',
  'search.submit': 'Buscar',

  // ── SearchResults ─────────────────────────────────────
  'searchResults.title': 'Resultados',
  'searchResults.matches': '{{n}} coincidencias',
  'searchResults.searching': 'Buscando…',
  'searchResults.emptyTitle': 'Nada con torrents disponibles.',
  'searchResults.emptyHint': 'TMDB no devolvió coincidencias, o ningún indexador tiene torrents con seeders para las que devolvió. Prueba el título original en inglés o añade el año.',
  'searchResults.badgeSeries': 'SERIE',

  // ── Torrents ──────────────────────────────────────────
  'torrents.title': 'Torrents',
  'torrents.results': '{{n}} resultados',
  'torrents.searching': 'Buscando…',
  'torrents.col.release': 'Release',
  'torrents.col.size': 'Tamaño',
  'torrents.col.seeds': 'Seeds',
  'torrents.col.leech': 'Leech',
  'torrents.col.quality': 'Calidad',
  'torrents.col.audio': 'Audio',
  'torrents.col.source': 'Fuente',
  'torrents.hint': 'Pulsa Enter para proyectar el torrent seleccionado. Los subtítulos se eligen desde el propio reproductor. S envía el magnet a tu cliente BitTorrent por defecto.',
  'torrents.matchKind.ep': 'EP',
  'torrents.matchKind.pack': 'PACK',
  'torrents.matchKind.series': 'SERIE',
  'torrents.chipTitle': 'Proyectarás este episodio dentro del pack',
  'torrents.stream.started': 'Streaming {{name}}',
  'torrents.stream.starting': 'Iniciando stream: {{name}}…',
  'torrents.stream.stopped': 'Stream detenido.',
  'torrents.stream.playerDied': 'Stream detenido: VLC cerrado.',
  'torrents.stream.error': 'Error stream: {{err}}',
  'torrents.stream.vlcFallback': 'Reproducción embebida no disponible. Abriendo con VLC…',
  'torrents.magnet.sent': 'Magnet enviado al cliente por defecto: {{name}}',
  'torrents.magnet.copied': 'Magnet copiado al portapapeles.',
  'torrents.magnet.error': 'Error abriendo magnet: {{err}}',
  'torrents.magnet.copyError': 'No se pudo copiar el magnet: {{err}}',
  'torrents.menu.playHtml': 'Proyectar en player',
  'torrents.menu.playVlc': 'Proyectar en VLC',
  'torrents.menu.playVlcOnce': 'Abrir en VLC (este torrent)',
  'torrents.menu.openClient': 'Abrir en cliente de torrents',
  'torrents.menu.copyMagnet': 'Copiar magnet',

  // ── Series detail ─────────────────────────────────────
  'series.badge': 'Serie',
  'series.seasonsCount': '{{n}} temporadas',
  'series.seasonCount1': '1 temporada',
  'series.loading': 'Cargando serie…',
  'series.loadingEpisodes': 'Cargando episodios…',
  'series.noEpisodes': 'Sin episodios listados para esta temporada.',
  'series.season': 'Temporada {{n}}',
  'series.searchPack': 'Buscar pack de temporada',
  'series.episodeShort': 'Episodio {{n}}',
  'series.noStill': 'sin still',
  'series.min': 'min',
  'series.invalidId': 'tmdbId inválido.',
  'series.tauriRequired': 'Esta vista requiere la app de escritorio (Tauri).',

  // ── Player ────────────────────────────────────────────
  'player.subs': 'Subs',
  'player.nextEpisode': 'Siguiente episodio →',
  'player.nextEpisodeTitle': 'Siguiente episodio',
  'player.noMagnet': 'Sin magnet. Vuelve a la lista y proyecta un torrent.',
  'player.startError': 'No se pudo arrancar el stream: {{err}}',
  'player.backTitle': 'Volver (Esc)',

  // ── Settings ──────────────────────────────────────────
  'settings.title': 'Ajustes',
  'settings.saved': 'Preferencias guardadas.',
  'settings.ui.section': 'Interfaz',
  'settings.ui.language': 'Idioma',
  'settings.ui.languageHint': 'Idioma de la interfaz. También se usa como primer idioma al buscar subtítulos.',
  'settings.subs.section': 'Subtítulos',
  'settings.subs.languages': 'Idiomas de subtítulos',
  'settings.subs.languagesHint': 'Códigos ISO 639-1 separados por comas (ej. "es,en,fr"). El idioma de la interfaz siempre va primero.',
  'settings.player.section': 'Reproductor',
  'settings.player.default': 'Reproductor por defecto',
  'settings.player.html': 'Embebido (HTML)',
  'settings.player.vlc': 'Externo (VLC)',
  'settings.recs.section': 'Recomendaciones',
  'settings.recs.minRating': 'Rating mínimo por defecto',
  'settings.recs.minRatingHint': 'Umbral inicial de la vista Cartelera (0.5 – 5.0).',
  'settings.cache.section': 'Caché',
  'settings.cache.clear': 'Limpiar',
  'settings.cache.clearAll': 'Limpiar todo',
  'settings.cache.streamTtl': 'TTL caché de streams (días)',
  'settings.glass.section': 'Apariencia',
  'settings.glass.opacity': 'Opacidad del vidrio',

  // ── Login ─────────────────────────────────────────────
  'login.title': 'Iniciar sesión',
  'login.username': 'Usuario',
  'login.password': 'Contraseña',
  'login.submit': 'Iniciar sesión',
  'login.errorGeneric': 'Error al iniciar sesión.',

  // ── Resume dialog ─────────────────────────────────────
  'resume.title': 'Reanudar reproducción',
  'resume.at': 'Estabas en {{time}}',
  'resume.resume': 'Reanudar',
  'resume.restart': 'Empezar de cero',

  // ── Home / Recs ───────────────────────────────────────
  'home.dismiss': 'No sugerir',
  'home.dismissedTitle': 'Sugerencias descartadas',
  'home.restore': 'Restaurar',
  'home.headline': '¿Qué hacemos hoy?',
  'home.subhead': 'Elige una de las opciones o pulsa Enter sobre la resaltada.',
  'home.sessionActive': 'Sesión activa',
  'home.up': 'Subir',
  'home.down': 'Bajar',
  'home.select': 'Seleccionar',
  'home.optionRecsLabel': 'Recomendaciones desde Letterboxd',
  'home.optionRecsHint': 'Genera y navega por películas recomendadas basadas en tu historial.',
  'home.optionSearchLabel': 'Buscar torrents directamente',
  'home.optionSearchHint': 'Escribe un título y busca torrents sin pasar por Letterboxd.',

  // ── HotkeyBar tooltip ────────────────────────────────
  'hotkey.shortcutTitle': 'Atajo: {{key}}',

  // ── StreamPanel ──────────────────────────────────────
  'streamPanel.streaming': 'Streaming',
  'streamPanel.stop': 'Detener',
  'streamPanel.hintPre': 'Pulsa',
  'streamPanel.hintMid': 'para proyectar el torrent seleccionado. Los subtítulos se eligen desde el propio reproductor.',
  'streamPanel.hintPost': 'envía el magnet a tu cliente BitTorrent por defecto.',

  // ── Login extras ─────────────────────────────────────
  'login.hint': 'Las credenciales se guardan solo en local; nunca salen de tu máquina.',
  'login.onlyDesktop': 'Esta ventana solo funciona dentro de la app de escritorio.',
  'login.verifying': 'Verificando…',

  // ── Resume extras ────────────────────────────────────
  'resume.eyebrow': 'Ya viste parte de esta peli',
  'resume.question': '¿Reanudar donde lo dejaste?',
  'resume.progress': 'Progreso guardado',
  'resume.jumpTo': 'Salta a {{time}}',
  'resume.ignorePrevious': 'Ignorar el progreso anterior',
  'resume.confirm': 'confirmar',

  // ── Recommendations ──────────────────────────────────
  'recs.title': 'Cartelera',
  'recs.reload': 'Recargar',
  'recs.detail': 'Detalle',
  'recs.emptyTitle': 'Sin resultados.',
  'recs.emptyHint': 'Baja el rating mínimo o comprueba tu historial en Letterboxd.',
  'recs.endOfList': 'Fin de la cartelera. {{n}} recomendaciones.',
  'recs.dismissError': 'Error al descartar: {{err}}',
  'recs.dismissedFlash': 'Descartada: {{title}}. Restaurar desde Ajustes.',
  'recs.menu.detail': 'Ver detalle',
  'recs.menu.torrents': 'Ver torrents',

  // ── Movie detail modal ───────────────────────────────
  'movieDetail.noOverview': 'Sin sinopsis disponible.',
  'movieDetail.viewTorrents': 'Ver torrents',

  // ── Search box ───────────────────────────────────────
  'search.boxPlaceholder': 'Buscar película…',

  // ── Time (relative, short form) ─────────────────────
  'time.secondsShort': 'hace {{n}}s',
  'time.minutesShort': 'hace {{n}}min',
  'time.hoursShort': 'hace {{n}}h',
  'time.daysShort': 'hace {{n}}d',

  // ── Settings sub-sections ───────────────────────────
  'settings.session.section': 'Sesión',
  'settings.session.noSession': 'Sin sesión',
  'settings.logoutDone': 'Sesión cerrada.',
  'settings.preferences.section': 'Preferencias',
  'settings.dismissed.section': 'Sugerencias descartadas',
  'settings.dismissed.count': '{{n}} películas',
  'settings.dismissed.count1': '1 película',
  'settings.dismissed.empty':
    'No has descartado ninguna recomendación. Usa clic derecho sobre una peli en Cartelera → "No sugerir".',
  'settings.dismissed.restored': 'Restaurada: {{title}}',
  'settings.cache.cleared': 'Caché "{{kind}}" borrada.',
  'settings.cache.allCleared': 'Todas las cachés borradas.',
  'settings.cache.updatedAgo': 'Actualizada {{age}}',
  'settings.cache.empty': 'vacía',
  'settings.cache.sessionHint': 'La sesión no se borra desde aquí. Usa "Cerrar sesión" arriba.',
  'settings.cache.label.log_entries': 'Historial Letterboxd',
  'settings.cache.label.watchlist': 'Watchlist Letterboxd',
  'settings.cache.label.tmdb_recs': 'Recomendaciones TMDB',
  'settings.cache.label.search': 'Búsquedas TMDB + torrents',
  'settings.cache.label.torrent_search': 'Resultados de torrents (30 min / 5 min vacío)',
  'settings.cache.label.tmdb_search': 'Búsquedas TMDB (títulos)',
  'settings.cache.label.tmdb_view': 'Detalles TMDB (modal)',
  'settings.cache.label.tmdb_details': 'Detalles TMDB (torrents)',
  'settings.cache.label.streams': 'Streams (piezas de BitTorrent)',
  'settings.streamCacheTtlHint':
    'Purga al arrancar: pelis no reproducidas en N días se borran del disco. Entre 1 y 365.',
  'settings.glass.hint':
    '0 = translúcido máximo (default). 100 = superficies casi sólidas, más legibles sobre grids de pósters.',
  'settings.glass.crystal': 'Cristal',
  'settings.glass.solid': 'Sólido',
  'settings.player.hint':
    'Player embebido dentro de la app o VLC como app externa. El clic derecho sobre un torrent siempre ofrece VLC como escape hatch aunque el default sea embebido.',

  // ── Player controls ─────────────────────────────────
  'player.playTitle': 'Play (Espacio)',
  'player.pauseTitle': 'Pausa (Espacio)',
  'player.stats': 'Estadísticas del stream',
  'player.audioTrack': 'Pista de audio',
  'player.subtitlesTitle': 'Subtítulos (C)',
  'player.subtitles': 'Subtítulos',
  'player.subtitle': 'Subtítulo',
  'player.fullscreenTitle': 'Pantalla completa (F)',
  'player.muteTitle': 'Silenciar (M)',
  'player.unmuteTitle': 'Reactivar audio (M)',
  'player.available1': '{{n}} disponible',
  'player.availableN': '{{n}} disponibles',
  'player.langUnknown': 'Idioma desconocido',
  'player.active': 'Activo',
  'player.trackN': 'Pista {{n}}',
  'player.removeCurrent': 'Quitar el actual',
  'player.embedded': 'Del fichero',
  'player.noSubs': 'Sin subtítulos disponibles.',
  'player.noSubsHint':
    'OpenSubtitles no tiene resultados para este título y el contenedor no lleva subs embebidos.',
  'player.downloads': '{{n}} descargas',
  'player.trustedTitle': 'Verificado por moderador de OpenSubtitles',
  'player.sdhTitle': 'Transcripción para sordos',
  'player.waitingData': 'Esperando datos…',
  'player.stat.speed': 'Velocidad',
  'player.stat.peers': 'Peers',
  'player.stat.progress': 'Progreso',
  'player.stat.downloaded': 'Descargado',
  'player.hlsUnsupported': 'Tu navegador/webview no soporta HLS. Cambia el reproductor a VLC en Ajustes.',
  'player.hlsFatal': 'Fallo de HLS ({{type}}/{{details}}). Prueba a cambiar el reproductor a VLC en Ajustes.',
  'player.swarmStalled':
    'El torrent está descargando a {{speed}} con {{peers}} peers ({{pct}}% completo). El enjambre no da suficiente ancho de banda para reproducir. Prueba otro release con más seeders, o abre el enlace en VLC desde Ajustes.',
  'player.videoFailed': 'No se pudo reproducir esta película en el player. Prueba a cambiar el reproductor a VLC desde Ajustes.',
  'player.ffmpegMissing':
    'ffmpeg no está disponible. {{hint}} Alternativa: cambia el reproductor a VLC en Ajustes.',
  'player.probeFailed':
    'No se pudo analizar el stream: {{err}}. Comprueba que ffmpeg está instalado, o cambia el reproductor a VLC en Ajustes.',
  'player.ffmpegHintWindows': 'Instálalo con `winget install Gyan.FFmpeg` (o `scoop install ffmpeg`).',
  'player.ffmpegHintMac': 'Instálalo con `brew install ffmpeg`.',
  'player.ffmpegHintLinux':
    'Instálalo con el gestor de paquetes de tu distro (`sudo apt install ffmpeg`, `sudo dnf install ffmpeg`, `sudo pacman -S ffmpeg`).',
  'player.ffmpegHintGeneric': 'Instala ffmpeg y asegúrate de que esté en el PATH.',
}
