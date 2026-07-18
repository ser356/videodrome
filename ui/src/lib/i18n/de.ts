/**
 * Deutsches Wörterbuch. Fehlende Schlüssel fallen in `t()` auf Englisch zurück.
 */
export const de: Record<string, string> = {
  // ── Common ────────────────────────────────────────────
  'common.back': 'Zurück',
  'common.close': 'Schließen',
  'common.cancel': 'Abbrechen',
  'common.save': 'Speichern',
  'common.loading': 'Lädt…',
  'common.retry': 'Erneut versuchen',
  'common.play': 'Abspielen',

  // ── Nav ───────────────────────────────────────────────
  'nav.home': 'Start',
  'nav.recs': 'Empfehlungen',
  'nav.search': 'Suchen',
  'nav.settings': 'Einstellungen',
  'nav.session': 'Sitzung',
  'nav.logout': 'Abmelden',

  // ── Hotkey bar ────────────────────────────────────────
  'hotkey.move': 'Navigieren',
  'hotkey.play': 'Abspielen',
  'hotkey.magnet': 'Magnet',
  'hotkey.panel': 'Panel',
  'hotkey.back': 'Zurück',
  'hotkey.torrents': 'Torrents',
  'hotkey.episode': 'Folge',
  'hotkey.season': 'Staffel',
  'hotkey.seasonPack': 'Staffel-Pack',
  'hotkey.dismiss': 'Ausblenden',

  // ── Search ────────────────────────────────────────────
  'search.title': 'Torrents suchen',
  'search.hint': 'Titel eingeben. Jahr am Ende hinzufügen, um Remakes zu unterscheiden (z. B. „Funny Games 2007“).',
  'search.placeholder': 'Titel…',
  'search.submit': 'Suchen',

  // ── SearchResults ─────────────────────────────────────
  'searchResults.title': 'Ergebnisse',
  'searchResults.matches': '{{n}} Treffer',
  'searchResults.searching': 'Suche…',
  'searchResults.emptyTitle': 'Nichts mit verfügbaren Torrents.',
  'searchResults.emptyHint': 'TMDB lieferte keine Treffer, oder kein Indexer hat Torrents mit Seedern. Versuche den englischen Originaltitel oder gib das Jahr an.',
  'searchResults.badgeSeries': 'SERIE',

  // ── Torrents ──────────────────────────────────────────
  'torrents.title': 'Torrents',
  'torrents.results': '{{n}} Ergebnisse',
  'torrents.searching': 'Suche…',
  'torrents.col.release': 'Release',
  'torrents.col.size': 'Größe',
  'torrents.col.seeds': 'Seeds',
  'torrents.col.leech': 'Leech',
  'torrents.col.quality': 'Qualität',
  'torrents.col.audio': 'Audio',
  'torrents.col.source': 'Quelle',
  'torrents.hint': 'Enter, um den gewählten Torrent abzuspielen. Untertitel werden im Player gewählt. S sendet den Magnet an deinen Standard-BitTorrent-Client.',
  'torrents.matchKind.ep': 'FOLGE',
  'torrents.matchKind.pack': 'PACK',
  'torrents.matchKind.series': 'SERIE',
  'torrents.chipTitle': 'Diese Folge wird aus dem Pack abgespielt',
  'torrents.menu.playHtml': 'Im Player abspielen',
  'torrents.menu.playVlc': 'In VLC abspielen',
  'torrents.menu.playVlcOnce': 'In VLC öffnen (dieser Torrent)',
  'torrents.menu.openClient': 'Im Torrent-Client öffnen',
  'torrents.menu.copyMagnet': 'Magnet kopieren',

  // ── Series detail ─────────────────────────────────────
  'series.badge': 'Serie',
  'series.seasonsCount': '{{n}} Staffeln',
  'series.seasonCount1': '1 Staffel',
  'series.loading': 'Serie wird geladen…',
  'series.loadingEpisodes': 'Folgen werden geladen…',
  'series.noEpisodes': 'Keine Folgen für diese Staffel gelistet.',
  'series.season': 'Staffel {{n}}',
  'series.searchPack': 'Staffel-Pack suchen',
  'series.episodeShort': 'Folge {{n}}',
  'series.noStill': 'kein Bild',
  'series.min': 'Min.',

  // ── Player ────────────────────────────────────────────
  'player.subs': 'Untertitel',
  'player.nextEpisode': 'Nächste Folge →',
  'player.nextEpisodeTitle': 'Nächste Folge',
  'player.backTitle': 'Zurück (Esc)',

  // ── Settings ──────────────────────────────────────────
  'settings.title': 'Einstellungen',
  'settings.ui.section': 'Oberfläche',
  'settings.ui.language': 'Sprache',
  'settings.ui.languageHint': 'Sprache der Oberfläche. Wird auch als bevorzugte Untertitelsprache bei der Suche verwendet.',
  'settings.subs.section': 'Untertitel',
  'settings.subs.languages': 'Untertitel-Sprachen',
  'settings.subs.languagesHint': 'ISO-639-1-Codes durch Komma getrennt (z. B. „de,en“). Die Oberflächensprache steht immer zuerst.',
  'settings.player.section': 'Player',
  'settings.player.default': 'Standard-Player',
  'settings.player.html': 'Eingebettet (HTML)',
  'settings.player.vlc': 'Extern (VLC)',
  'settings.recs.section': 'Empfehlungen',
  'settings.recs.minRating': 'Standard-Mindestbewertung',
  'settings.cache.section': 'Cache',
  'settings.cache.clear': 'Leeren',
  'settings.cache.clearAll': 'Alles leeren',
  'settings.glass.section': 'Erscheinungsbild',
  'settings.glass.opacity': 'Glasdurchsicht',

  // ── Resume dialog ─────────────────────────────────────
  'resume.title': 'Wiedergabe fortsetzen',
  'resume.at': 'Du warst bei {{time}}',
  'resume.resume': 'Fortsetzen',
  'resume.restart': 'Von vorn beginnen',
  'resume.eyebrow': 'Du hast schon einen Teil gesehen',
  'resume.question': 'An der letzten Stelle fortsetzen?',
  'resume.progress': 'Gespeicherter Fortschritt',
  'resume.jumpTo': 'Zu {{time}} springen',
  'resume.ignorePrevious': 'Vorherigen Fortschritt ignorieren',
  'resume.confirm': 'bestätigen',

  // ── Home / Recs ───────────────────────────────────────
  'home.headline': 'Was schauen wir heute?',
  'home.subhead': 'Wähle eine Option oder drücke Enter auf der hervorgehobenen.',
  'home.sessionActive': 'Sitzung aktiv',
  'home.up': 'Hoch',
  'home.down': 'Runter',
  'home.select': 'Auswählen',
  'home.optionRecsLabel': 'Empfehlungen aus Letterboxd',
  'home.optionRecsHint': 'Filme auf Basis deiner Historie generieren und durchsuchen.',
  'home.optionSearchLabel': 'Torrents direkt suchen',
  'home.optionSearchHint': 'Titel eingeben und Torrents ohne Letterboxd suchen.',

  // ── HotkeyBar tooltip ────────────────────────────────
  'hotkey.shortcutTitle': 'Kurzbefehl: {{key}}',

  // ── StreamPanel ──────────────────────────────────────
  'streamPanel.streaming': 'Wiedergabe',
  'streamPanel.stop': 'Stopp',
  'streamPanel.hintPre': 'Drücke',
  'streamPanel.hintMid': 'um den ausgewählten Torrent abzuspielen. Untertitel werden im Player gewählt.',
  'streamPanel.hintPost': 'sendet den Magnet an deinen Standard-BitTorrent-Client.',

  // ── Login extras ─────────────────────────────────────
  'login.title': 'Anmelden',
  'login.username': 'Benutzername',
  'login.password': 'Passwort',
  'login.submit': 'Anmelden',
  'login.hint': 'Zugangsdaten bleiben lokal; sie verlassen deinen Rechner nie.',
  'login.onlyDesktop': 'Dieses Fenster funktioniert nur in der Desktop-App.',
  'login.verifying': 'Überprüfe…',

  // ── Recommendations ──────────────────────────────────
  'recs.title': 'Filmauswahl',
  'recs.reload': 'Neu laden',
  'recs.detail': 'Details',
  'recs.emptyTitle': 'Keine Ergebnisse.',
  'recs.emptyHint': 'Senke die Mindestbewertung oder prüfe deinen Letterboxd-Verlauf.',
  'recs.endOfList': 'Ende der Liste. {{n}} Empfehlungen.',
  'recs.dismissError': 'Fehler beim Ausblenden: {{err}}',
  'recs.dismissedFlash': 'Ausgeblendet: {{title}}. Wiederherstellen unter Einstellungen.',
  'recs.menu.detail': 'Details anzeigen',
  'recs.menu.torrents': 'Torrents anzeigen',

  // ── Movie detail modal ───────────────────────────────
  'movieDetail.noOverview': 'Keine Inhaltsangabe verfügbar.',
  'movieDetail.viewTorrents': 'Torrents anzeigen',

  // ── Search box ───────────────────────────────────────
  'search.boxPlaceholder': 'Film suchen…',

  // ── Time ────────────────────────────────────────────
  'time.secondsShort': 'vor {{n}}s',
  'time.minutesShort': 'vor {{n}}min',
  'time.hoursShort': 'vor {{n}}h',
  'time.daysShort': 'vor {{n}}T',

  // ── Settings extras ─────────────────────────────────
  'settings.session.section': 'Sitzung',
  'settings.session.noSession': 'Keine Sitzung',
  'settings.logoutDone': 'Abgemeldet.',
  'settings.preferences.section': 'Einstellungen',
  'settings.dismissed.section': 'Ausgeblendete Vorschläge',
  'settings.dismissed.count': '{{n}} Filme',
  'settings.dismissed.count1': '1 Film',
  'settings.dismissed.empty':
    'Du hast nichts ausgeblendet. Rechtsklick auf einen Film in „Filmauswahl“ → „Nicht mehr vorschlagen“.',
  'settings.dismissed.restored': 'Wiederhergestellt: {{title}}',
  'settings.cache.cleared': 'Cache „{{kind}}“ geleert.',
  'settings.cache.allCleared': 'Alle Caches geleert.',
  'settings.cache.updatedAgo': 'Aktualisiert {{age}}',
  'settings.cache.empty': 'leer',
  'settings.cache.sessionHint': 'Die Sitzung wird hier nicht geleert. Nutze „Abmelden“ oben.',
  'settings.cache.label.log_entries': 'Letterboxd-Verlauf',
  'settings.cache.label.watchlist': 'Letterboxd-Watchlist',
  'settings.cache.label.tmdb_recs': 'TMDB-Empfehlungen',
  'settings.cache.label.search': 'TMDB + Torrent-Suchen',
  'settings.cache.label.torrent_search': 'Torrent-Ergebnisse (30 min / 5 min leer)',
  'settings.cache.label.tmdb_search': 'TMDB-Suchen (Titel)',
  'settings.cache.label.tmdb_view': 'TMDB-Details (Modal)',
  'settings.cache.label.tmdb_details': 'TMDB-Details (Torrents)',
  'settings.cache.label.streams': 'Streams (BitTorrent-Stücke)',
  'settings.streamCacheTtlHint':
    'Bereinigung beim Start: Filme, die N Tage nicht abgespielt wurden, werden gelöscht. Zwischen 1 und 365.',
  'settings.glass.hint':
    '0 = maximale Transluzenz (Standard). 100 = fast solide Oberflächen, besser lesbar über Poster-Grids.',
  'settings.glass.crystal': 'Kristall',
  'settings.glass.solid': 'Solide',
  'settings.player.hint':
    'Eingebetteter Player oder externes VLC. Rechtsklick auf einen Torrent bietet immer VLC als Ausweg.',

  // ── Player ──────────────────────────────────────────
  'player.playTitle': 'Wiedergabe (Leertaste)',
  'player.pauseTitle': 'Pause (Leertaste)',
  'player.stats': 'Stream-Statistiken',
  'player.audioTrack': 'Audiospur',
  'player.subtitlesTitle': 'Untertitel (C)',
  'player.subtitles': 'Untertitel',
  'player.subtitle': 'Untertitel',
  'player.fullscreenTitle': 'Vollbild (F)',
  'player.muteTitle': 'Stumm (M)',
  'player.unmuteTitle': 'Ton an (M)',
  'player.available1': '{{n}} verfügbar',
  'player.availableN': '{{n}} verfügbar',
  'player.langUnknown': 'Unbekannte Sprache',
  'player.active': 'Aktiv',
  'player.trackN': 'Spur {{n}}',
  'player.removeCurrent': 'Entfernen',
  'player.embedded': 'Aus der Datei',
  'player.noSubs': 'Keine Untertitel verfügbar.',
  'player.noSubsHint':
    'OpenSubtitles hat keine Ergebnisse und der Container enthält keine eingebetteten Untertitel.',
  'player.downloads': '{{n}} Downloads',
  'player.trustedTitle': 'Von OpenSubtitles-Moderator verifiziert',
  'player.sdhTitle': 'Transkription für Gehörlose',
  'player.waitingData': 'Warte auf Daten…',
  'player.stat.speed': 'Geschwindigkeit',
  'player.stat.peers': 'Peers',
  'player.stat.progress': 'Fortschritt',
  'player.stat.downloaded': 'Heruntergeladen',
}
