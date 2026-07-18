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
  'resume.eyebrow': 'Vous en avez déjà vu une partie',
  'resume.question': 'Reprendre où vous vous êtes arrêté ?',
  'resume.progress': 'Progression enregistrée',
  'resume.jumpTo': 'Aller à {{time}}',
  'resume.ignorePrevious': 'Ignorer la progression précédente',
  'resume.confirm': 'confirmer',

  // ── Home / Recs ───────────────────────────────────────
  'home.headline': 'Qu’est-ce qu’on regarde aujourd’hui ?',
  'home.subhead': 'Choisissez une option ou appuyez sur Entrée sur celle en surbrillance.',
  'home.sessionActive': 'Session active',
  'home.up': 'Haut',
  'home.down': 'Bas',
  'home.select': 'Sélectionner',
  'home.optionRecsLabel': 'Recommandations depuis Letterboxd',
  'home.optionRecsHint': 'Générer et parcourir des films recommandés basés sur votre historique.',
  'home.optionSearchLabel': 'Rechercher des torrents directement',
  'home.optionSearchHint': 'Tapez un titre et cherchez des torrents sans passer par Letterboxd.',

  // ── HotkeyBar tooltip ────────────────────────────────
  'hotkey.shortcutTitle': 'Raccourci : {{key}}',

  // ── StreamPanel ──────────────────────────────────────
  'streamPanel.streaming': 'Lecture',
  'streamPanel.stop': 'Arrêter',
  'streamPanel.hintPre': 'Appuyez sur',
  'streamPanel.hintMid': 'pour lire le torrent sélectionné. Les sous-titres se choisissent dans le lecteur.',
  'streamPanel.hintPost': 'envoie le magnet à votre client BitTorrent par défaut.',

  // ── Login extras ─────────────────────────────────────
  'login.title': 'Se connecter',
  'login.username': 'Utilisateur',
  'login.password': 'Mot de passe',
  'login.submit': 'Se connecter',
  'login.hint': 'Les identifiants restent en local ; ils ne quittent jamais votre machine.',
  'login.onlyDesktop': 'Cette fenêtre ne fonctionne que dans l’app de bureau.',
  'login.verifying': 'Vérification…',

  // ── Recommendations ──────────────────────────────────
  'recs.title': 'À l’affiche',
  'recs.reload': 'Recharger',
  'recs.detail': 'Détail',
  'recs.emptyTitle': 'Aucun résultat.',
  'recs.emptyHint': 'Baissez la note minimale ou vérifiez votre historique Letterboxd.',
  'recs.endOfList': 'Fin de la liste. {{n}} recommandations.',
  'recs.dismissError': 'Erreur en écartant : {{err}}',
  'recs.dismissedFlash': 'Écartée : {{title}}. Restaurer depuis Paramètres.',
  'recs.menu.detail': 'Voir le détail',
  'recs.menu.torrents': 'Voir les torrents',

  // ── Movie detail modal ───────────────────────────────
  'movieDetail.noOverview': 'Aucun synopsis disponible.',
  'movieDetail.viewTorrents': 'Voir les torrents',

  // ── Search box ───────────────────────────────────────
  'search.boxPlaceholder': 'Rechercher un film…',

  // ── Time ────────────────────────────────────────────
  'time.secondsShort': 'il y a {{n}}s',
  'time.minutesShort': 'il y a {{n}}min',
  'time.hoursShort': 'il y a {{n}}h',
  'time.daysShort': 'il y a {{n}}j',

  // ── Settings extras ─────────────────────────────────
  'settings.session.section': 'Session',
  'settings.session.noSession': 'Pas de session',
  'settings.logoutDone': 'Session fermée.',
  'settings.preferences.section': 'Préférences',
  'settings.dismissed.section': 'Suggestions écartées',
  'settings.dismissed.count': '{{n}} films',
  'settings.dismissed.count1': '1 film',
  'settings.dismissed.empty':
    'Vous n’avez rien écarté. Clic droit sur un film dans « À l’affiche » → « Ne plus suggérer ».',
  'settings.dismissed.restored': 'Restaurée : {{title}}',
  'settings.cache.cleared': 'Cache « {{kind}} » vidé.',
  'settings.cache.allCleared': 'Tous les caches vidés.',
  'settings.cache.updatedAgo': 'Mis à jour {{age}}',
  'settings.cache.empty': 'vide',
  'settings.cache.sessionHint': 'La session ne se ferme pas ici. Utilisez « Déconnexion » ci-dessus.',
  'settings.cache.label.log_entries': 'Historique Letterboxd',
  'settings.cache.label.watchlist': 'Watchlist Letterboxd',
  'settings.cache.label.tmdb_recs': 'Recommandations TMDB',
  'settings.cache.label.search': 'Recherches TMDB + torrents',
  'settings.cache.label.torrent_search': 'Résultats torrents (30 min / 5 min vide)',
  'settings.cache.label.tmdb_search': 'Recherches TMDB (titres)',
  'settings.cache.label.tmdb_view': 'Détails TMDB (modal)',
  'settings.cache.label.tmdb_details': 'Détails TMDB (torrents)',
  'settings.cache.label.streams': 'Streams (morceaux BitTorrent)',
  'settings.streamCacheTtlHint':
    'Purge au démarrage : les films non lus depuis N jours sont supprimés du disque. Entre 1 et 365.',
  'settings.glass.hint':
    '0 = translucidité max (défaut). 100 = surfaces presque solides, plus lisibles sur les grilles d’affiches.',
  'settings.glass.crystal': 'Cristal',
  'settings.glass.solid': 'Solide',
  'settings.player.hint':
    'Lecteur intégré ou VLC externe. Le clic droit sur un torrent propose toujours VLC comme échappatoire.',

  // ── Player ──────────────────────────────────────────
  'player.playTitle': 'Lire (Espace)',
  'player.pauseTitle': 'Pause (Espace)',
  'player.stats': 'Statistiques du stream',
  'player.audioTrack': 'Piste audio',
  'player.subtitlesTitle': 'Sous-titres (C)',
  'player.subtitles': 'Sous-titres',
  'player.subtitle': 'Sous-titre',
  'player.fullscreenTitle': 'Plein écran (F)',
  'player.muteTitle': 'Muet (M)',
  'player.unmuteTitle': 'Réactiver le son (M)',
  'player.available1': '{{n}} disponible',
  'player.availableN': '{{n}} disponibles',
  'player.langUnknown': 'Langue inconnue',
  'player.active': 'Actif',
  'player.trackN': 'Piste {{n}}',
  'player.removeCurrent': 'Retirer',
  'player.embedded': 'Du fichier',
  'player.noSubs': 'Aucun sous-titre disponible.',
  'player.noSubsHint':
    'OpenSubtitles n’a pas de résultats et le conteneur ne contient pas de sous-titres embarqués.',
  'player.downloads': '{{n}} téléchargements',
  'player.trustedTitle': 'Vérifié par un modérateur OpenSubtitles',
  'player.sdhTitle': 'Transcription pour sourds et malentendants',
  'player.waitingData': 'En attente de données…',
  'player.stat.speed': 'Vitesse',
  'player.stat.peers': 'Peers',
  'player.stat.progress': 'Progression',
  'player.stat.downloaded': 'Téléchargé',
  'player.ffmpegHintMac': 'Installez-le avec `brew install ffmpeg`.',
}
