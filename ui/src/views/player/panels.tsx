import { useEffect, useState } from 'react'
import { X } from '@phosphor-icons/react'
import { type MediaStream, type Subtitle } from '../../lib/api'
import { useT } from '../../lib/i18n'
import { languageLabel } from './utils'

export function AudioPanel({
  tracks,
  activeIdx,
  switching,
  onPick,
  onClose,
}: {
  tracks: MediaStream[]
  activeIdx: number
  /** `true` mientras el backend está purgando + respawneando.
   * Deshabilita clicks para evitar switches concurrentes. */
  switching: boolean
  onPick: (idx: number) => void
  onClose: () => void
}) {
  const tr = useT()
  return (
    <div
      className="absolute inset-y-0 right-0 z-30 flex w-full max-w-[420px] flex-col border-l border-hairline bg-black/95 backdrop-blur-lg"
      onClick={(e) => e.stopPropagation()}
    >
      <header className="flex items-center justify-between border-b border-hairline px-5 py-4">
        <div>
          <h2 className="text-[15px] font-semibold text-ink">{tr('player.audioTrack')}</h2>
          <p className="mt-0.5 text-[11px] text-muted">
            {tracks.length === 1
              ? tr('player.available1', { n: tracks.length })
              : tr('player.availableN', { n: tracks.length })}
          </p>
        </div>
        <button
          onClick={onClose}
          className="flex h-8 w-8 items-center justify-center rounded-full text-muted hover:bg-surface hover:text-ink"
          aria-label={tr('common.close')}
        >
          <X size={16} weight="bold" />
        </button>
      </header>

      <ul className="flex-1 divide-y divide-hairline-soft overflow-y-auto">
        {tracks.map((t, idx) => {
          const isActive = idx === activeIdx
          // El label del track del contenedor suele traer info útil
          // (ej: "English 5.1 Commentary"). Si no, componemos con
          // idioma + codec.
          const label =
            t.title ||
            [t.language ? languageLabel(t.language) : null, t.codec]
              .filter(Boolean)
              .join(' · ') ||
            tr('player.trackN', { n: idx + 1 })
          return (
            <li key={`audio-${idx}`}>
              <button
                onClick={() => onPick(idx)}
                disabled={switching || isActive}
                className={`flex w-full items-start justify-between gap-3 px-5 py-3 text-left transition-colors ${
                  isActive
                    ? 'bg-accent/10'
                    : 'hover:bg-surface disabled:opacity-50'
                }`}
              >
                <div className="min-w-0 flex-1">
                  <p className="truncate text-[13px] text-ink">{label}</p>
                  <p className="mt-0.5 text-[11px] text-muted">
                    {t.language ? languageLabel(t.language) : tr('player.langUnknown')}
                    <span className="mx-1.5 text-dim">·</span>
                    <span className="text-dim">{t.codec}</span>
                  </p>
                </div>
                {isActive && (
                  <span className="mt-0.5 text-[11px] font-medium text-accent">
                    {switching ? tr('common.loading') : tr('player.active')}
                  </span>
                )}
              </button>
            </li>
          )
        })}
      </ul>
    </div>
  )
}

/**
 * Panel lateral con los subtítulos disponibles agrupados por idioma.
 * Tabs de idioma arriba (ordenadas por número de subs disponibles);
 * abajo, la lista de releases para el idioma seleccionado ordenados
 * por descargas.
 */
export function SubsPanel({
  subs,
  loading,
  activeFileId,
  downloadingFileId,
  onPick,
  onClear,
  onClose,
  embeddedSubs,
  activeEmbeddedIdx,
  onPickEmbedded,
}: {
  subs: Subtitle[] | null
  loading: boolean
  activeFileId: number | null
  downloadingFileId: number | null
  onPick: (sub: Subtitle) => void
  onClear: () => void
  onClose: () => void
  /** Subs embebidos (extraídos del contenedor con ffmpeg). Ya
   * vienen filtrados por el caller para excluir bitmap (PGS/DVBSUB).
   * Si está vacío, la sección "Del fichero" no se pinta. */
  embeddedSubs: MediaStream[]
  /** Índice GLOBAL de la pista activa (`MediaStream.index`, tal
   * como lo reporta ffprobe entre TODOS los streams del contenedor),
   * o `null` si el sub activo no es embedded. Se compara contra
   * `sub.index` — no contra la posición de `sub` dentro del array
   * filtrado, porque el filtro de bitmap puede haber saltado índices. */
  activeEmbeddedIdx: number | null
  onPickEmbedded: (stream: MediaStream, streamIndex: number) => void
}) {
  const tr = useT()
  // Idiomas presentes en la lista + conteo. Se ordenan por count
  // descendente y luego alfabético → el idioma con más opciones
  // aparece primero (típicamente inglés).
  const [langs, defaultLang] = (() => {
    if (!subs || subs.length === 0) return [[] as { code: string; count: number }[], null]
    const map = new Map<string, number>()
    for (const s of subs) {
      map.set(s.language, (map.get(s.language) ?? 0) + 1)
    }
    const arr = Array.from(map, ([code, count]) => ({ code, count })).sort(
      (a, b) => b.count - a.count || a.code.localeCompare(b.code),
    )
    // Prioriza español si está entre los 3 primeros idiomas (aunque
    // no sea el que más subs tiene) — mejor default para el usuario
    // hispanohablante que abrir siempre en inglés.
    const es = arr.findIndex((l) => l.code === 'es')
    if (es > 0 && es < 3) {
      const [esItem] = arr.splice(es, 1)
      arr.unshift(esItem)
    }
    return [arr, arr[0]?.code ?? null]
  })()

  const [selectedLang, setSelectedLang] = useState<string | null>(defaultLang)
  // Sincroniza selectedLang si cambia la lista (nueva peli, refetch).
  useEffect(() => {
    if (selectedLang && langs.some((l) => l.code === selectedLang)) return
    // eslint-disable-next-line react-hooks/set-state-in-effect -- Reset s\u00edncrono cuando el idioma actual desaparece de la lista.
    setSelectedLang(defaultLang)
  }, [defaultLang, langs, selectedLang])

  const filtered = subs?.filter((s) => s.language === selectedLang) ?? []

  return (
    <div
      className="absolute inset-y-0 right-0 z-30 flex w-full max-w-[420px] flex-col border-l border-hairline bg-black/95 backdrop-blur-lg"
      onClick={(e) => e.stopPropagation()}
    >
      <header className="flex items-center justify-between border-b border-hairline px-5 py-4">
        <div>
          <h2 className="text-[15px] font-semibold text-ink">{tr('player.subtitles')}</h2>
          {(activeFileId != null || activeEmbeddedIdx != null) && (
            <button
              onClick={onClear}
              className="mt-0.5 text-[11px] text-muted hover:text-ink"
            >
              {tr('player.removeCurrent')}
            </button>
          )}
        </div>
        <button
          onClick={onClose}
          className="flex h-8 w-8 items-center justify-center rounded-full text-muted hover:bg-surface hover:text-ink"
          aria-label={tr('common.close')}
        >
          <X size={16} weight="bold" />
        </button>
      </header>

      {/* Layout de dos scrolls independientes:
       *   1. Embedded arriba con techo del 45% (si hay >4 pistas) +
       *      scroll propio. Cuando el contenedor no llega al tope,
       *      se autoajusta al alto de su contenido.
       *   2. Tabs de idiomas + lista OpenSubtitles debajo, en su
       *      propio bloque `flex-1` con overflow-y-auto. Las tabs
       *      quedan siempre visibles porque son `flex-shrink-0` del
       *      wrapper de lista, no sticky.
       *
       * El intento anterior (un único scroll con tabs `sticky top-0`)
       * daba UX rara: al arrastrar mucho, las tabs desaparec\u00edan y
       * reaparec\u00edan superpuestas al header, y con muchos embedded no
       * qued\u00f3 forma clara de dividir espacio. Con dos regiones
       * separadas todo es predecible y llegamos a OpenSubtitles
       * siempre \u2014 aunque haya 20 embedded, ese bloque ocupa como
       * mucho 45% del panel. */}
      {embeddedSubs.length > 0 && (
        <div className="flex max-h-[45%] flex-shrink-0 flex-col border-b border-hairline">
          <p className="px-5 pt-3 text-[10px] uppercase tracking-[0.14em] text-dim">
            {tr('player.embedded')}
          </p>
          <ul className="min-h-0 flex-1 divide-y divide-hairline-soft overflow-y-auto">
            {embeddedSubs.map((sub, idx) => {
              const isActive = sub.index === activeEmbeddedIdx
              const label = sub.title || tr('player.trackN', { n: idx + 1 })
              return (
                <li key={`emb-${sub.index}`}>
                  <button
                    onClick={() => onPickEmbedded(sub, sub.index)}
                    className={`flex w-full items-start justify-between gap-3 px-5 py-3 text-left transition-colors ${
                      isActive ? 'bg-accent/10' : 'hover:bg-surface'
                    }`}
                  >
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-[13px] text-ink">{label}</p>
                      <p className="mt-0.5 text-[11px] text-muted">
                        {sub.language
                          ? languageLabel(sub.language)
                          : tr('player.langUnknown')}
                        <span className="mx-1.5 text-dim">·</span>
                        <span className="text-dim">{sub.codec}</span>
                      </p>
                    </div>
                    {isActive && (
                      <span className="mt-0.5 text-[11px] font-medium text-accent">
                        {tr('player.active')}
                      </span>
                    )}
                  </button>
                </li>
              )
            })}
          </ul>
        </div>
      )}

      {loading && (
        <div className="flex flex-1 items-center justify-center py-10">
          <div className="h-6 w-6 animate-spin rounded-full border-2 border-accent border-t-transparent" />
        </div>
      )}

      {!loading &&
        (subs === null || subs.length === 0) &&
        embeddedSubs.length === 0 && (
          <div className="flex flex-1 flex-col items-center justify-center px-6 text-center">
            <p className="text-[14px] text-body">{tr('player.noSubs')}</p>
            <p className="mt-1 text-[12px] text-muted">
              {tr('player.noSubsHint')}
            </p>
          </div>
        )}

      {!loading && subs && subs.length > 0 && (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex flex-shrink-0 gap-1 overflow-x-auto border-b border-hairline px-3 py-2">
            {langs.map((l) => (
              <button
                key={l.code}
                onClick={() => setSelectedLang(l.code)}
                className={`shrink-0 rounded-full px-3 py-1.5 text-[12px] transition-colors ${
                  selectedLang === l.code
                    ? 'bg-accent text-on-accent'
                    : 'bg-surface text-body hover:bg-surface-hi'
                }`}
              >
                {languageLabel(l.code)}{' '}
                <span
                  className={
                    selectedLang === l.code ? 'opacity-70' : 'text-muted'
                  }
                >
                  {l.count}
                </span>
              </button>
            ))}
          </div>

          <ul className="min-h-0 flex-1 divide-y divide-hairline-soft overflow-y-auto">
            {filtered.map((sub) => {
              const isActive = sub.file_id === activeFileId
              const isDownloading = sub.file_id === downloadingFileId
              return (
                <li key={sub.file_id}>
                  <button
                    disabled={isDownloading || downloadingFileId !== null}
                    onClick={() => onPick(sub)}
                    className={`flex w-full items-start justify-between gap-3 px-5 py-3 text-left transition-colors ${
                      isActive
                        ? 'bg-accent/10'
                        : 'hover:bg-surface disabled:opacity-50'
                    }`}
                  >
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-[13px] text-ink">
                        {sub.release || sub.file_name || tr('player.subtitle')}
                      </p>
                      <p className="mt-0.5 flex items-center gap-2 text-[11px] text-muted">
                        <span>{tr('player.downloads', { n: sub.downloads.toLocaleString() })}</span>
                        {sub.from_trusted && (
                          <span
                            className="rounded-sm border border-good/40 bg-good/10 px-1.5 py-0.5 text-[10px] font-medium text-good"
                            title={tr('player.trustedTitle')}
                          >
                            Trusted
                          </span>
                        )}
                        {sub.hearing_impaired && (
                          <span
                            className="rounded-sm border border-hairline px-1.5 py-0.5 text-[10px]"
                            title={tr('player.sdhTitle')}
                          >
                            SDH
                          </span>
                        )}
                      </p>
                    </div>
                    {isActive && (
                      <span className="mt-0.5 text-[11px] font-medium text-accent">
                        {tr('player.active')}
                      </span>
                    )}
                    {isDownloading && (
                      <div className="mt-0.5 h-4 w-4 animate-spin rounded-full border-2 border-accent border-t-transparent" />
                    )}
                  </button>
                </li>
              )
            })}
          </ul>
        </div>
      )}
    </div>
  )
}
