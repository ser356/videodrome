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
}
