/**
 * Dictionnaire français. Les clés absentes retombent sur l’anglais
 * en `t()`.
 */
export const fr: Record<string, string> = {
  // ── Common ────────────────────────────────────────────
  'common.back': 'Retour',
  'common.close': 'Fermer',
  'common.cancel': 'Annuler',
  'common.save': 'Enregistrer',
  'common.loading': 'Chargement…',
  'common.retry': 'Réessayer',
  'common.play': 'Lire',

  // ── Nav ───────────────────────────────────────────────
  'nav.home': 'Accueil',
  'nav.recs': 'Recommandations',
  'nav.search': 'Rechercher',
  'nav.settings': 'Paramètres',
  'nav.session': 'Session',
  'nav.logout': 'Déconnexion',

  // ── Hotkey bar ────────────────────────────────────────
  'hotkey.move': 'Déplacer',
  'hotkey.play': 'Lire',
  'hotkey.magnet': 'Magnet',
  'hotkey.panel': 'Panneau',
  'hotkey.back': 'Retour',
  'hotkey.torrents': 'Torrents',
  'hotkey.episode': 'Épisode',
  'hotkey.season': 'Saison',
  'hotkey.seasonPack': 'Pack saison',
  'hotkey.dismiss': 'Ignorer',

  // ── Search ────────────────────────────────────────────
  'search.title': 'Rechercher des torrents',
  'search.hint': 'Tapez le titre. Ajoutez l’année à la fin pour distinguer les remakes (ex. « Funny Games 2007 »).',
  'search.placeholder': 'Titre…',
  'search.submit': 'Rechercher',

  // ── SearchResults ─────────────────────────────────────
  'searchResults.title': 'Résultats',
  'searchResults.matches': '{{n}} correspondances',
  'searchResults.searching': 'Recherche…',
  'searchResults.emptyTitle': 'Rien avec des torrents disponibles.',
  'searchResults.emptyHint': 'TMDB n’a renvoyé aucune correspondance, ou aucun indexeur n’a de torrents avec des seeders. Essayez le titre original en anglais ou ajoutez l’année.',
  'searchResults.badgeSeries': 'SÉRIE',

  // ── Torrents ──────────────────────────────────────────
  'torrents.title': 'Torrents',
  'torrents.results': '{{n}} résultats',
  'torrents.searching': 'Recherche…',
  'torrents.col.release': 'Release',
  'torrents.col.size': 'Taille',
  'torrents.col.seeds': 'Seeds',
  'torrents.col.leech': 'Leech',
  'torrents.col.quality': 'Qualité',
  'torrents.col.audio': 'Audio',
  'torrents.col.source': 'Source',
  'torrents.hint': 'Appuyez sur Entrée pour lire le torrent sélectionné. Les sous-titres se choisissent dans le lecteur. S envoie le magnet à votre client BitTorrent par défaut.',
  'torrents.matchKind.ep': 'ÉP',
  'torrents.matchKind.pack': 'PACK',
  'torrents.matchKind.series': 'SÉRIE',
  'torrents.chipTitle': 'Vous lirez cet épisode depuis le pack',
  'torrents.menu.playHtml': 'Lire dans le lecteur',
  'torrents.menu.playVlc': 'Lire dans VLC',
  'torrents.menu.playVlcOnce': 'Ouvrir dans VLC (ce torrent)',
  'torrents.menu.openClient': 'Ouvrir dans le client torrent',
  'torrents.menu.copyMagnet': 'Copier le magnet',

  // ── Series detail ─────────────────────────────────────
  'series.badge': 'Série',
  'series.seasonsCount': '{{n}} saisons',
  'series.seasonCount1': '1 saison',
  'series.loading': 'Chargement de la série…',
  'series.loadingEpisodes': 'Chargement des épisodes…',
  'series.noEpisodes': 'Aucun épisode listé pour cette saison.',
  'series.season': 'Saison {{n}}',
  'series.searchPack': 'Rechercher un pack de saison',
  'series.episodeShort': 'Épisode {{n}}',
  'series.noStill': 'pas d’image',
  'series.min': 'min',

  // ── Player ────────────────────────────────────────────
  'player.subs': 'Sous-titres',
  'player.nextEpisode': 'Épisode suivant →',
  'player.nextEpisodeTitle': 'Épisode suivant',
  'player.backTitle': 'Retour (Échap)',

  // ── Settings ──────────────────────────────────────────
  'settings.title': 'Paramètres',
  'settings.ui.section': 'Interface',
  'settings.ui.language': 'Langue',
  'settings.ui.languageHint': 'Langue de l’interface. Utilisée aussi comme première langue de sous-titres lors de la recherche.',
  'settings.subs.section': 'Sous-titres',
  'settings.subs.languages': 'Langues des sous-titres',
  'settings.subs.languagesHint': 'Codes ISO 639-1 séparés par des virgules (ex. « fr,en »). La langue de l’interface passe toujours en premier.',
  'settings.player.section': 'Lecteur',
  'settings.player.default': 'Lecteur par défaut',
  'settings.player.html': 'Intégré (HTML)',
  'settings.player.vlc': 'Externe (VLC)',
  'settings.recs.section': 'Recommandations',
  'settings.recs.minRating': 'Note minimale par défaut',
  'settings.cache.section': 'Cache',
  'settings.cache.clear': 'Vider',
  'settings.cache.clearAll': 'Tout vider',
  'settings.glass.section': 'Apparence',
  'settings.glass.opacity': 'Opacité du verre',

  // ── Resume dialog ─────────────────────────────────────
  'resume.title': 'Reprendre la lecture',
  'resume.at': 'Vous étiez à {{time}}',
  'resume.resume': 'Reprendre',
  'resume.restart': 'Recommencer',
}
