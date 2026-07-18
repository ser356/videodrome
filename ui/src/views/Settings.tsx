import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { HotkeyBar } from '../components/HotkeyBar'
import { Toast } from '../components/Toast'
import { TopNav } from '../components/TopNav'
import {
  cacheInfo,
  clearCache,
  formatSize,
  getPreferences,
  getUsername,
  hasSession,
  isTauri,
  listDismissed,
  logInfo,
  logout,
  openLogFolder,
  setPreferences,
  tmdbPoster,
  undismissRecommendation,
  type AppLogInfo,
  type CacheEntry,
  type DismissedEntry,
  type Preferences,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'
import { getLocale, LOCALE_LABELS, setLocale, SUPPORTED_LOCALES, useT } from '../lib/i18n'
import { applyGlassOpacity } from '../lib/theme'

/**
 * Vista de Ajustes. Tres bloques:
 *   1. Sesión: username + logout.
 *   2. Preferencias: rating/count por defecto de Recs, idiomas de subs.
 *      Se guardan en `preferences.json` vía `set_preferences`.
 *   3. Caché: lista los ficheros conocidos (log entries, watchlist, recs
 *      TMDB, búsquedas) con tamaño/edad y permite borrarlos individual
 *      o globalmente.
 *
 * No se toca `token.json`: la sesión se cierra con el botón de logout,
 * no con "borrar caché".
 */
export function Settings() {
  const nav = useNavigate()
  const t = useT()

  const [username, setUsername] = useState<string | null>(null)
  const [prefs, setPrefs] = useState<Preferences | null>(null)
  const [caches, setCaches] = useState<CacheEntry[] | null>(null)
  const [dismissed, setDismissed] = useState<DismissedEntry[] | null>(null)
  const [log, setLog] = useState<AppLogInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setError('Esta vista requiere la app de escritorio (Tauri).')
      return
    }
    try {
      const [hasS, list, p, dis, li] = await Promise.all([
        hasSession(),
        cacheInfo(),
        getPreferences(),
        listDismissed(),
        logInfo(),
      ])
      setCaches(list)
      setPrefs(p)
      setDismissed(dis)
      setLog(li)
      if (hasS) {
        setUsername(await getUsername())
      } else {
        setUsername(null)
      }
    } catch (e) {
      setError(String(e))
    }
  }, [])

  useEffect(() => {
    refresh()
  }, [refresh])

  const flash = (msg: string) => {
    setToast(msg)
    window.setTimeout(() => setToast(null), 2500)
  }

  const onLogout = async () => {
    try {
      await logout()
      flash(t('settings.logoutDone'))
      nav('/login')
    } catch (e) {
      setError(String(e))
    }
  }

  const onClearOne = async (kind: CacheEntry['kind']) => {
    try {
      await clearCache(kind)
      flash(t('settings.cache.cleared', { kind }))
      refresh()
    } catch (e) {
      setError(String(e))
    }
  }

  const onClearAll = async () => {
    try {
      await clearCache('all')
      flash(t('settings.cache.allCleared'))
      refresh()
    } catch (e) {
      setError(String(e))
    }
  }

  const savePrefs = async (next: Preferences) => {
    setSaving(true)
    try {
      await setPreferences(next)
      setPrefs(next)
      flash(t('settings.saved'))
    } catch (e) {
      setError(String(e))
    } finally {
      setSaving(false)
    }
  }

  const onRestoreDismissed = async (id: number, title: string) => {
    try {
      await undismissRecommendation(id)
      setDismissed((prev) => prev?.filter((e) => e.id !== id) ?? null)
      flash(t('settings.dismissed.restored', { title }))
    } catch (e) {
      setError(String(e))
    }
  }

  const onOpenLogFolder = async () => {
    try {
      await openLogFolder()
    } catch (e) {
      setError(String(e))
    }
  }

  // Volver = pantalla anterior en la historia, no Home fijo. El user
  // llega a Ajustes desde el engranaje de cualquier vista y espera
  // regresar a esa misma vista. Si por alguna razón no hay historia
  // (deep-link inicial a /settings), caemos a Home.
  const goBack = () => {
    if (window.history.length > 1) nav(-1)
    else nav('/')
  }

  const hotkeys: Hotkey[] = [
    { key: 'Escape', hint: t('common.back'), run: goBack, ignoreInInput: false },
  ]
  useHotkeys(hotkeys, [])

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        <button
          onClick={goBack}
          className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-body hover:border-border-strong"
        >
          {t('common.back')}
        </button>
      </TopNav>

      <main className="mx-auto flex w-full max-w-[880px] flex-1 flex-col gap-10 px-8 py-8">
        <h1 className="text-[22px] font-semibold text-ink">{t('settings.title')}</h1>

        {error && (
          <div className="rounded-md border border-danger/40 bg-danger/10 p-4 text-[14px] text-danger">
            {error}
          </div>
        )}

        <Section title={t('settings.session.section')}>
          <div className="flex items-center justify-between gap-4">
            <div>
              <div className="text-[13px] text-dim">Letterboxd</div>
              <div className="text-[15px] text-ink">
                {username ?? <span className="text-muted">{t('settings.session.noSession')}</span>}
              </div>
            </div>
            {username && (
              <button
                onClick={onLogout}
                className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-[13px] text-body hover:border-danger hover:text-danger"
              >
                {t('nav.logout')}
              </button>
            )}
          </div>
        </Section>

        <Section title={t('settings.preferences.section')}>
          {prefs ? (
            <PreferencesEditor
              prefs={prefs}
              saving={saving}
              onSave={savePrefs}
            />
          ) : (
            <div className="text-[13px] text-muted">{t('common.loading')}</div>
          )}
        </Section>

        <Section
          title={t('settings.dismissed.section')}
          action={
            dismissed && dismissed.length > 0 ? (
              <span className="text-[11px] tabular-nums text-dim">
                {dismissed.length === 1
                  ? t('settings.dismissed.count1')
                  : t('settings.dismissed.count', { n: dismissed.length })}
              </span>
            ) : null
          }
        >
          {dismissed === null ? (
            <div className="text-[13px] text-muted">{t('common.loading')}</div>
          ) : dismissed.length === 0 ? (
            <p className="text-[13px] text-muted">
              {t('settings.dismissed.empty')}
            </p>
          ) : (
            <ul className="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4">
              {dismissed.map((d) => (
                <DismissedCard
                  key={d.id}
                  entry={d}
                  onRestore={() => onRestoreDismissed(d.id, d.title)}
                />
              ))}
            </ul>
          )}
        </Section>

        <Section
          title={t('settings.cache.section')}
          action={
            caches && caches.some((c) => c.exists) ? (
              <button
                onClick={onClearAll}
                className="focus-ring rounded-full border border-danger/40 px-3 py-1 text-[12px] text-danger hover:bg-danger/10"
              >
                {t('settings.cache.clearAll')}
              </button>
            ) : null
          }
        >
          {caches ? (
            <ul className="divide-y divide-hairline-soft">
              {caches.map((c) => (
                <CacheRow key={c.kind} entry={c} onClear={() => onClearOne(c.kind)} />
              ))}
            </ul>
          ) : (
            <div className="text-[13px] text-muted">{t('common.loading')}</div>
          )}
          <p className="mt-3 text-[11px] text-dim">
            {t('settings.cache.sessionHint')}
          </p>
        </Section>

        <Section title={t('settings.about.section')}>
          {log ? (
            <div className="flex flex-col gap-3">
              <div className="flex items-baseline justify-between gap-4">
                <span className="text-[13px] text-dim">
                  {t('settings.about.version')}
                </span>
                <span className="text-[14px] tabular-nums text-ink">
                  v{log.version}
                </span>
              </div>

              <div className="flex flex-col gap-1">
                <div className="flex items-center justify-between gap-4">
                  <span className="text-[13px] text-dim">
                    {t('settings.about.logFile')}
                  </span>
                  <button
                    onClick={onOpenLogFolder}
                    disabled={!log.enabled || (!log.dir && !log.file)}
                    className="focus-ring shrink-0 rounded-full border border-hairline px-3 py-1 text-[12px] text-body hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-hairline disabled:hover:text-body"
                  >
                    {t('settings.about.openLogFolder')}
                  </button>
                </div>
                {log.enabled ? (
                  <>
                    <code className="break-all text-[11px] text-muted">
                      {log.file ?? log.dir ?? ''}
                    </code>
                    <span className="text-[11px] text-dim">
                      {log.explicit_path
                        ? t('settings.about.logExplicit')
                        : t('settings.about.logRotation')}
                    </span>
                  </>
                ) : (
                  <span className="text-[11px] text-dim">
                    {t('settings.about.logDisabled')}
                  </span>
                )}
              </div>
            </div>
          ) : (
            <div className="text-[13px] text-muted">{t('common.loading')}</div>
          )}
        </Section>
      </main>

      <Toast visible={toast !== null}>
        <span>{toast}</span>
      </Toast>

      <HotkeyBar hotkeys={hotkeys.filter((h) => h.hint)} />
    </div>
  )
}

function Section({
  title,
  action,
  children,
}: {
  title: string
  action?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <section className="glass rounded-lg p-5">
      <header className="mb-4 flex items-baseline justify-between">
        <h2 className="text-[15px] font-semibold text-ink">{title}</h2>
        {action}
      </header>
      {children}
    </section>
  )
}

function PreferencesEditor({
  prefs,
  saving,
  onSave,
}: {
  prefs: Preferences
  saving: boolean
  onSave: (p: Preferences) => void
}) {
  const t = useT()
  const [rating, setRating] = useState(prefs.default_min_rating)
  const [langs, setLangs] = useState(prefs.subtitle_languages)
  const [ttl, setTtl] = useState(prefs.stream_cache_ttl_days)
  const [glass, setGlass] = useState(prefs.glass_opacity)
  const [player, setPlayer] = useState<'html' | 'vlc'>(prefs.default_player)
  // Idioma UI: se aplica en VIVO al cambiarlo (sin esperar a "Guardar")
  // para que el user vea el efecto inmediato. La persistencia va con
  // el submit — pero `setLocale` ya persiste por su cuenta (best-effort),
  // así que un cambio de idioma + cerrar sin guardar deja el idioma
  // aplicado. Es UX correcto: el idioma no es "un ajuste con estado
  // dirty", es un modo.
  const [ui, setUi] = useState<string>(prefs.ui_language ?? getLocale())

  const dirty =
    rating !== prefs.default_min_rating ||
    langs.trim() !== prefs.subtitle_languages.trim() ||
    ttl !== prefs.stream_cache_ttl_days ||
    glass !== prefs.glass_opacity ||
    player !== prefs.default_player

  // Preview en vivo del slider de liquid glass: aplicamos la variable
  // CSS al arrastrar aunque el usuario aún no haya pulsado "Guardar".
  // Si abandona sin guardar y vuelve, el `main.tsx` la re-establecerá
  // al valor persistido al recargar la app.
  useEffect(() => {
    applyGlassOpacity(glass)
  }, [glass])

  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
      <Field
        label={t('settings.ui.language')}
        hint={t('settings.ui.languageHint')}
      >
        <select
          value={ui}
          onChange={(e) => {
            const next = e.target.value
            setUi(next)
            // Aplica y persiste el idioma en vivo.
            void setLocale(next)
          }}
          className="focus-ring h-10 w-full rounded-md border border-hairline bg-surface px-3 text-[14px] text-ink"
        >
          {SUPPORTED_LOCALES.map((l) => (
            <option key={l} value={l}>
              {LOCALE_LABELS[l]}
            </option>
          ))}
        </select>
      </Field>

      <Field
        label={t('settings.recs.minRating')}
        hint={t('settings.recs.minRatingHint')}
      >
        <input
          type="number"
          min={0.5}
          max={5.0}
          step={0.5}
          value={rating}
          onChange={(e) => setRating(Number(e.target.value))}
          className="focus-ring h-10 w-full rounded-md border border-hairline bg-surface px-3 text-[14px] text-ink"
        />
      </Field>

      <Field
        label={t('settings.subs.languages')}
        hint={t('settings.subs.languagesHint')}
      >
        <input
          type="text"
          value={langs}
          onChange={(e) => setLangs(e.target.value)}
          spellCheck={false}
          className="focus-ring h-10 w-full rounded-md border border-hairline bg-surface px-3 text-[14px] text-ink"
        />
      </Field>

      <Field
        label={t('settings.cache.streamTtl')}
        hint={t('settings.streamCacheTtlHint')}
      >
        <input
          type="number"
          min={1}
          max={365}
          step={1}
          value={ttl}
          onChange={(e) => setTtl(Math.max(1, Number(e.target.value) || 1))}
          className="focus-ring h-10 w-full rounded-md border border-hairline bg-surface px-3 text-[14px] text-ink"
        />
      </Field>

      <Field
        label={`${t('settings.glass.opacity')} · ${glass}%`}
        hint={t('settings.glass.hint')}
      >
        <div className="flex items-center gap-3">
          <span className="text-[11px] text-dim">{t('settings.glass.crystal')}</span>
          <input
            type="range"
            min={0}
            max={100}
            step={5}
            value={glass}
            onChange={(e) => setGlass(Number(e.target.value))}
            className="focus-ring h-2 flex-1 cursor-pointer appearance-none rounded-full bg-surface accent-accent"
          />
          <span className="text-[11px] text-dim">{t('settings.glass.solid')}</span>
        </div>
      </Field>

      <Field
        label={t('settings.player.default')}
        hint={t('settings.player.hint')}
      >
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => setPlayer('html')}
            className={`focus-ring h-10 flex-1 rounded-md border text-[13px] transition-colors ${
              player === 'html'
                ? 'border-accent bg-accent/15 text-ink'
                : 'border-hairline bg-surface text-body hover:bg-surface-hi'
            }`}
          >
            {t('settings.player.html')}
          </button>
          <button
            type="button"
            onClick={() => setPlayer('vlc')}
            className={`focus-ring h-10 flex-1 rounded-md border text-[13px] transition-colors ${
              player === 'vlc'
                ? 'border-accent bg-accent/15 text-ink'
                : 'border-hairline bg-surface text-body hover:bg-surface-hi'
            }`}
          >
            {t('settings.player.vlc')}
          </button>
        </div>
      </Field>

      <div className="flex items-end justify-end sm:col-span-2">
        <button
          disabled={!dirty || saving}
          onClick={() =>
            onSave({
              default_min_rating: rating,
              subtitle_languages: langs.trim(),
              stream_cache_ttl_days: ttl,
              glass_opacity: glass,
              default_player: player,
              // Preservamos el idioma actual — no se toca desde el
              // botón "Guardar" (se persiste inline con setLocale).
              ui_language: ui,
            })
          }
          className="focus-ring h-10 rounded-full bg-accent px-5 text-[13px] font-semibold text-on-accent transition-colors hover:bg-accent-hover disabled:cursor-not-allowed disabled:bg-accent-disabled"
        >
          {saving ? t('common.loading') : t('common.save')}
        </button>
      </div>
    </div>
  )
}

function Field({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-[12px] uppercase tracking-wide text-dim">
        {label}
      </span>
      {children}
      {hint && <span className="text-[11px] text-muted">{hint}</span>}
    </label>
  )
}

function CacheRow({
  entry,
  onClear,
}: {
  entry: CacheEntry
  onClear: () => void
}) {
  const t = useT()
  // Traducir el label por `kind` (id estable del backend). Cae al
  // `entry.label` original (español) si no hay clave localizada.
  const localizedLabel = (() => {
    const key = `settings.cache.label.${entry.kind}`
    const raw = t(key)
    return raw === key ? entry.label : raw
  })()
  return (
    <li className="flex items-center gap-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="text-[14px] text-ink">{localizedLabel}</span>
          {entry.exists ? (
            <span className="text-[11px] tabular-nums text-good">
              {formatSize(entry.size_bytes)}
            </span>
          ) : (
            <span className="text-[11px] text-dim">{t('settings.cache.empty')}</span>
          )}
        </div>
        <div className="mt-0.5 truncate text-[11px] text-dim">{entry.path}</div>
        {entry.exists && (
          <div className="mt-0.5 text-[11px] text-muted">
            {t('settings.cache.updatedAgo', { age: formatAge(entry.modified_at, t) })}
          </div>
        )}
      </div>
      <button
        disabled={!entry.exists}
        onClick={onClear}
        className="focus-ring shrink-0 rounded-full border border-hairline px-3 py-1 text-[12px] text-body hover:border-danger hover:text-danger disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-hairline disabled:hover:text-body"
      >
        {t('settings.cache.clear')}
      </button>
    </li>
  )
}

function formatAge(unixSeconds: number, t: (k: string, v?: Record<string, string | number>) => string): string {
  if (!unixSeconds) return ''
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds)
  if (diff < 60) return t('time.secondsShort', { n: diff })
  if (diff < 3600) return t('time.minutesShort', { n: Math.floor(diff / 60) })
  if (diff < 86400) return t('time.hoursShort', { n: Math.floor(diff / 3600) })
  return t('time.daysShort', { n: Math.floor(diff / 86400) })
}

function DismissedCard({
  entry,
  onRestore,
}: {
  entry: DismissedEntry
  onRestore: () => void
}) {
  const t = useT()
  const src = tmdbPoster(entry.poster_path, 'w342')
  return (
    <li className="flex flex-col gap-2 animate-card-in">
      <div className="relative aspect-[2/3] w-full overflow-hidden rounded-poster bg-surface-hi">
        {src ? (
          <img
            src={src}
            alt={entry.title}
            loading="lazy"
            draggable={false}
            className="pointer-events-none h-full w-full select-none object-cover opacity-70"
          />
        ) : (
          <div className="pointer-events-none flex h-full w-full items-center justify-center px-2 text-center text-[11px] text-dim">
            {entry.title}
          </div>
        )}
        <div className="absolute inset-0 bg-black/30" />
      </div>
      <div className="flex min-w-0 items-baseline justify-between gap-2">
        <p className="truncate text-[12px] text-body">{entry.title}</p>
      </div>
      <button
        onClick={onRestore}
        className="focus-ring rounded-full border border-hairline px-3 py-1 text-[11px] text-body hover:border-accent hover:text-accent"
      >
        {t('home.restore')}
      </button>
    </li>
  )
}
