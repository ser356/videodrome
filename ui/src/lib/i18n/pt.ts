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
  'resume.eyebrow': 'Já viste parte disto',
  'resume.question': 'Retomar onde ficaste?',
  'resume.progress': 'Progresso guardado',
  'resume.jumpTo': 'Ir para {{time}}',
  'resume.ignorePrevious': 'Ignorar o progresso anterior',
  'resume.confirm': 'confirmar',

  // ── Home / Recs ───────────────────────────────────────
  'home.headline': 'O que vemos hoje?',
  'home.subhead': 'Escolhe uma opção ou pressiona Enter na realçada.',
  'home.sessionActive': 'Sessão ativa',
  'home.up': 'Cima',
  'home.down': 'Baixo',
  'home.select': 'Selecionar',
  'home.optionRecsLabel': 'Recomendações do Letterboxd',
  'home.optionRecsHint': 'Gera e navega por filmes recomendados com base no teu histórico.',
  'home.optionSearchLabel': 'Pesquisar torrents diretamente',
  'home.optionSearchHint': 'Escreve um título e pesquisa torrents sem passar pelo Letterboxd.',

  // ── HotkeyBar tooltip ────────────────────────────────
  'hotkey.shortcutTitle': 'Atalho: {{key}}',

  // ── StreamPanel ──────────────────────────────────────
  'streamPanel.streaming': 'A reproduzir',
  'streamPanel.stop': 'Parar',
  'streamPanel.hintPre': 'Pressiona',
  'streamPanel.hintMid': 'para reproduzir o torrent selecionado. As legendas escolhem-se no leitor.',
  'streamPanel.hintPost': 'envia o magnet ao teu cliente BitTorrent predefinido.',

  // ── Login extras ─────────────────────────────────────
  'login.title': 'Iniciar sessão',
  'login.username': 'Utilizador',
  'login.password': 'Palavra-passe',
  'login.submit': 'Iniciar sessão',
  'login.hint': 'As credenciais ficam locais; nunca saem da tua máquina.',
  'login.onlyDesktop': 'Esta janela só funciona dentro da app desktop.',
  'login.verifying': 'A verificar…',

  // ── Recommendations ──────────────────────────────────
  'recs.title': 'Em cartaz',
  'recs.reload': 'Recarregar',
  'recs.detail': 'Detalhe',
  'recs.emptyTitle': 'Sem resultados.',
  'recs.emptyHint': 'Baixa a avaliação mínima ou verifica o teu histórico no Letterboxd.',
  'recs.endOfList': 'Fim da lista. {{n}} recomendações.',
  'recs.dismissError': 'Erro ao descartar: {{err}}',
  'recs.dismissedFlash': 'Descartada: {{title}}. Restaurar em Definições.',
  'recs.menu.detail': 'Ver detalhe',
  'recs.menu.torrents': 'Ver torrents',

  // ── Movie detail modal ───────────────────────────────
  'movieDetail.noOverview': 'Sem sinopse disponível.',
  'movieDetail.viewTorrents': 'Ver torrents',

  // ── Search box ───────────────────────────────────────
  'search.boxPlaceholder': 'Pesquisar filme…',

  // ── Time ────────────────────────────────────────────
  'time.secondsShort': 'há {{n}}s',
  'time.minutesShort': 'há {{n}}min',
  'time.hoursShort': 'há {{n}}h',
  'time.daysShort': 'há {{n}}d',

  // ── Settings extras ─────────────────────────────────
  'settings.session.section': 'Sessão',
  'settings.session.noSession': 'Sem sessão',
  'settings.logoutDone': 'Sessão terminada.',
  'settings.preferences.section': 'Preferências',
  'settings.dismissed.section': 'Sugestões descartadas',
  'settings.dismissed.count': '{{n}} filmes',
  'settings.dismissed.count1': '1 filme',
  'settings.dismissed.empty':
    'Não descartaste nenhuma recomendação. Clique direito num filme em «Em cartaz» → «Não sugerir».',
  'settings.dismissed.restored': 'Restaurada: {{title}}',
  'settings.cache.cleared': 'Cache «{{kind}}» limpa.',
  'settings.cache.allCleared': 'Todas as caches limpas.',
  'settings.cache.updatedAgo': 'Atualizada {{age}}',
  'settings.cache.empty': 'vazia',
  'settings.cache.sessionHint': 'A sessão não se limpa aqui. Usa «Terminar sessão» acima.',
  'settings.cache.label.log_entries': 'Histórico Letterboxd',
  'settings.cache.label.watchlist': 'Watchlist Letterboxd',
  'settings.cache.label.tmdb_recs': 'Recomendações TMDB',
  'settings.cache.label.search': 'Pesquisas TMDB + torrents',
  'settings.cache.label.torrent_search': 'Resultados de torrents (30 min / 5 min vazio)',
  'settings.cache.label.tmdb_search': 'Pesquisas TMDB (títulos)',
  'settings.cache.label.tmdb_view': 'Detalhes TMDB (modal)',
  'settings.cache.label.tmdb_details': 'Detalhes TMDB (torrents)',
  'settings.cache.label.streams': 'Streams (peças BitTorrent)',
  'settings.streamCacheTtlHint':
    'Limpeza ao arrancar: filmes não reproduzidos há N dias são apagados do disco. Entre 1 e 365.',
  'settings.glass.hint':
    '0 = translucidez máx (default). 100 = superfícies quase sólidas, mais legíveis sobre grelhas de posters.',
  'settings.glass.crystal': 'Cristal',
  'settings.glass.solid': 'Sólido',
  'settings.player.hint':
    'Leitor integrado ou VLC externo. O clique direito num torrent oferece sempre VLC como saída de emergência.',

  // ── Player ──────────────────────────────────────────
  'player.playTitle': 'Reproduzir (Espaço)',
  'player.pauseTitle': 'Pausa (Espaço)',
  'player.stats': 'Estatísticas do stream',
  'player.audioTrack': 'Faixa de áudio',
  'player.subtitlesTitle': 'Legendas (C)',
  'player.subtitles': 'Legendas',
  'player.subtitle': 'Legenda',
  'player.fullscreenTitle': 'Ecrã inteiro (F)',
  'player.muteTitle': 'Sem som (M)',
  'player.unmuteTitle': 'Reativar som (M)',
  'player.available1': '{{n}} disponível',
  'player.availableN': '{{n}} disponíveis',
  'player.langUnknown': 'Idioma desconhecido',
  'player.active': 'Ativo',
  'player.trackN': 'Faixa {{n}}',
  'player.removeCurrent': 'Remover atual',
  'player.embedded': 'Do ficheiro',
  'player.noSubs': 'Sem legendas disponíveis.',
  'player.noSubsHint':
    'OpenSubtitles não tem resultados e o contentor não tem legendas embutidas.',
  'player.downloads': '{{n}} descargas',
  'player.trustedTitle': 'Verificado por moderador OpenSubtitles',
  'player.sdhTitle': 'Transcrição para surdos',
  'player.waitingData': 'À espera de dados…',
  'player.stat.speed': 'Velocidade',
  'player.stat.peers': 'Peers',
  'player.stat.progress': 'Progresso',
  'player.stat.downloaded': 'Descarregado',
}
