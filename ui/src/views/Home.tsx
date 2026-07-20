import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { KeyReturn, X } from '@phosphor-icons/react'
import { HotkeyBar } from '../components/HotkeyBar'
import { TopNav } from '../components/TopNav'
import {
  hasSession,
  isTauri,
  listWatchProgress,
  removeWatchProgress,
  tmdbBackdrop,
  tmdbPoster,
  type WatchProgress,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'
import { useT } from '../lib/i18n'

/**
 * Menu principal, equivalente al enum `View::Menu` de la TUI: dos
 * opciones (Recomendaciones / Búsqueda directa) navegables con j/k y
 * Enter. Si no hay sesión de Letterboxd, "Recomendaciones" redirige a
 * /login antes.
 */
const OPTION_KEYS = [
  {
    key: 'recs',
    labelKey: 'home.optionRecsLabel',
    hintKey: 'home.optionRecsHint',
    path: '/recs',
    needsSession: true,
  },
  {
    key: 'search',
    labelKey: 'home.optionSearchLabel',
    hintKey: 'home.optionSearchHint',
    path: '/search',
    needsSession: false,
  },
] as const

export function Home() {
  const nav = useNavigate()
  const t = useT()
  const [i, setI] = useState(0)
  const [loggedIn, setLoggedIn] = useState<boolean | null>(null)
  const [progress, setProgress] = useState<WatchProgress[] | null>(null)

  useEffect(() => {
    if (!isTauri()) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- Gate no-Tauri: setState síncrona única para el dev en web puro; no cascada porque retornamos.
      setLoggedIn(false)
      return
    }
    hasSession().then(setLoggedIn).catch(() => setLoggedIn(false))
    // Catálogo "Seguir viendo": pelis/episodios con posición entre
    // 2%–95%. Fallo silencioso — si el backend no responde no
    // pintamos la sección, no rompemos el resto de Home.
    listWatchProgress()
      .then(setProgress)
      .catch(() => setProgress([]))
  }, [])

  // Navega al torrents/tmdb correspondiente. Como el resume ahora
  // vive por-peli (no por-torrent), la vista Torrents disparará el
  // ResumeDialog automáticamente al elegir cualquier release. NO
  // navegamos directo al player con el `last_magnet` guardado
  // porque:
  //   1. El torrent viejo puede ya no tener seeders — dejar que el
  //      user elija asegura una fuente viable.
  //   2. Torrents.tsx pinta metadata bonita del título (sinopsis,
  //      backdrop) que refuerza el "estás retomando esto".
  const openProgress = (p: WatchProgress) => {
    if (p.kind === 'series' && p.season != null && p.episode != null) {
      nav(`/torrents/series/${p.tmdb_id}?season=${p.season}&episode=${p.episode}`)
      return
    }
    const y = p.year ? `&year=${p.year}` : ''
    nav(`/torrents/tmdb/${p.tmdb_id}?title=${encodeURIComponent(p.title)}${y}`)
  }

  const removeProgress = async (p: WatchProgress) => {
    try {
      await removeWatchProgress(p.tmdb_id, p.season, p.episode)
      setProgress((prev) =>
        prev?.filter(
          (x) =>
            !(
              x.tmdb_id === p.tmdb_id &&
              x.season === p.season &&
              x.episode === p.episode
            ),
        ) ?? null,
      )
    } catch {
      /* best-effort */
    }
  }

  const go = (opt: (typeof OPTION_KEYS)[number]) => {
    if (opt.needsSession && loggedIn === false) {
      nav('/login?next=' + encodeURIComponent(opt.path))
      return
    }
    nav(opt.path)
  }

  const hotkeys: Hotkey[] = [
    { key: 'j', hint: t('home.down'), run: () => setI((x) => Math.min(x + 1, OPTION_KEYS.length - 1)) },
    { key: 'ArrowDown', hint: '', run: () => setI((x) => Math.min(x + 1, OPTION_KEYS.length - 1)) },
    { key: 'k', hint: t('home.up'), run: () => setI((x) => Math.max(x - 1, 0)) },
    { key: 'ArrowUp', hint: '', run: () => setI((x) => Math.max(x - 1, 0)) },
    { key: 'Enter', hint: t('home.select'), run: () => go(OPTION_KEYS[i]) },
    { key: ',', hint: t('nav.settings'), run: () => nav('/settings') },
  ]
  useHotkeys(hotkeys, [i, loggedIn])

  const barKeys: Hotkey[] = [
    { key: 'j', hint: t('hotkey.move'), run: () => {} },
    { key: 'Enter', hint: t('home.select'), run: () => {} },
  ]

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        {loggedIn ? (
          <span className="rounded-full px-3 py-1 text-[13px] text-muted">
            {t('home.sessionActive')}
          </span>
        ) : (
          <button
            onClick={() => nav('/login')}
            className="focus-ring glass rounded-full px-4 py-1.5 text-[13px] text-ink transition-transform hover:scale-[1.02]"
          >
            {t('login.title')}
          </button>
        )}
        <button
          onClick={() => nav('/settings')}
          className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-[13px] text-body hover:border-border-strong"
          title={`${t('nav.settings')} (,)`}
        >
          {t('nav.settings')}
        </button>
      </TopNav>

      <main className="mx-auto flex w-full max-w-[1120px] flex-1 flex-col justify-center px-8 py-10">
        <div className="mx-auto w-full max-w-[720px]">
          <h1 className="mb-2 text-[32px] font-semibold leading-tight tracking-tight text-ink">
            {t('home.headline')}
          </h1>
          <p className="mb-10 text-[15px] text-muted">
            {t('home.subhead')}
          </p>
        </div>

        {/* Sección "Seguir viendo": scroll horizontal con las pelis
            y episodios a mitad de reproducción. Se pinta ARRIBA de
            las opciones porque es la acción más frecuente ("continuar
            lo que ya empecé" > "buscar algo nuevo") y porque estar
            arriba del fold la hace descubrible. Ocultamos la sección
            entera si el catálogo está vacío — no queremos un placeholder
            de "empieza a ver algo" que sobra en la primera pantalla
            de la app.

            Anclada al mismo `max-w-[720px] mx-auto` que el resto de
            bloques de Home — con 1 sola card el <ul> antes ocupaba
            los 1120 px del `main` y quedaba visualmente descentrada
            respecto al headline y los CTA. Con 3+ cards el scroll
            horizontal sigue funcionando dentro de los 720 px. */}
        {progress && progress.length > 0 && (
          <section className="mx-auto mb-10 w-full max-w-[720px]">
            <div className="mb-4 flex items-baseline justify-between">
              <h2 className="text-[15px] font-semibold uppercase tracking-wide text-ink">
                {t('home.continueWatching')}
              </h2>
              <span className="text-[11px] tabular-nums text-dim">
                {progress.length}
              </span>
            </div>
            <ul className="scroll-hide flex snap-x snap-mandatory gap-4 overflow-x-auto pb-2">
              {progress.map((p) => (
                <ContinueWatchingCard
                  key={`${p.tmdb_id}-${p.season ?? 0}-${p.episode ?? 0}`}
                  entry={p}
                  onOpen={() => openProgress(p)}
                  onRemove={() => removeProgress(p)}
                />
              ))}
            </ul>
          </section>
        )}

        <div className="mx-auto w-full max-w-[720px]">
          <ul className="flex flex-col gap-2">
            {OPTION_KEYS.map((opt, idx) => {
              const active = idx === i
              return (
                <li key={opt.key}>
                  <button
                    onClick={() => go(opt)}
                    onMouseEnter={() => setI(idx)}
                    className={`focus-ring glass w-full rounded-lg px-5 py-4 text-left transition-transform ${
                      active
                        ? 'scale-[1.01] outline outline-1 outline-white/30'
                        : 'hover:scale-[1.005]'
                    }`}
                  >
                    <div className="flex items-baseline justify-between gap-4">
                      <span className="text-[16px] font-medium text-ink">
                        {t(opt.labelKey)}
                      </span>
                      {active && (
                        <span
                          className="flex h-6 w-6 items-center justify-center text-accent"
                          aria-label="Enter"
                          title="Enter"
                        >
                          <KeyReturn size={18} weight="bold" />
                        </span>
                      )}
                    </div>
                    <p className="mt-1 text-[13px] text-muted">{t(opt.hintKey)}</p>
                  </button>
                </li>
              )
            })}
          </ul>
        </div>
      </main>

      <HotkeyBar hotkeys={barKeys} />
    </div>
  )
}

/** Card horizontal de "Seguir viendo". Formato widescreen (usa el
 * backdrop de TMDB en 16:9 con fallback al poster) porque el
 * catálogo suele tener pocas entradas (3–6 típicas) y una tira de
 * widescreen se lee más rápido que una fila de posters verticales.
 * Botón "X" flotante en hover para eliminar la entrada del store
 * (`remove_watch_progress`). Barra inferior con % de progreso — la
 * única "métrica" honesta que hace justicia al nombre de la sección. */
function ContinueWatchingCard({
  entry,
  onOpen,
  onRemove,
}: {
  entry: WatchProgress
  onOpen: () => void
  onRemove: () => void
}) {
  const t = useT()
  const backdrop =
    tmdbBackdrop(entry.backdrop_path, 'w780') ??
    tmdbPoster(entry.poster_path, 'w500')
  const pct =
    entry.duration_seconds > 0
      ? Math.max(2, Math.min(98, (entry.seconds / entry.duration_seconds) * 100))
      : 5
  const label =
    entry.kind === 'series' && entry.season != null && entry.episode != null
      ? `${entry.title} · S${String(entry.season).padStart(2, '0')}E${String(entry.episode).padStart(2, '0')}`
      : entry.title
  const minsLeft =
    entry.duration_seconds > 0
      ? Math.max(1, Math.round((entry.duration_seconds - entry.seconds) / 60))
      : null
  return (
    <li className="group relative shrink-0 snap-start">
      <button
        onClick={onOpen}
        className="focus-ring block w-[260px] overflow-hidden rounded-lg bg-surface-hi text-left transition-transform hover:scale-[1.015]"
      >
        <div className="relative aspect-video w-full overflow-hidden">
          {backdrop ? (
            <img
              src={backdrop}
              alt={entry.title}
              loading="lazy"
              draggable={false}
              className="pointer-events-none h-full w-full select-none object-cover"
            />
          ) : (
            <div className="flex h-full w-full items-center justify-center px-3 text-center text-[12px] text-dim">
              {entry.title}
            </div>
          )}
          {/* Gradiente para legibilidad del label sobre la imagen. */}
          <div className="absolute inset-0 bg-gradient-to-t from-black/85 via-black/30 to-transparent" />
          {/* Progreso: barra fina al pie de la imagen. */}
          <div className="absolute inset-x-0 bottom-0 h-1 bg-white/10">
            <div
              className="h-full bg-accent"
              style={{ width: `${pct}%` }}
            />
          </div>
        </div>
        <div className="p-3">
          <p className="line-clamp-1 text-[13px] font-medium text-ink">{label}</p>
          <p className="mt-0.5 text-[11px] text-muted">
            {minsLeft != null
              ? t('home.minutesLeft', { n: minsLeft })
              : t('home.inProgress')}
          </p>
        </div>
      </button>
      {/* Botón "eliminar del catálogo" — invisible hasta hover; NO
          consume el click al card gracias a stopPropagation. */}
      <button
        onClick={(e) => {
          e.stopPropagation()
          onRemove()
        }}
        title={t('home.removeFromContinue')}
        aria-label={t('home.removeFromContinue')}
        className="focus-ring absolute right-2 top-2 flex h-7 w-7 items-center justify-center rounded-full bg-black/70 text-ink opacity-0 transition-opacity group-hover:opacity-100"
      >
        <X size={14} weight="bold" />
      </button>
    </li>
  )
}
