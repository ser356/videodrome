import { describe, expect, it } from 'vitest'
import { applyGlassOpacity, applySkin, SKINS } from './theme'

describe('applyGlassOpacity', () => {
  it('writes --glass-opaque scaled to 0..1', () => {
    applyGlassOpacity(50)
    expect(document.documentElement.style.getPropertyValue('--glass-opaque')).toBe('0.500')
    applyGlassOpacity(0)
    expect(document.documentElement.style.getPropertyValue('--glass-opaque')).toBe('0.000')
    applyGlassOpacity(100)
    expect(document.documentElement.style.getPropertyValue('--glass-opaque')).toBe('1.000')
  })

  it('clamps values outside 0..100', () => {
    applyGlassOpacity(-50)
    expect(document.documentElement.style.getPropertyValue('--glass-opaque')).toBe('0.000')
    applyGlassOpacity(500)
    expect(document.documentElement.style.getPropertyValue('--glass-opaque')).toBe('1.000')
  })

  it('is idempotent', () => {
    applyGlassOpacity(25)
    applyGlassOpacity(25)
    expect(document.documentElement.style.getPropertyValue('--glass-opaque')).toBe('0.250')
  })
})

describe('applySkin', () => {
  it('writes the accent variables of the requested skin', () => {
    applySkin('noir')
    const root = document.documentElement.style
    expect(root.getPropertyValue('--color-accent')).toBe('#e11d48')
    expect(root.getPropertyValue('--color-canvas')).toBe('#080608')
  })

  it('falls back to videodrome for unknown id', () => {
    applySkin('this-does-not-exist')
    const root = document.documentElement.style
    expect(root.getPropertyValue('--color-accent')).toBe('#ff8000')
  })

  it('falls back for null/undefined', () => {
    applySkin(null)
    expect(document.documentElement.style.getPropertyValue('--color-accent')).toBe('#ff8000')
    applySkin(undefined)
    expect(document.documentElement.style.getPropertyValue('--color-accent')).toBe('#ff8000')
  })

  it('exports 6 skins with valid ids', () => {
    expect(SKINS.length).toBeGreaterThanOrEqual(6)
    const ids = SKINS.map((s) => s.id)
    expect(new Set(ids).size).toBe(SKINS.length) // sin duplicados
    for (const s of SKINS) {
      expect(s.vars['--color-accent']).toMatch(/^#[0-9a-f]{6}$/i)
      expect(s.vars['--color-canvas']).toMatch(/^#[0-9a-f]{6}$/i)
    }
  })
})
