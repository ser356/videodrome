/**
 * Dicionário português. As chaves ausentes voltam ao inglês em `t()`.
 */
export const pt: Record<string, string> = {
  // ── Common ────────────────────────────────────────────
  'common.back': 'Voltar',
  'common.close': 'Fechar',
  'common.cancel': 'Cancelar',
  'common.save': 'Guardar',
  'common.loading': 'A carregar…',
  'common.retry': 'Repetir',
  'common.play': 'Reproduzir',

  // ── Nav ───────────────────────────────────────────────
  'nav.home': 'Início',
  'nav.recs': 'Recomendações',
  'nav.search': 'Pesquisar',
  'nav.settings': 'Definições',
  'nav.session': 'Sessão',
  'nav.logout': 'Terminar sessão',

  // ── Hotkey bar ────────────────────────────────────────
  'hotkey.move': 'Mover',
  'hotkey.play': 'Reproduzir',
  'hotkey.magnet': 'Magnet',
  'hotkey.panel': 'Painel',
  'hotkey.back': 'Voltar',
  'hotkey.torrents': 'Torrents',
  'hotkey.episode': 'Episódio',
  'hotkey.season': 'Temporada',
  'hotkey.seasonPack': 'Pack temporada',
  'hotkey.dismiss': 'Descartar',

  // ── Search ────────────────────────────────────────────
  'search.title': 'Pesquisar torrents',
  'search.hint': 'Escreve o título. Adiciona o ano no fim para distinguir remakes (ex. «Funny Games 2007»).',
  'search.placeholder': 'Título…',
  'search.submit': 'Pesquisar',

  // ── SearchResults ─────────────────────────────────────
  'searchResults.title': 'Resultados',
  'searchResults.matches': '{{n}} correspondências',
  'searchResults.searching': 'A pesquisar…',
  'searchResults.emptyTitle': 'Nada com torrents disponíveis.',
  'searchResults.emptyHint': 'O TMDB não devolveu correspondências, ou nenhum indexer tem torrents com seeders. Tenta o título original em inglês ou adiciona o ano.',
  'searchResults.badgeSeries': 'SÉRIE',

  // ── Torrents ──────────────────────────────────────────
  'torrents.title': 'Torrents',
  'torrents.results': '{{n}} resultados',
  'torrents.searching': 'A pesquisar…',
  'torrents.col.release': 'Release',
  'torrents.col.size': 'Tamanho',
  'torrents.col.seeds': 'Seeds',
  'torrents.col.leech': 'Leech',
  'torrents.col.quality': 'Qualidade',
  'torrents.col.audio': 'Áudio',
  'torrents.col.source': 'Fonte',
  'torrents.hint': 'Pressiona Enter para reproduzir o torrent selecionado. As legendas escolhem-se no próprio leitor. S envia o magnet ao teu cliente BitTorrent predefinido.',
  'torrents.matchKind.ep': 'EP',
  'torrents.matchKind.pack': 'PACK',
  'torrents.matchKind.series': 'SÉRIE',
  'torrents.chipTitle': 'Vais reproduzir este episódio dentro do pack',
  'torrents.menu.playHtml': 'Reproduzir no leitor',
  'torrents.menu.playVlc': 'Reproduzir em VLC',
  'torrents.menu.playVlcOnce': 'Abrir em VLC (este torrent)',
  'torrents.menu.openClient': 'Abrir no cliente de torrents',
  'torrents.menu.copyMagnet': 'Copiar magnet',

  // ── Series detail ─────────────────────────────────────
  'series.badge': 'Série',
  'series.seasonsCount': '{{n}} temporadas',
  'series.seasonCount1': '1 temporada',
  'series.loading': 'A carregar série…',
  'series.loadingEpisodes': 'A carregar episódios…',
  'series.noEpisodes': 'Sem episódios listados para esta temporada.',
  'series.season': 'Temporada {{n}}',
  'series.searchPack': 'Pesquisar pack de temporada',
  'series.episodeShort': 'Episódio {{n}}',
  'series.noStill': 'sem still',
  'series.min': 'min',

  // ── Player ────────────────────────────────────────────
  'player.subs': 'Legendas',
  'player.nextEpisode': 'Próximo episódio →',
  'player.nextEpisodeTitle': 'Próximo episódio',
  'player.backTitle': 'Voltar (Esc)',

  // ── Settings ──────────────────────────────────────────
  'settings.title': 'Definições',
  'settings.ui.section': 'Interface',
  'settings.ui.language': 'Idioma',
  'settings.ui.languageHint': 'Idioma da interface. Também usado como primeiro idioma ao pesquisar legendas.',
  'settings.subs.section': 'Legendas',
  'settings.subs.languages': 'Idiomas das legendas',
  'settings.subs.languagesHint': 'Códigos ISO 639-1 separados por vírgulas (ex. «pt,en»). O idioma da interface vai sempre em primeiro.',
  'settings.player.section': 'Leitor',
  'settings.player.default': 'Leitor predefinido',
  'settings.player.html': 'Integrado (HTML)',
  'settings.player.vlc': 'Externo (VLC)',
  'settings.recs.section': 'Recomendações',
  'settings.recs.minRating': 'Avaliação mínima predefinida',
  'settings.cache.section': 'Cache',
  'settings.cache.clear': 'Limpar',
  'settings.cache.clearAll': 'Limpar tudo',
  'settings.glass.section': 'Aparência',
  'settings.glass.opacity': 'Opacidade do vidro',

  // ── Resume dialog ─────────────────────────────────────
  'resume.title': 'Retomar reprodução',
  'resume.at': 'Estavas em {{time}}',
  'resume.resume': 'Retomar',
  'resume.restart': 'Recomeçar',
}
