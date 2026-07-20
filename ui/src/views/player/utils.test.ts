import { describe, expect, it } from 'vitest'
import {
  ffmpegInstallHint,
  formatEta,
  formatSpeed,
  formatTime,
  isBitmapSubCodec,
  languageLabel,
} from './utils'

describe('formatTime', () => {
  it('formats < 1h as m:ss', () => {
    expect(formatTime(0)).toBe('0:00')
    expect(formatTime(5)).toBe('0:05')
    expect(formatTime(65)).toBe('1:05')
    expect(formatTime(225)).toBe('3:45')
  })

  it('formats >= 1h as h:mm:ss', () => {
    expect(formatTime(3600)).toBe('1:00:00')
    expect(formatTime(3665)).toBe('1:01:05')
    expect(formatTime(7325)).toBe('2:02:05')
  })

  it('handles fractional seconds by flooring', () => {
    expect(formatTime(59.9)).toBe('0:59')
    expect(formatTime(3599.7)).toBe('59:59')
  })

  it('returns 0:00 for negative or non-finite', () => {
    expect(formatTime(-5)).toBe('0:00')
    expect(formatTime(Number.NaN)).toBe('0:00')
    expect(formatTime(Number.POSITIVE_INFINITY)).toBe('0:00')
  })
})

describe('formatSpeed', () => {
  it('returns 0 B/s for zero/negative', () => {
    expect(formatSpeed(0)).toBe('0 B/s')
    expect(formatSpeed(-1)).toBe('0 B/s')
  })

  it('renders KiB/s under 1 MiB', () => {
    expect(formatSpeed(1024)).toBe('1 KiB/s')
    expect(formatSpeed(1024 * 512)).toBe('512 KiB/s')
  })

  it('renders MiB/s in [1MiB, 1GiB)', () => {
    expect(formatSpeed(1024 * 1024)).toBe('1.00 MiB/s')
    expect(formatSpeed(1024 * 1024 * 5.5)).toBe('5.50 MiB/s')
  })

  it('renders GiB/s for >= 1GiB', () => {
    expect(formatSpeed(1024 * 1024 * 1024)).toBe('1.00 GiB/s')
    expect(formatSpeed(1024 * 1024 * 1024 * 2.25)).toBe('2.25 GiB/s')
  })
})

describe('formatEta', () => {
  it('returns — for non-positive or non-finite', () => {
    expect(formatEta(0)).toBe('—')
    expect(formatEta(-10)).toBe('—')
    expect(formatEta(Number.NaN)).toBe('—')
    expect(formatEta(Number.POSITIVE_INFINITY)).toBe('—')
  })

  it('renders seconds under 1 min', () => {
    expect(formatEta(1)).toBe('1s')
    expect(formatEta(45)).toBe('45s')
  })

  it('renders m + s with zero-padded seconds under 1 hour', () => {
    expect(formatEta(60)).toBe('1m 00s')
    expect(formatEta(125)).toBe('2m 05s')
  })

  it('renders h + m for >= 1 hour', () => {
    expect(formatEta(3600)).toBe('1h 00m')
    expect(formatEta(3661)).toBe('1h 01m')
  })
})

describe('isBitmapSubCodec', () => {
  it.each([
    'hdmv_pgs_subtitle',
    'pgssub',
    'pgs',
    'dvd_subtitle',
    'dvdsub',
    'dvb_subtitle',
    'dvbsub',
    'xsub',
  ])('%s is bitmap', (codec) => {
    expect(isBitmapSubCodec(codec)).toBe(true)
  })

  it.each(['subrip', 'srt', 'ass', 'ssa', 'webvtt', 'mov_text', ''])(
    '%s is not bitmap',
    (codec) => {
      expect(isBitmapSubCodec(codec)).toBe(false)
    },
  )
})

describe('languageLabel', () => {
  it('returns em dash for empty code', () => {
    expect(languageLabel('')).toBe('—')
  })

  it('capitalizes the Intl.DisplayNames result for known codes', () => {
    // Node/jsdom expone Intl.DisplayNames — el resultado exacto en
    // español para 'en' puede ser "inglés" o "Inglés" según ICU;
    // afirmamos que empieza en mayúscula y no coincide con el código
    // en bruto.
    const label = languageLabel('en')
    expect(label).not.toBe('EN')
    expect(label.charAt(0)).toBe(label.charAt(0).toUpperCase())
  })

  it('never returns the raw lowercase code unchanged', () => {
    // Sea cual sea la rama (Intl.DisplayNames traduce + capitaliza,
    // o fallback a mayúsculas), el primer carácter debe estar en
    // mayúscula. Nunca queremos ver "en" o "zz-xx" en la UI.
    for (const code of ['en', 'es', 'fr', 'de', 'it', 'pt', 'ja', 'zz']) {
      const label = languageLabel(code)
      expect(label).not.toBe(code)
      expect(label.charAt(0)).toBe(label.charAt(0).toUpperCase())
    }
  })
})

describe('ffmpegInstallHint', () => {
  const t = (k: string) => k

  const setUA = (ua: string) => {
    Object.defineProperty(window.navigator, 'userAgent', {
      value: ua,
      configurable: true,
    })
  }

  it('returns Windows hint for Windows UA', () => {
    setUA('Mozilla/5.0 (Windows NT 10.0; Win64; x64)')
    expect(ffmpegInstallHint(t)).toBe('player.ffmpegHintWindows')
  })

  it('returns Mac hint for macOS UA', () => {
    setUA('Mozilla/5.0 (Macintosh; Intel Mac OS X 13_0)')
    expect(ffmpegInstallHint(t)).toBe('player.ffmpegHintMac')
  })

  it('returns Linux hint for Linux UA', () => {
    setUA('Mozilla/5.0 (X11; Linux x86_64)')
    expect(ffmpegInstallHint(t)).toBe('player.ffmpegHintLinux')
  })

  it('returns generic hint for unknown UA', () => {
    setUA('SomeUnknownAgent/1.0')
    expect(ffmpegInstallHint(t)).toBe('player.ffmpegHintGeneric')
  })
})
