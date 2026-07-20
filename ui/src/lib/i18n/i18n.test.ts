import { describe, expect, it } from 'vitest'
import { mergeSubtitleLangs, normalizeLocale, SUPPORTED_LOCALES } from './index'

describe('normalizeLocale', () => {
  it('returns supported locales as-is when lowercased', () => {
    for (const loc of SUPPORTED_LOCALES) {
      expect(normalizeLocale(loc)).toBe(loc)
    }
  })

  it('strips region suffixes', () => {
    expect(normalizeLocale('es-ES')).toBe('es')
    expect(normalizeLocale('en_US')).toBe('en')
    expect(normalizeLocale('pt-BR')).toBe('pt')
    expect(normalizeLocale('fr-CA')).toBe('fr')
  })

  it('is case-insensitive', () => {
    expect(normalizeLocale('ES')).toBe('es')
    expect(normalizeLocale('EN_US')).toBe('en')
  })

  it('trims whitespace', () => {
    expect(normalizeLocale('  es  ')).toBe('es')
  })

  it('falls back to en for unknown locales', () => {
    expect(normalizeLocale('ja')).toBe('en')
    expect(normalizeLocale('xx-XX')).toBe('en')
    expect(normalizeLocale('')).toBe('en')
  })
})

describe('mergeSubtitleLangs', () => {
  it('puts UI locale first', () => {
    expect(mergeSubtitleLangs('es', 'en,fr')).toBe('es,en,fr')
    expect(mergeSubtitleLangs('fr', 'en,fr,de')).toBe('fr,en,de')
  })

  it('deduplicates case-insensitively', () => {
    expect(mergeSubtitleLangs('es', 'ES,en')).toBe('es,en')
    expect(mergeSubtitleLangs('EN', 'en,fr')).toBe('en,fr')
  })

  it('emits only UI locale when prefs list is empty', () => {
    expect(mergeSubtitleLangs('es', '')).toBe('es')
  })

  it('emits only UI locale when prefs list is whitespace', () => {
    expect(mergeSubtitleLangs('es', '  ,  ,  ')).toBe('es')
  })

  it('trims each entry', () => {
    expect(mergeSubtitleLangs('es', ' en , fr ')).toBe('es,en,fr')
  })
})
