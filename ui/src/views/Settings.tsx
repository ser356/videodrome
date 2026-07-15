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
  logout,
  setPreferences,
  type CacheEntry,
  type Preferences,
} from '../lib/api'
import { useHotkeys, type Hotkey } from '../lib/hotkeys'

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

  const [username, setUsername] = useState<string | null>(null)
  const [prefs, setPrefs] = useState<Preferences | null>(null)
  const [caches, setCaches] = useState<CacheEntry[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setError('Esta vista requiere la app de escritorio (Tauri).')
      return
    }
    try {
      const [hasS, list, p] = await Promise.all([
        hasSession(),
        cacheInfo(),
        getPreferences(),
      ])
      setCaches(list)
      setPrefs(p)
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
      flash('Sesión cerrada.')
      nav('/login')
    } catch (e) {
      setError(String(e))
    }
  }

  const onClearOne = async (kind: CacheEntry['kind']) => {
    try {
      await clearCache(kind)
      flash(`Caché "${kind}" borrada.`)
      refresh()
    } catch (e) {
      setError(String(e))
    }
  }

  const onClearAll = async () => {
    try {
      await clearCache('all')
      flash('Todas las cachés borradas.')
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
      flash('Preferencias guardadas.')
    } catch (e) {
      setError(String(e))
    } finally {
      setSaving(false)
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
    { key: 'Escape', hint: 'Volver', run: goBack, ignoreInInput: false },
  ]
  useHotkeys(hotkeys, [])

  return (
    <div className="flex min-h-[100dvh] flex-col bg-canvas">
      <TopNav>
        <button
          onClick={goBack}
          className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-body hover:border-border-strong"
        >
          Volver
        </button>
      </TopNav>

      <main className="mx-auto flex w-full max-w-[880px] flex-1 flex-col gap-10 px-8 py-8">
        <h1 className="text-[22px] font-semibold text-ink">Ajustes</h1>

        {error && (
          <div className="rounded-md border border-danger/40 bg-danger/10 p-4 text-[14px] text-danger">
            {error}
          </div>
        )}

        <Section title="Sesión">
          <div className="flex items-center justify-between gap-4">
            <div>
              <div className="text-[13px] text-dim">Letterboxd</div>
              <div className="text-[15px] text-ink">
                {username ?? <span className="text-muted">Sin sesión</span>}
              </div>
            </div>
            {username && (
              <button
                onClick={onLogout}
                className="focus-ring rounded-full border border-hairline px-4 py-1.5 text-[13px] text-body hover:border-danger hover:text-danger"
              >
                Cerrar sesión
              </button>
            )}
          </div>
        </Section>

        <Section title="Preferencias">
          {prefs ? (
            <PreferencesEditor
              prefs={prefs}
              saving={saving}
              onSave={savePrefs}
            />
          ) : (
            <div className="text-[13px] text-muted">Cargando…</div>
          )}
        </Section>

        <Section
          title="Caché"
          action={
            caches && caches.some((c) => c.exists) ? (
              <button
                onClick={onClearAll}
                className="focus-ring rounded-full border border-danger/40 px-3 py-1 text-[12px] text-danger hover:bg-danger/10"
              >
                Borrar todo
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
            <div className="text-[13px] text-muted">Cargando…</div>
          )}
          <p className="mt-3 text-[11px] text-dim">
            La sesión no se borra desde aquí. Usa "Cerrar sesión" arriba.
          </p>
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
  const [rating, setRating] = useState(prefs.default_min_rating)
  const [count, setCount] = useState(prefs.default_count)
  const [langs, setLangs] = useState(prefs.subtitle_languages)
  const [ttl, setTtl] = useState(prefs.stream_cache_ttl_days)

  const dirty =
    rating !== prefs.default_min_rating ||
    count !== prefs.default_count ||
    langs.trim() !== prefs.subtitle_languages.trim() ||
    ttl !== prefs.stream_cache_ttl_days

  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
      <Field
        label="Rating mínimo por defecto"
        hint="Umbral inicial de la vista Cartelera (0.5 – 5.0)."
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
        label="Número de recomendaciones"
        hint="Cantidad inicial de posters en Cartelera."
      >
        <input
          type="number"
          min={5}
          max={100}
          step={5}
          value={count}
          onChange={(e) => setCount(Number(e.target.value))}
          className="focus-ring h-10 w-full rounded-md border border-hairline bg-surface px-3 text-[14px] text-ink"
        />
      </Field>

      <Field
        label="Idiomas de subtítulos"
        hint='Códigos ISO 639-1 separados por coma. Ej: "es,en,fr".'
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
        label="TTL caché de streams (días)"
        hint="Purga al arrancar: pelis no reproducidas en N días se borran del disco. Entre 1 y 365."
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

      <div className="flex items-end justify-end sm:col-span-2">
        <button
          disabled={!dirty || saving}
          onClick={() =>
            onSave({
              default_min_rating: rating,
              default_count: count,
              subtitle_languages: langs.trim(),
              stream_cache_ttl_days: ttl,
            })
          }
          className="focus-ring h-10 rounded-full bg-accent px-5 text-[13px] font-semibold text-on-accent transition-colors hover:bg-accent-hover disabled:cursor-not-allowed disabled:bg-accent-disabled"
        >
          {saving ? 'Guardando…' : 'Guardar cambios'}
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
  return (
    <li className="flex items-center gap-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="text-[14px] text-ink">{entry.label}</span>
          {entry.exists ? (
            <span className="text-[11px] tabular-nums text-good">
              {formatSize(entry.size_bytes)}
            </span>
          ) : (
            <span className="text-[11px] text-dim">vacía</span>
          )}
        </div>
        <div className="mt-0.5 truncate text-[11px] text-dim">{entry.path}</div>
        {entry.exists && (
          <div className="mt-0.5 text-[11px] text-muted">
            Actualizada {formatAge(entry.modified_at)}
          </div>
        )}
      </div>
      <button
        disabled={!entry.exists}
        onClick={onClear}
        className="focus-ring shrink-0 rounded-full border border-hairline px-3 py-1 text-[12px] text-body hover:border-danger hover:text-danger disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-hairline disabled:hover:text-body"
      >
        Borrar
      </button>
    </li>
  )
}

function formatAge(unixSeconds: number): string {
  if (!unixSeconds) return ''
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds)
  if (diff < 60) return `hace ${diff}s`
  if (diff < 3600) return `hace ${Math.floor(diff / 60)}min`
  if (diff < 86400) return `hace ${Math.floor(diff / 3600)}h`
  return `hace ${Math.floor(diff / 86400)}d`
}
