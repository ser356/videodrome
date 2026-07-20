/**
 * Aplica el valor de "liquid glass opacity" (0..=100) a la variable CSS
 * `--glass-opaque` en `:root`. Todas las utilidades `.glass`,
 * `.glass-strong` y `.popover` lo leen para interpolar entre el look
 * translúcido por defecto (0) y superficies casi sólidas (100).
 */
export function applyGlassOpacity(value: number) {
  const clamped = Math.min(100, Math.max(0, value))
  document.documentElement.style.setProperty(
    '--glass-opaque',
    (clamped / 100).toFixed(3),
  )
}

/**
 * Skins: presets de apariencia que remapean un puñado de variables CSS
 * ancla (canvas, accent, on-accent, y el color de las selecciones).
 * El resto del sistema visual (glass, hairlines, muted…) queda igual
 * — hemos hecho suficientemente neutro el paladar como para que un
 * cambio de acento no rompa el look and feel.
 *
 * `id` es lo que se guarda en `preferences.skin`. `label` es
 * user-facing (traducible via i18n si hiciera falta; de momento
 * literal en la UI para no crear 5×6 = 30 claves).
 *
 * Añadir una skin: mete un preset aquí, se pinta automáticamente en
 * Ajustes (el editor recorre `SKINS` y renderiza un swatch por cada
 * uno). No hace falta tocar CSS de base — las variables se aplican
 * a `documentElement.style` y ganan sobre las de `@theme` porque son
 * inline styles.
 */
export interface Skin {
  id: string
  label: string
  vars: Record<string, string>
}

export const SKINS: readonly Skin[] = [
  {
    id: 'videodrome',
    label: 'Videodrome',
    vars: {
      '--color-canvas': '#0e0f13',
      '--color-accent': '#ff8000',
      '--color-accent-hover': '#ff9d33',
      '--color-accent-disabled': 'rgba(255, 128, 0, 0.35)',
      '--color-on-accent': '#14181c',
    },
  },
  {
    id: 'noir',
    label: 'Cinema noir',
    vars: {
      '--color-canvas': '#080608',
      '--color-accent': '#e11d48',
      '--color-accent-hover': '#f43f5e',
      '--color-accent-disabled': 'rgba(225, 29, 72, 0.35)',
      '--color-on-accent': '#fff5f5',
    },
  },
  {
    id: 'tokyo',
    label: 'Neo Tokyo',
    vars: {
      '--color-canvas': '#0a0f1e',
      '--color-accent': '#22d3ee',
      '--color-accent-hover': '#67e8f9',
      '--color-accent-disabled': 'rgba(34, 211, 238, 0.35)',
      '--color-on-accent': '#0a0f1e',
    },
  },
  {
    id: 'vapor',
    label: 'Vaporwave',
    vars: {
      '--color-canvas': '#180a24',
      '--color-accent': '#f472b6',
      '--color-accent-hover': '#f9a8d4',
      '--color-accent-disabled': 'rgba(244, 114, 182, 0.35)',
      '--color-on-accent': '#1b0836',
    },
  },
  {
    id: 'sepia',
    label: 'Sepia',
    vars: {
      '--color-canvas': '#1a140f',
      '--color-accent': '#f0b429',
      '--color-accent-hover': '#f7c948',
      '--color-accent-disabled': 'rgba(240, 180, 41, 0.35)',
      '--color-on-accent': '#1a1004',
    },
  },
  {
    id: 'forest',
    label: 'Forest',
    vars: {
      '--color-canvas': '#0c1410',
      '--color-accent': '#34d399',
      '--color-accent-hover': '#6ee7b7',
      '--color-accent-disabled': 'rgba(52, 211, 153, 0.35)',
      '--color-on-accent': '#052018',
    },
  },
] as const

const DEFAULT_SKIN_ID = 'videodrome'

/**
 * Aplica una skin escribiendo su set de variables CSS a
 * `documentElement`. Idempotente. Un id desconocido se resuelve al
 * default (evita quedarnos con las vars de la skin previa colgando
 * si un valor persistido queda obsoleto tras un downgrade).
 */
export function applySkin(id: string | null | undefined) {
  const skin =
    SKINS.find((s) => s.id === id) ??
    SKINS.find((s) => s.id === DEFAULT_SKIN_ID) ??
    SKINS[0]
  const root = document.documentElement
  for (const [k, v] of Object.entries(skin.vars)) {
    root.style.setProperty(k, v)
  }
}
