import { useCallback, useEffect, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { HotkeyBar } from '../components/HotkeyBar'
import { TopNav } from '../components/TopNav'
import {
  getSeriesSeason,
  getSeriesView,
  isTauri,
  tmdbBackdrop,
  tmdbPoster,
  type SeriesDetails,
  type SeriesEpisode,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'
import { useT } from '../lib/i18n'

/**
 * Vista SeriesDetail (§7 audit series). Se llega desde SearchResults
 * al hacer click en un hit con `kind = 'series'`.
 *
 * Layout:
 *   - Header con backdrop + poster + metadata (name, año, overview,
 *     status).
 *   - Selector horizontal de temporadas (tabs — número + episode_count).
 *     Excluye "Season 0" (specials) por defecto.
 *   - Lista de episodios de la temporada seleccionada.
 *   - Acción "Temporada completa" que va a `/torrents/series/:id?season=N`
 *     sin `episode` (busca packs).
 *
 * Hotkeys:
 *   j/k mover en episodios · h/l cambiar temporada · Enter buscar
 *   torrents del episodio · p buscar pack de temporada · Esc volver.
 */
export function SeriesDetail() {
  const nav = useNavigate()
  const t = useT()
  const { tmdbId } = useParams<{ tmdbId?: string }>()
  const [params] = useSearchParams()
  const fallbackTitle = params.get('title') ?? ''

  const [details, setDetails] = useState<SeriesDetails | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  const [selectedSeason, setSelectedSeason] = useState<number | null>(null)
  const [episodes, setEpisodes] = useState<SeriesEpisode[]>([])
  const [epsLoading, setEpsLoading] = useState(false)
  const [sel, setSel] = useState(0)

  // Fetch series details on mount.
  useEffect(() => {
    if (!isTauri()) {
      setError(t('series.tauriRequired'))
      setLoading(false)
      return
    }
    const id = Number(tmdbId ?? '')
    if (!Number.isFinite(id) || id <= 0) {
      setError(t('series.invalidId'))
      setLoading(false)
      return
    }
    setLoading(true)
    setError(null)
    getSeriesView(id)
      .then((d) => {
        setDetails(d)
        // Preseleccionar la primera temporada "real" (skip specials S0
        // salvo que sea lo único que haya).
        const first =
          d?.seasons.find((s) => s.season_number > 0) ??
          d?.seasons[0] ??
          null
        setSelectedSeason(first?.season_number ?? null)
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false))
  }, [tmdbId])

  // Fetch episodes when the season changes.
  useEffect(() => {
    if (!isTauri() || !tmdbId || selectedSeason == null) return
    const id = Number(tmdbId)
    setEpsLoading(true)
    setEpisodes([])
    setSel(0)
    getSeriesSeason(id, selectedSeason)
      .then(setEpisodes)
      .catch(() => setEpisodes([]))
      .finally(() => setEpsLoading(false))
  }, [tmdbId, selectedSeason])

  const openEpisode = useCallback(
    (ep: SeriesEpisode) => {
      if (!tmdbId) return
      nav(
        `/torrents/series/${tmdbId}?season=${ep.season_number}&episode=${ep.episode_number}`,
      )
    },
    [tmdbId, nav],
  )

  const openSeasonPack = useCallback(() => {
    if (!tmdbId || selectedSeason == null) return
    nav(`/torrents/series/${tmdbId}?season=${selectedSeason}`)
  }, [tmdbId, selectedSeason, nav])

  const changeSeason = (delta: number) => {
    if (!details) return
    const seasons = details.seasons.filter((s) => s.season_number > 0)
    if (seasons.length === 0) return
    const currentIdx = seasons.findIndex(
      (s) => s.season_number === selectedSeason,
    )
    const nextIdx =
      currentIdx < 0
        ? 0
        : (currentIdx + delta + seasons.length) % seasons.length
    setSelectedSeason(seasons[nextIdx].season_number)
  }

  const move = (delta: number) => {
    const n = episodes.length
    if (n === 0) return
    setSel((i) => (i + delta + n) % n)
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: '', run: () => move(1) },
    { key: 'ArrowDown', hint: '', run: () => move(1) },
    { key: 'k', hint: t('hotkey.episode'), run: () => move(-1) },
    { key: 'ArrowUp', hint: '', run: () => move(-1) },
    { key: 'l', hint: '', run: () => changeSeason(1) },
    { key: 'ArrowRight', hint: t('hotkey.season'), run: () => changeSeason(1) },
    { key: 'h', hint: '', run: () => changeSeason(-1) },
    { key: 'ArrowLeft', hint: '', run: () => changeSeason(-1) },
    {
      key: 'Enter',
      hint: t('hotkey.torrents'),
      run: () => episodes[sel] && openEpisode(episodes[sel]),
    },
    { key: 'p', hint: t('hotkey.seasonPack'), run: openSeasonPack },
    { key: 'Escape', hint: t('common.back'), run: () => nav(-1) },
  ]
  useHotkeys(hotkeys, [episodes, sel, details, selectedSeason])

  const title = details?.name ?? fallbackTitle
  const year = details?.first_air_date?.slice(0, 4)
  const backdrop = tmdbBackdrop(details?.backdrop_path ?? null)
  const poster = tmdbPoster(details?.poster_path ?? null, 'w342')
  const realSeasons = details?.seasons.filter((s) => s.season_number > 0) ?? []

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        <button
          onClick={() => nav(-1)}
          className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-body hover:border-border-strong"
        >
          {t('common.back')}
        </button>
      </TopNav>

      <main className="mx-auto w-full max-w-[1200px] flex-1 px-8 py-6">
        {error && (
          <div className="rounded-sm border border-danger/40 bg-danger/10 p-4 text-[14px] text-danger">
            {error}
          </div>
        )}

        {loading && !error && (
          <div className="mt-16 text-center text-[14px] text-muted">
            {t('series.loading')}
          </div>
        )}

        {details && (
          <>
            <SeriesHeader
              title={title}
              year={year ?? null}
              overview={details.overview}
              status={details.status}
              seasons={details.number_of_seasons}
              backdrop={backdrop}
              poster={poster}
            />

            {realSeasons.length > 1 && (
              <SeasonTabs
                seasons={realSeasons.map((s) => ({
                  number: s.season_number,
                  count: s.episode_count,
                }))}
                selected={selectedSeason}
                onSelect={setSelectedSeason}
              />
            )}

            <div className="mb-3 mt-6 flex items-center justify-between">
              <h2 className="text-[16px] font-semibold text-ink">
                {selectedSeason
                  ? t('series.season', { n: selectedSeason })
                  : t('hotkey.episode')}
              </h2>
              <button
                onClick={openSeasonPack}
                className="focus-ring rounded-full border border-hairline px-3 py-1 text-[12px] text-body hover:border-border-strong"
                disabled={selectedSeason == null}
              >
                {t('series.searchPack')}
              </button>
            </div>

            {epsLoading && (
              <div className="text-[13px] text-muted">{t('series.loadingEpisodes')}</div>
            )}

            {!epsLoading && episodes.length === 0 && (
              <div className="rounded-sm border border-hairline bg-surface p-4 text-[13px] text-muted">
                {t('series.noEpisodes')}
              </div>
            )}

            {!epsLoading && episodes.length > 0 && (
              <ul className="flex flex-col gap-2">
                {episodes.map((ep, i) => (
                  <EpisodeRow
                    key={`${ep.season_number}-${ep.episode_number}`}
                    ep={ep}
                    active={i === sel}
                    onClick={() => {
                      setSel(i)
                      openEpisode(ep)
                    }}
                    onMouseEnter={() => setSel(i)}
                  />
                ))}
              </ul>
            )}
          </>
        )}
      </main>

      <HotkeyBar hotkeys={hotkeys.filter((h) => h.hint)} />
    </div>
  )
}

function SeriesHeader({
  title,
  year,
  overview,
  status,
  seasons,
  backdrop,
  poster,
}: {
  title: string
  year: string | null
  overview: string | null
  status: string | null
  seasons: number
  backdrop: string | null
  poster: string | null
}) {
  const t = useT()
  return (
    <div className="relative mb-6 overflow-hidden rounded-lg border border-hairline bg-surface">
      {backdrop && (
        <div
          className="absolute inset-0 bg-cover bg-center opacity-25"
          style={{ backgroundImage: `url(${backdrop})` }}
          aria-hidden
        />
      )}
      <div className="relative flex gap-6 p-6">
        {poster && (
          <img
            src={poster}
            alt={title}
            className="h-[220px] w-[147px] shrink-0 rounded-poster object-cover shadow-lg"
            draggable={false}
          />
        )}
        <div className="min-w-0 flex-1">
          <div className="mb-1 flex items-baseline gap-3">
            <h1 className="truncate text-[22px] font-semibold text-ink">
              {title}
            </h1>
            {year && (
              <span className="shrink-0 text-[14px] text-muted">{year}</span>
            )}
          </div>
          <div className="mb-3 flex items-center gap-2 text-[12px] text-dim">
            <span className="rounded-sm border border-accent/40 px-1.5 py-0 text-[10px] font-semibold uppercase tracking-wide text-accent">
              {t('series.badge')}
            </span>
            {status && <span>· {status}</span>}
            <span>
              ·{' '}
              {seasons === 1
                ? t('series.seasonCount1')
                : t('series.seasonsCount', { n: seasons })}
            </span>
          </div>
          {overview && (
            <p className="line-clamp-6 text-[13px] leading-relaxed text-body">
              {overview}
            </p>
          )}
        </div>
      </div>
    </div>
  )
}

function SeasonTabs({
  seasons,
  selected,
  onSelect,
}: {
  seasons: Array<{ number: number; count: number }>
  selected: number | null
  onSelect: (n: number) => void
}) {
  return (
    <div className="flex flex-wrap gap-2">
      {seasons.map((s) => {
        const active = s.number === selected
        return (
          <button
            key={s.number}
            onClick={() => onSelect(s.number)}
            className={`focus-ring rounded-full border px-3 py-1 text-[12px] transition-colors ${
              active
                ? 'border-accent bg-accent text-on-accent'
                : 'border-hairline text-body hover:border-border-strong'
            }`}
          >
            T{s.number}
            <span className="ml-1.5 text-[10px] opacity-70">· {s.count}</span>
          </button>
        )
      })}
    </div>
  )
}

function EpisodeRow({
  ep,
  active,
  onClick,
  onMouseEnter,
}: {
  ep: SeriesEpisode
  active: boolean
  onClick: () => void
  onMouseEnter: () => void
}) {
  const t = useT()
  const still = ep.still_path
    ? `https://image.tmdb.org/t/p/w300${ep.still_path}`
    : null
  return (
    <li
      onClick={onClick}
      onMouseEnter={onMouseEnter}
      className={`flex cursor-pointer gap-4 rounded-sm border border-hairline-soft p-3 transition-colors ${
        active ? 'bg-surface-hi' : 'bg-surface hover:bg-surface-hi'
      }`}
    >
      {still ? (
        <img
          src={still}
          alt=""
          className="h-[80px] w-[142px] shrink-0 rounded-sm object-cover"
          draggable={false}
        />
      ) : (
        <div className="flex h-[80px] w-[142px] shrink-0 items-center justify-center rounded-sm bg-surface-hi text-[10px] text-dim">
          {t('series.noStill')}
        </div>
      )}
      <div className="min-w-0 flex-1">
        <div className="mb-1 flex items-baseline gap-2">
          <span
            className={`text-[13px] font-mono ${
              active ? 'text-accent' : 'text-dim'
            }`}
          >
            E{String(ep.episode_number).padStart(2, '0')}
          </span>
          <p className="truncate text-[14px] text-ink">
            {ep.name ?? t('series.episodeShort', { n: ep.episode_number })}
          </p>
          {ep.runtime != null && (
            <span className="shrink-0 text-[11px] text-dim">
              · {ep.runtime} {t('series.min')}
            </span>
          )}
          {ep.air_date && (
            <span className="shrink-0 text-[11px] text-dim">
              · {ep.air_date}
            </span>
          )}
        </div>
        {ep.overview && (
          <p className="line-clamp-2 text-[12px] leading-relaxed text-muted">
            {ep.overview}
          </p>
        )}
      </div>
    </li>
  )
}
