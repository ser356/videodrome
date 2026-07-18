/**
 * Canonical dictionary (English). All other locales are validated
 * against this shape — missing keys cascade to `en` in `t()`.
 *
 * Convention: `namespace.key.dotted`, values as short sentence
 * fragments. Interpolation with `{{var}}`.
 */
export const en: Record<string, string> = {
  // ── Common ────────────────────────────────────────────
  'common.back': 'Back',
  'common.close': 'Close',
  'common.cancel': 'Cancel',
  'common.save': 'Save',
  'common.loading': 'Loading…',
  'common.retry': 'Retry',
  'common.play': 'Play',

  // ── Nav ───────────────────────────────────────────────
  'nav.home': 'Home',
  'nav.recs': 'Recommendations',
  'nav.search': 'Search',
  'nav.settings': 'Settings',
  'nav.session': 'Session',
  'nav.logout': 'Log out',

  // ── Hotkey bar ────────────────────────────────────────
  'hotkey.move': 'Move',
  'hotkey.play': 'Play',
  'hotkey.magnet': 'Magnet',
  'hotkey.panel': 'Panel',
  'hotkey.back': 'Back',
  'hotkey.torrents': 'Torrents',
  'hotkey.episode': 'Episode',
  'hotkey.season': 'Season',
  'hotkey.seasonPack': 'Season pack',
  'hotkey.dismiss': 'Dismiss',

  // ── Search ────────────────────────────────────────────
  'search.title': 'Search torrents',
  'search.hint': 'Type the title. Add the year at the end to disambiguate remakes (e.g., "Funny Games 2007").',
  'search.placeholder': 'Title…',
  'search.submit': 'Search',

  // ── SearchResults ─────────────────────────────────────
  'searchResults.title': 'Results',
  'searchResults.matches': '{{n}} matches',
  'searchResults.searching': 'Searching…',
  'searchResults.emptyTitle': 'Nothing with available torrents.',
  'searchResults.emptyHint': 'TMDB returned no matches, or no indexer has torrents with seeders for those it did. Try the English original title or add the year.',
  'searchResults.badgeSeries': 'SERIES',

  // ── Torrents ──────────────────────────────────────────
  'torrents.title': 'Torrents',
  'torrents.results': '{{n}} results',
  'torrents.searching': 'Searching…',
  'torrents.col.release': 'Release',
  'torrents.col.size': 'Size',
  'torrents.col.seeds': 'Seeds',
  'torrents.col.leech': 'Leech',
  'torrents.col.quality': 'Quality',
  'torrents.col.audio': 'Audio',
  'torrents.col.source': 'Source',
  'torrents.hint': 'Press Enter to play the selected torrent. Subtitles are chosen from the player itself. S sends the magnet to your default BitTorrent client.',
  'torrents.matchKind.ep': 'EP',
  'torrents.matchKind.pack': 'PACK',
  'torrents.matchKind.series': 'SERIES',
  'torrents.chipTitle': 'You’ll play this episode from inside the pack',
  'torrents.stream.started': 'Streaming {{name}}',
  'torrents.stream.starting': 'Starting stream: {{name}}…',
  'torrents.stream.stopped': 'Stream stopped.',
  'torrents.stream.playerDied': 'Stream stopped: VLC closed.',
  'torrents.stream.error': 'Stream error: {{err}}',
  'torrents.stream.vlcFallback': 'Embedded playback unavailable. Opening in VLC…',
  'torrents.magnet.sent': 'Magnet sent to default client: {{name}}',
  'torrents.magnet.copied': 'Magnet copied to clipboard.',
  'torrents.magnet.error': 'Error opening magnet: {{err}}',
  'torrents.magnet.copyError': 'Could not copy magnet: {{err}}',
  'torrents.menu.playHtml': 'Play in embedded player',
  'torrents.menu.playVlc': 'Play in VLC',
  'torrents.menu.playVlcOnce': 'Open in VLC (this torrent)',
  'torrents.menu.openClient': 'Open in torrent client',
  'torrents.menu.copyMagnet': 'Copy magnet',

  // ── Series detail ─────────────────────────────────────
  'series.badge': 'Series',
  'series.seasonsCount': '{{n}} seasons',
  'series.seasonCount1': '1 season',
  'series.loading': 'Loading series…',
  'series.loadingEpisodes': 'Loading episodes…',
  'series.noEpisodes': 'No episodes listed for this season.',
  'series.season': 'Season {{n}}',
  'series.searchPack': 'Search season pack',
  'series.episodeShort': 'Episode {{n}}',
  'series.noStill': 'no still',
  'series.min': 'min',
  'series.invalidId': 'Invalid tmdbId.',
  'series.tauriRequired': 'This view requires the desktop app (Tauri).',

  // ── Player ────────────────────────────────────────────
  'player.subs': 'Subs',
  'player.nextEpisode': 'Next episode →',
  'player.nextEpisodeTitle': 'Next episode',
  'player.noMagnet': 'No magnet. Go back to the list and play a torrent.',
  'player.startError': 'Could not start stream: {{err}}',
  'player.backTitle': 'Back (Esc)',

  // ── Settings ──────────────────────────────────────────
  'settings.title': 'Settings',
  'settings.saved': 'Preferences saved.',
  'settings.ui.section': 'Interface',
  'settings.ui.language': 'Language',
  'settings.ui.languageHint': 'Interface language. Also used as the top-priority subtitle language when searching.',
  'settings.subs.section': 'Subtitles',
  'settings.subs.languages': 'Subtitle languages',
  'settings.subs.languagesHint': 'ISO 639-1 codes separated by commas (e.g., "es,en,fr"). The interface language always goes first.',
  'settings.player.section': 'Player',
  'settings.player.default': 'Default player',
  'settings.player.html': 'Embedded (HTML)',
  'settings.player.vlc': 'External (VLC)',
  'settings.recs.section': 'Recommendations',
  'settings.recs.minRating': 'Default minimum rating',
  'settings.recs.minRatingHint': 'Starting threshold for the Recommendations view (0.5 – 5.0).',
  'settings.cache.section': 'Cache',
  'settings.cache.clear': 'Clear',
  'settings.cache.clearAll': 'Clear all',
  'settings.cache.streamTtl': 'Stream cache TTL (days)',
  'settings.glass.section': 'Appearance',
  'settings.glass.opacity': 'Glass opacity',

  // ── Login ─────────────────────────────────────────────
  'login.title': 'Log in',
  'login.username': 'Username',
  'login.password': 'Password',
  'login.submit': 'Log in',
  'login.errorGeneric': 'Log in failed.',

  // ── Resume dialog ─────────────────────────────────────
  'resume.title': 'Resume playback',
  'resume.at': 'You were at {{time}}',
  'resume.resume': 'Resume',
  'resume.restart': 'Start over',

  // ── Home / Recs bits ──────────────────────────────────
  'home.dismiss': 'Do not suggest',
  'home.dismissedTitle': 'Dismissed suggestions',
  'home.restore': 'Restore',
}
