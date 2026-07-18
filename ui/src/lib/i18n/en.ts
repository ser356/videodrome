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
  'home.headline': 'What are we watching today?',
  'home.subhead': 'Pick one of the options or press Enter on the highlighted one.',
  'home.sessionActive': 'Session active',
  'home.up': 'Up',
  'home.down': 'Down',
  'home.select': 'Select',
  'home.optionRecsLabel': 'Recommendations from Letterboxd',
  'home.optionRecsHint': 'Generate and browse suggested films based on your history.',
  'home.optionSearchLabel': 'Search torrents directly',
  'home.optionSearchHint': 'Type a title and search torrents without going through Letterboxd.',

  // ── HotkeyBar tooltip ────────────────────────────────
  'hotkey.shortcutTitle': 'Shortcut: {{key}}',

  // ── StreamPanel ──────────────────────────────────────
  'streamPanel.streaming': 'Streaming',
  'streamPanel.stop': 'Stop',
  'streamPanel.hintPre': 'Press',
  'streamPanel.hintMid': 'to play the selected torrent. Subtitles are chosen from the player itself.',
  'streamPanel.hintPost': 'sends the magnet to your default BitTorrent client.',

  // ── Login extra ──────────────────────────────────────
  'login.hint': 'Credentials stay local; they never leave your machine.',
  'login.onlyDesktop': 'This window only works inside the desktop app.',
  'login.verifying': 'Verifying…',

  // ── Resume extras ────────────────────────────────────
  'resume.eyebrow': 'You already watched part of this',
  'resume.question': 'Resume where you left off?',
  'resume.progress': 'Saved progress',
  'resume.jumpTo': 'Jump to {{time}}',
  'resume.ignorePrevious': 'Ignore previous progress',
  'resume.confirm': 'confirm',

  // ── Recommendations ──────────────────────────────────
  'recs.title': 'Now playing',
  'recs.reload': 'Reload',
  'recs.detail': 'Detail',
  'recs.emptyTitle': 'No results.',
  'recs.emptyHint': 'Lower the minimum rating or check your Letterboxd history.',
  'recs.endOfList': 'End of the list. {{n}} recommendations.',
  'recs.dismissError': 'Error dismissing: {{err}}',
  'recs.dismissedFlash': 'Dismissed: {{title}}. Restore from Settings.',
  'recs.menu.detail': 'View detail',
  'recs.menu.torrents': 'View torrents',

  // ── Movie detail modal ───────────────────────────────
  'movieDetail.noOverview': 'No synopsis available.',
  'movieDetail.viewTorrents': 'View torrents',

  // ── Search box ───────────────────────────────────────
  'search.boxPlaceholder': 'Search movie…',

  // ── Time (relative, short form) ─────────────────────
  'time.secondsShort': '{{n}}s ago',
  'time.minutesShort': '{{n}}min ago',
  'time.hoursShort': '{{n}}h ago',
  'time.daysShort': '{{n}}d ago',

  // ── Settings sub-sections ───────────────────────────
  'settings.session.section': 'Session',
  'settings.session.noSession': 'No session',
  'settings.logoutDone': 'Logged out.',
  'settings.preferences.section': 'Preferences',
  'settings.dismissed.section': 'Dismissed suggestions',
  'settings.dismissed.count': '{{n}} movies',
  'settings.dismissed.count1': '1 movie',
  'settings.dismissed.empty':
    'You have not dismissed any recommendation. Right-click a movie in Now Playing → "Do not suggest".',
  'settings.dismissed.restored': 'Restored: {{title}}',
  'settings.cache.cleared': 'Cache "{{kind}}" cleared.',
  'settings.cache.allCleared': 'All caches cleared.',
  'settings.cache.updatedAgo': 'Updated {{age}}',
  'settings.cache.empty': 'empty',
  'settings.cache.sessionHint': 'The session is not cleared here. Use "Log out" above.',
  'settings.cache.label.log_entries': 'Letterboxd history',
  'settings.cache.label.watchlist': 'Letterboxd watchlist',
  'settings.cache.label.tmdb_recs': 'TMDB recommendations',
  'settings.cache.label.search': 'TMDB + torrents searches',
  'settings.cache.label.torrent_search': 'Torrent results (30 min / 5 min empty)',
  'settings.cache.label.tmdb_search': 'TMDB searches (titles)',
  'settings.cache.label.tmdb_view': 'TMDB details (modal)',
  'settings.cache.label.tmdb_details': 'TMDB details (torrents)',
  'settings.cache.label.streams': 'Streams (BitTorrent pieces)',
  'settings.streamCacheTtlHint':
    'Prune at startup: movies not played in N days are removed from disk. Between 1 and 365.',
  'settings.glass.hint':
    '0 = maximum translucency (default). 100 = nearly solid surfaces, more readable over poster grids.',
  'settings.glass.crystal': 'Crystal',
  'settings.glass.solid': 'Solid',
  'settings.player.hint':
    'Embedded player inside the app or VLC as an external app. Right-click a torrent always offers VLC as an escape hatch even if the default is embedded.',

  // ── Player controls ─────────────────────────────────
  'player.playTitle': 'Play (Space)',
  'player.pauseTitle': 'Pause (Space)',
  'player.stats': 'Stream stats',
  'player.audioTrack': 'Audio track',
  'player.subtitlesTitle': 'Subtitles (C)',
  'player.subtitles': 'Subtitles',
  'player.subtitle': 'Subtitle',
  'player.fullscreenTitle': 'Fullscreen (F)',
  'player.muteTitle': 'Mute (M)',
  'player.unmuteTitle': 'Unmute (M)',
  'player.available1': '{{n}} available',
  'player.availableN': '{{n}} available',
  'player.langUnknown': 'Unknown language',
  'player.active': 'Active',
  'player.trackN': 'Track {{n}}',
  'player.removeCurrent': 'Remove current',
  'player.embedded': 'From file',
  'player.noSubs': 'No subtitles available.',
  'player.noSubsHint':
    'OpenSubtitles has no results for this title and the container has no embedded subs.',
  'player.downloads': '{{n}} downloads',
  'player.trustedTitle': 'Verified by OpenSubtitles moderator',
  'player.sdhTitle': 'Transcription for deaf and hard of hearing',
  'player.waitingData': 'Waiting for data…',
  'player.stat.speed': 'Speed',
  'player.stat.peers': 'Peers',
  'player.stat.progress': 'Progress',
  'player.stat.downloaded': 'Downloaded',
  'player.hlsUnsupported': 'Your browser/webview does not support HLS. Switch the player to VLC in Settings.',
  'player.hlsFatal': 'HLS failure ({{type}}/{{details}}). Try switching the player to VLC in Settings.',
  'player.swarmStalled':
    'The torrent is downloading at {{speed}} with {{peers}} peers ({{pct}}% complete). The swarm cannot sustain enough bandwidth for playback. Try another release with more seeders, or open the link in VLC from Settings.',
  'player.probeStalled':
    'This torrent is not downloading (0 B in {{elapsed}} s), it probably has no active seeders. Try another release from the list.',
  'player.videoFailed': 'Could not play this movie in the embedded player. Try switching to VLC from Settings.',
  'player.ffmpegMissing':
    'ffmpeg is not available. {{hint}} Alternative: switch the player to VLC in Settings.',
  'player.probeFailed':
    'Could not analyze the stream: {{err}}. Check that ffmpeg is installed, or switch the player to VLC in Settings.',
  'player.ffmpegHintWindows': 'Install it with `winget install Gyan.FFmpeg` (or `scoop install ffmpeg`).',
  'player.ffmpegHintMac': 'Install it with `brew install ffmpeg`.',
  'player.ffmpegHintLinux':
    'Install it via your distro package manager (`sudo apt install ffmpeg`, `sudo dnf install ffmpeg`, `sudo pacman -S ffmpeg`).',
  'player.ffmpegHintGeneric': 'Install ffmpeg and make sure it is in the PATH.',
}
