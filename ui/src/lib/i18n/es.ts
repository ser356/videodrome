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
}
