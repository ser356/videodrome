import { describe, expect, it } from 'vitest'
import { canGoDirect } from './useHlsAttach'
import type { MediaInfo } from '../../lib/api'

function media(
  streams: Array<{ kind: 'video' | 'audio' | 'subtitle' | 'other'; codec: string }>,
  directPlayable = true,
): MediaInfo {
  return {
    duration_seconds: 60,
    container: 'mov',
    direct_playable: directPlayable,
    streams: streams.map((s, index) => ({
      index,
      kind: s.kind,
      codec: s.codec,
      language: null,
      title: null,
      width: null,
      height: null,
    })),
  }
}

describe('canGoDirect', () => {
  it('returns false when directFailed is true (runtime fallback)', () => {
    const m = media([{ kind: 'video', codec: 'h264' }])
    expect(canGoDirect(m, true)).toBe(false)
  })

  it('returns false when media is null', () => {
    expect(canGoDirect(null, false)).toBe(false)
  })

  it('returns false when backend marks direct_playable=false', () => {
    const m = media([{ kind: 'video', codec: 'h264' }], false)
    expect(canGoDirect(m, false)).toBe(false)
  })

  it('returns true for H.264 with direct_playable=true', () => {
    const m = media([{ kind: 'video', codec: 'h264' }])
    expect(canGoDirect(m, false)).toBe(true)
  })

  it('returns true when there is no video stream (audio-only)', () => {
    const m = media([{ kind: 'audio', codec: 'aac' }])
    expect(canGoDirect(m, false)).toBe(true)
  })

  it('probes hvc1 canPlayType for HEVC', () => {
    // jsdom devuelve '' de canPlayType — la rama HEVC se resuelve
    // a false porque el check es strict `=== 'probably'`.
    const m = media([{ kind: 'video', codec: 'hevc' }])
    expect(canGoDirect(m, false)).toBe(false)
  })

  it.each(['hevc', 'h265', 'h.265'])(
    'treats %s codec as HEVC (case insensitive)',
    (codec) => {
      const m = media([{ kind: 'video', codec }])
      expect(canGoDirect(m, false)).toBe(false)
    },
  )

  it('is case-insensitive on codec', () => {
    const m = media([{ kind: 'video', codec: 'HEVC' }])
    expect(canGoDirect(m, false)).toBe(false)
  })
})
