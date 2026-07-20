import { describe, expect, it } from 'vitest'
import { audioFlag, formatSize, hlsUrl, tmdbBackdrop, tmdbPoster } from './api'

describe('formatSize', () => {
  it('returns bytes for small numbers', () => {
    expect(formatSize(0)).toBe('0.00 B')
    // 500 usa 1 decimal (n>=10 sale del ramo, pero 500 B < 1024 y >= 10)
    expect(formatSize(500)).toBe('500.0 B')
  })

  it('scales through KB / MB / GB / TB', () => {
    expect(formatSize(1024)).toBe('1.00 KB')
    expect(formatSize(1024 * 1024)).toBe('1.00 MB')
    expect(formatSize(1024 * 1024 * 1024)).toBe('1.00 GB')
    expect(formatSize(1024 ** 4)).toBe('1.00 TB')
  })

  it('uses 2 decimals under 10 and 1 above', () => {
    expect(formatSize(1024 * 2)).toBe('2.00 KB')
    expect(formatSize(1024 * 15)).toBe('15.0 KB')
    expect(formatSize(1024 * 1024 * 12.5)).toBe('12.5 MB')
  })

  it('caps at TB and does not go higher', () => {
    // 1024 TB = 1 PB — nuestro formateador se queda en TB.
    const val = formatSize(1024 ** 5)
    expect(val).toMatch(/TB$/)
  })
})

describe('tmdbPoster', () => {
  it('returns null for null path', () => {
    expect(tmdbPoster(null)).toBeNull()
  })

  it('prepends TMDB CDN with default w500', () => {
    expect(tmdbPoster('/abc.jpg')).toBe('https://image.tmdb.org/t/p/w500/abc.jpg')
  })

  it('honors the size parameter', () => {
    expect(tmdbPoster('/abc.jpg', 'w780')).toBe('https://image.tmdb.org/t/p/w780/abc.jpg')
    expect(tmdbPoster('/abc.jpg', 'w342')).toBe('https://image.tmdb.org/t/p/w342/abc.jpg')
  })

  it('passes absolute URLs through unchanged (Cinemeta fallback)', () => {
    const url = 'https://images.metahub.space/poster/medium/tt0000000/img'
    expect(tmdbPoster(url)).toBe(url)
    expect(tmdbPoster('http://example.com/x.jpg')).toBe('http://example.com/x.jpg')
  })
})

describe('tmdbBackdrop', () => {
  it('returns null for null path', () => {
    expect(tmdbBackdrop(null)).toBeNull()
  })

  it('uses w1280 by default', () => {
    expect(tmdbBackdrop('/b.jpg')).toBe('https://image.tmdb.org/t/p/w1280/b.jpg')
  })

  it('supports original size', () => {
    expect(tmdbBackdrop('/b.jpg', 'original')).toBe(
      'https://image.tmdb.org/t/p/original/b.jpg',
    )
  })

  it('passes absolute URLs through', () => {
    const url = 'https://cdn.example.com/backdrop.jpg'
    expect(tmdbBackdrop(url)).toBe(url)
  })
})

describe('hlsUrl', () => {
  it('replaces trailing /video with /hls/playlist.m3u8', () => {
    expect(hlsUrl('http://127.0.0.1:1234/stream/abc/video')).toBe(
      'http://127.0.0.1:1234/stream/abc/hls/playlist.m3u8',
    )
  })

  it('appends /hls/playlist.m3u8 when input lacks /video suffix', () => {
    // El regex es `/video$` — sin él, la URL queda como base + suffix.
    expect(hlsUrl('http://127.0.0.1/x')).toBe('http://127.0.0.1/x/hls/playlist.m3u8')
  })
})

describe('audioFlag', () => {
  it('returns known flag for common ISO 639-1', () => {
    expect(audioFlag('en').flag).toBe('🇬🇧')
    expect(audioFlag('es').label).toBe('ES')
    expect(audioFlag('ja').flag).toBe('🇯🇵')
  })

  it('returns special marker for orig / multi / dub / unknown', () => {
    expect(audioFlag('multi').flag).toBe('🌐')
    expect(audioFlag('orig').flag).toBe('🎬')
    expect(audioFlag('dub').flag).toBe('💬')
    expect(audioFlag('unknown').flag).toBe('·')
  })

  it('falls back to · + uppercase code for unknown languages', () => {
    const r = audioFlag('sk')
    expect(r.flag).toBe('·')
    expect(r.label).toBe('SK')
  })
})
