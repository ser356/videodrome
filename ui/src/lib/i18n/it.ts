/**
 * Dizionario italiano. Le chiavi mancanti ricadono sull’inglese in `t()`.
 */
export const it: Record<string, string> = {
  // ── Common ────────────────────────────────────────────
  'common.back': 'Indietro',
  'common.close': 'Chiudi',
  'common.cancel': 'Annulla',
  'common.save': 'Salva',
  'common.loading': 'Caricamento…',
  'common.retry': 'Riprova',
  'common.play': 'Riproduci',

  // ── Nav ───────────────────────────────────────────────
  'nav.home': 'Home',
  'nav.recs': 'Consigliati',
  'nav.search': 'Cerca',
  'nav.settings': 'Impostazioni',
  'nav.session': 'Sessione',
  'nav.logout': 'Esci',

  // ── Hotkey bar ────────────────────────────────────────
  'hotkey.move': 'Sposta',
  'hotkey.play': 'Riproduci',
  'hotkey.magnet': 'Magnet',
  'hotkey.panel': 'Pannello',
  'hotkey.back': 'Indietro',
  'hotkey.torrents': 'Torrent',
  'hotkey.episode': 'Episodio',
  'hotkey.season': 'Stagione',
  'hotkey.seasonPack': 'Pack stagione',
  'hotkey.dismiss': 'Scarta',

  // ── Search ────────────────────────────────────────────
  'search.title': 'Cerca torrent',
  'search.hint': 'Digita il titolo. Aggiungi l’anno alla fine per distinguere i remake (es. «Funny Games 2007»).',
  'search.placeholder': 'Titolo…',
  'search.submit': 'Cerca',

  // ── SearchResults ─────────────────────────────────────
  'searchResults.title': 'Risultati',
  'searchResults.matches': '{{n}} corrispondenze',
  'searchResults.searching': 'Ricerca…',
  'searchResults.emptyTitle': 'Nessun torrent disponibile.',
  'searchResults.emptyHint': 'TMDB non ha restituito corrispondenze, o nessun indexer ha torrent con seeder. Prova il titolo originale in inglese o aggiungi l’anno.',
  'searchResults.badgeSeries': 'SERIE',

  // ── Torrents ──────────────────────────────────────────
  'torrents.title': 'Torrent',
  'torrents.results': '{{n}} risultati',
  'torrents.searching': 'Ricerca…',
  'torrents.col.release': 'Release',
  'torrents.col.size': 'Dimensione',
  'torrents.col.seeds': 'Seed',
  'torrents.col.leech': 'Leech',
  'torrents.col.quality': 'Qualità',
  'torrents.col.audio': 'Audio',
  'torrents.col.source': 'Fonte',
  'torrents.hint': 'Premi Invio per riprodurre il torrent selezionato. I sottotitoli si scelgono dal player. S invia il magnet al tuo client BitTorrent predefinito.',
  'torrents.matchKind.ep': 'EP',
  'torrents.matchKind.pack': 'PACK',
  'torrents.matchKind.series': 'SERIE',
  'torrents.chipTitle': 'Riprodurrai questo episodio dal pack',
  'torrents.menu.playHtml': 'Riproduci nel player',
  'torrents.menu.playVlc': 'Riproduci in VLC',
  'torrents.menu.playVlcOnce': 'Apri in VLC (questo torrent)',
  'torrents.menu.openClient': 'Apri nel client torrent',
  'torrents.menu.copyMagnet': 'Copia magnet',

  // ── Series detail ─────────────────────────────────────
  'series.badge': 'Serie',
  'series.seasonsCount': '{{n}} stagioni',
  'series.seasonCount1': '1 stagione',
  'series.loading': 'Caricamento serie…',
  'series.loadingEpisodes': 'Caricamento episodi…',
  'series.noEpisodes': 'Nessun episodio elencato per questa stagione.',
  'series.season': 'Stagione {{n}}',
  'series.searchPack': 'Cerca pack di stagione',
  'series.episodeShort': 'Episodio {{n}}',
  'series.noStill': 'no still',
  'series.min': 'min',

  // ── Player ────────────────────────────────────────────
  'player.subs': 'Sottotitoli',
  'player.nextEpisode': 'Prossimo episodio →',
  'player.nextEpisodeTitle': 'Prossimo episodio',
  'player.backTitle': 'Indietro (Esc)',

  // ── Settings ──────────────────────────────────────────
  'settings.title': 'Impostazioni',
  'settings.ui.section': 'Interfaccia',
  'settings.ui.language': 'Lingua',
  'settings.ui.languageHint': 'Lingua dell’interfaccia. Usata anche come prima lingua nei sottotitoli.',
  'settings.subs.section': 'Sottotitoli',
  'settings.subs.languages': 'Lingue dei sottotitoli',
  'settings.subs.languagesHint': 'Codici ISO 639-1 separati da virgole (es. «it,en»). La lingua dell’interfaccia va sempre per prima.',
  'settings.player.section': 'Player',
  'settings.player.default': 'Player predefinito',
  'settings.player.html': 'Integrato (HTML)',
  'settings.player.vlc': 'Esterno (VLC)',
  'settings.recs.section': 'Consigliati',
  'settings.recs.minRating': 'Valutazione minima predefinita',
  'settings.cache.section': 'Cache',
  'settings.cache.clear': 'Svuota',
  'settings.cache.clearAll': 'Svuota tutto',
  'settings.glass.section': 'Aspetto',
  'settings.glass.opacity': 'Opacità del vetro',

  // ── Resume dialog ─────────────────────────────────────
  'resume.title': 'Riprendi riproduzione',
  'resume.at': 'Eri a {{time}}',
  'resume.resume': 'Riprendi',
  'resume.restart': 'Ricomincia',
}
