/** Formatea segundos como "12s", "3m 45s" o "1h 12m". Usado por la
 * `SeekBar` para el tooltip de hover. */
export function formatTime(s: number): string {
  if (!isFinite(s) || s < 0) return '0:00'
  const hh = Math.floor(s / 3600)
  const mm = Math.floor((s % 3600) / 60)
  const ss = Math.floor(s % 60)
  const pad = (n: number) => n.toString().padStart(2, '0')
  return hh > 0 ? `${hh}:${pad(mm)}:${pad(ss)}` : `${mm}:${pad(ss)}`
}

/** Instrucción de instalación de ffmpeg específica del SO en el que
 * corre la WebView. Se usa en el mensaje de error del player cuando
 * `probeStream` falla por falta del binario. Detección por
 * `navigator.userAgent` — Tauri no expone el OS al frontend sin el
 * plugin `@tauri-apps/plugin-os`, y este helper es suficiente para
 * los tres SO que soportamos. */
export function ffmpegInstallHint(
  t: (k: string, v?: Record<string, string | number>) => string,
): string {
  const ua = navigator.userAgent
  if (ua.includes('Windows')) {
    return t('player.ffmpegHintWindows')
  }
  if (ua.includes('Mac OS X') || ua.includes('Macintosh')) {
    return t('player.ffmpegHintMac')
  }
  if (ua.includes('Linux')) {
    return t('player.ffmpegHintLinux')
  }
  return t('player.ffmpegHintGeneric')
}

/** Detecta codecs de subtítulos de imagen (bitmap) que ffmpeg no
 * puede convertir a WebVTT sin OCR. La UI oculta estas pistas del
 * panel — si el user las eligiera, el endpoint `/subs/embedded/N.vtt`
 * devolvería HTTP 415 y el error saldría por consola. Mejor no
 * ofrecerlas. Lista basada en los codecs de subs de ffmpeg. */
export function isBitmapSubCodec(codec: string): boolean {
  return (
    codec === 'hdmv_pgs_subtitle' ||
    codec === 'pgssub' ||
    codec === 'pgs' ||
    codec === 'dvd_subtitle' ||
    codec === 'dvdsub' ||
    codec === 'dvb_subtitle' ||
    codec === 'dvbsub' ||
    codec === 'xsub'
  )
}

/** Nombre legible del idioma según código ISO 639-1 (`"es"`, `"en"`,
 * `"pt-BR"`…). Usa `Intl.DisplayNames` del navegador con fallback al
 * código en mayúsculas si el runtime no lo conoce. */
export function languageLabel(code: string): string {
  if (!code) return '—'
  try {
    const dn = new Intl.DisplayNames(['es'], { type: 'language' })
    const name = dn.of(code)
    if (name && name !== code) {
      // Capitaliza primera letra ("español" → "Español").
      return name.charAt(0).toUpperCase() + name.slice(1)
    }
  } catch {
    // Runtime sin Intl.DisplayNames o código desconocido → fallback.
  }
  return code.toUpperCase()
}

/**
 * Formatea bytes/s en la unidad adecuada (B, KiB, MiB, GiB por
 * segundo). Usado por el StremioLoader durante arranque y por el
 * mensaje de `swarm_stalled` para que el user vea números humanos.
 */
export function formatSpeed(bps: number): string {
  if (bps <= 0) return '0 B/s'
  const kib = bps / 1024
  if (kib < 1024) return `${kib.toFixed(0)} KiB/s`
  const mib = kib / 1024
  if (mib < 1024) return `${mib.toFixed(2)} MiB/s`
  const gib = mib / 1024
  return `${gib.toFixed(2)} GiB/s`
}

/** Formatea segundos como "12s", "3m 45s" o "1h 12m" para ETA. */
export function formatEta(sec: number): string {
  if (!Number.isFinite(sec) || sec <= 0) return '—'
  if (sec < 60) return `${Math.ceil(sec)}s`
  if (sec < 3600) {
    const m = Math.floor(sec / 60)
    const s = Math.floor(sec % 60)
    return `${m}m ${s.toString().padStart(2, '0')}s`
  }
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  return `${h}h ${m.toString().padStart(2, '0')}m`
}
