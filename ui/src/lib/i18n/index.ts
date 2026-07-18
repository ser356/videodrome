/**
 * i18n minimal para videodrome. Sin dependencias externas — la app
 * ya lleva mucha lógica custom y no queríamos meter react-i18next
 * por 150 keys.
 *
 * Design:
 *   - `SUPPORTED_LOCALES` fija los idiomas visibles en Settings.
 *   - Cada locale tiene un `Record<string, string>` en su fichero.
 *   - Keys en formato `namespace.key.dotted` — legibles a ojo.
 *   - Interpolación con `{{var}}`.
 *   - Fallback en cascada: `<current locale>` → `en` → `key` crudo.
 *
 * Init flow:
 *   1. `main.tsx` llama a `initLocale()` ANTES del render.
 *   2. `initLocale` pide `getPreferences()`:
 *      - si `ui_language` está poblado, lo usa
 *      - si no, detecta con `navigator.language`, guarda la
 *        elección en prefs (así la próxima ejecución no re-detecta)
 *   3. `setLocale(l)` cambia el diccionario activo, guarda prefs
 *      y emite `locale-changed` para que los componentes con
 *      `useT()` re-rendericen.
 *
 * Reactividad:
 *   - Componentes que necesiten re-render al cambiar idioma llaman
 *     `useT()`. Devuelve la función `t()` estable y se suscribe al
 *     evento `locale-changed`.
 *   - Fuera de React (helpers, side effects) usa `t()` directo.
 */
import { useEffect, useReducer } from 'react'
import { getPreferences, setPreferences } from '../api'
import { de } from './de'
import { en } from './en'
import { es } from './es'
import { fr } from './fr'
import { it } from './it'
import { pt } from './pt'

export type Locale = 'en' | 'es' | 'fr' | 'de' | 'it' | 'pt'

export const SUPPORTED_LOCALES: readonly Locale[] = ['en', 'es', 'fr', 'de', 'it', 'pt'] as const

/** Nombre nativo de cada idioma — se pinta en el dropdown de Ajustes. */
export const LOCALE_LABELS: Record<Locale, string> = {
  en: 'English',
  es: 'Español',
  fr: 'Français',
  de: 'Deutsch',
  it: 'Italiano',
  pt: 'Português',
}

/** Diccionarios por locale. `en` es la fuente canónica; los demás
 * heredan las claves ausentes via fallback en `t()`. FR/DE/IT/PT
 * arrancan como stubs y se completan progresivamente. */
const DICTIONARIES: Record<Locale, Record<string, string>> = { en, es, fr, de, it, pt }

const FALLBACK: Locale = 'en'

/** Locale activo. Inicializado a `en` para que las llamadas a `t()`
 * antes de `initLocale()` (imports colaterales) no crasheen. */
let currentLocale: Locale = FALLBACK
let currentDict: Record<string, string> = DICTIONARIES[FALLBACK]

const LOCALE_CHANGED = 'videodrome:locale-changed'

/**
 * Devuelve el locale activo. Solo lectura — usar `setLocale` para
 * cambiarlo.
 */
export function getLocale(): Locale {
  return currentLocale
}

/**
 * Cambia el locale activo. Persiste en prefs (best-effort) y notifica
 * a los `useT()` que re-rendericen. Si el locale no es soportado,
 * cae a `en`.
 */
export async function setLocale(l: string): Promise<void> {
  const norm = normalizeLocale(l)
  currentLocale = norm
  currentDict = DICTIONARIES[norm]
  window.dispatchEvent(new Event(LOCALE_CHANGED))
  // Persistir en prefs sin bloquear. Errores silenciosos — no
  // queremos que un fallo de IPC rompa el cambio de idioma en la UI.
  try {
    const p = await getPreferences()
    if (p.ui_language !== norm) {
      await setPreferences({ ...p, ui_language: norm })
    }
  } catch {
    /* best-effort */
  }
}

/**
 * Normaliza cualquier string a un locale soportado. Acepta
 * "es-ES", "es_ES", "ES", "es" → `"es"`. Fallback `en`.
 */
export function normalizeLocale(raw: string): Locale {
  const base = raw.trim().toLowerCase().split(/[-_]/)[0]
  return (SUPPORTED_LOCALES as readonly string[]).includes(base) ? (base as Locale) : FALLBACK
}

/**
 * Init al arrancar. Prioridad:
 *   1. `Preferences.ui_language` si está set.
 *   2. `navigator.language` normalizado.
 *   3. Fallback `en`.
 *
 * Si detectamos (2) y no había pref, guardamos la elección. Así el
 * user puede sobrescribir después desde Ajustes sin re-detectar.
 */
export async function initLocale(): Promise<void> {
  try {
    const prefs = await getPreferences()
    if (prefs.ui_language) {
      const norm = normalizeLocale(prefs.ui_language)
      currentLocale = norm
      currentDict = DICTIONARIES[norm]
      return
    }
    // Sin pref: detectar del navegador. WKWebView/WebView2 devuelven
    // la locale del sistema en `navigator.language` (formato BCP47).
    const detected = normalizeLocale(navigator.language ?? '')
    currentLocale = detected
    currentDict = DICTIONARIES[detected]
    // Persistir la detección para que el user vea qué idioma tiene
    // en Ajustes y pueda cambiarlo. Si falla, no importa.
    try {
      await setPreferences({ ...prefs, ui_language: detected })
    } catch {
      /* best-effort */
    }
  } catch {
    // Preferencias no accesibles (no-Tauri, backend no arrancado).
    // Detectamos del navegador y ya está.
    const detected = normalizeLocale(navigator.language ?? '')
    currentLocale = detected
    currentDict = DICTIONARIES[detected]
  }
}

/**
 * Traduce una key. Sustituye `{{var}}` por `vars.var` cuando `vars`
 * está presente. Fallback: locale actual → `en` → key literal
 * (útil para no explotar en desarrollo cuando falta una traducción).
 */
export function t(key: string, vars?: Record<string, string | number>): string {
  let s = currentDict[key] ?? DICTIONARIES[FALLBACK][key] ?? key
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      s = s.replaceAll(`{{${k}}}`, String(v))
    }
  }
  return s
}

/**
 * Hook. Devuelve la función `t()` y fuerza re-render en cada cambio
 * de locale. La función devuelta es estable entre renders — no
 * romper deps de useEffect / useCallback.
 */
export function useT(): typeof t {
  const [, force] = useReducer((x: number) => x + 1, 0)
  useEffect(() => {
    const handler = () => force()
    window.addEventListener(LOCALE_CHANGED, handler)
    return () => window.removeEventListener(LOCALE_CHANGED, handler)
  }, [])
  return t
}

/**
 * Fusiona el locale de la UI con la lista `subtitle_languages` de
 * preferencias, poniendo el UI language en primera posición y
 * deduplicando (case-insensitive). Devuelve una cadena coma-separada
 * lista para pasar al parámetro `languages` de OpenSubtitles.
 *
 * Ejemplos:
 *   ui="es", prefs="en,es"  → "es,en"
 *   ui="es", prefs=""        → "es"
 *   ui="fr", prefs="en,fr,de"→ "fr,en,de"
 */
export function mergeSubtitleLangs(ui: string, prefsLangs: string): string {
  const out: string[] = []
  const seen = new Set<string>()
  const push = (raw: string) => {
    const v = raw.trim().toLowerCase()
    if (!v || seen.has(v)) return
    seen.add(v)
    out.push(v)
  }
  push(ui)
  for (const part of prefsLangs.split(',')) push(part)
  return out.join(',')
}
