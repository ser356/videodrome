import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Route, Routes } from 'react-router-dom'
import './index.css'
import { GlobalContextMenu } from './components/GlobalContextMenu'
import { ScrollRestore } from './components/ScrollRestore'
import { TorrentDropOverlay } from './components/TorrentDropOverlay'
import {
  detectClientCapabilities,
  getPreferences,
  isTauri,
  setClientCapabilities,
} from './lib/api'
import { initLocale } from './lib/i18n'
import { applyGlassOpacity, applySkin } from './lib/theme'
import { DroppedTorrent } from './views/DroppedTorrent'
import { Home } from './views/Home'
import { Login } from './views/Login'
import { Player } from './views/Player'
import { Recommendations } from './views/Recommendations'
import { Search } from './views/Search'
import { SearchResults } from './views/SearchResults'
import { SeriesDetail } from './views/SeriesDetail'
import { Settings } from './views/Settings'
import { Torrents } from './views/Torrents'

// Aplica la opacidad del liquid glass ANTES de montar el árbol para
// evitar un flash del look default. Si aún no estamos en Tauri (dev
// del UI en Safari puro) o el backend falla, se queda en el default
// del CSS (`--glass-opaque: 0`).
if (isTauri()) {
  getPreferences()
    .then((p) => {
      applyGlassOpacity(p.glass_opacity)
      applySkin(p.skin)
    })
    .catch(() => {})

  // Audit §4: registra las capacidades del WebView (`canPlayType`
  // por códec) para que el backend decida DIRECT vs COPY vs
  // TRANSCODE con datos reales del cliente en vez de una whitelist
  // estática. Es una llamada disparada-y-olvidada: no bloquea el
  // arranque, y si falla el backend usa `safe_default` (H.264+AAC).
  const caps = detectClientCapabilities()
  setClientCapabilities(caps).catch(() => {})
}

// Inicializa el locale ANTES del render para que el primer paint ya
// muestre los strings del idioma correcto y no haya un flash EN → ES.
// `initLocale` es best-effort: si falla (no-Tauri, backend caído),
// cae a `navigator.language` y de ahí a `en`.
async function bootstrap() {
  await initLocale()
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <BrowserRouter>
        <ScrollRestore />
        <TorrentDropOverlay />
        <GlobalContextMenu />
        <Routes>
          <Route path="/" element={<Home />} />
          <Route path="/login" element={<Login />} />
          <Route path="/recs" element={<Recommendations />} />
          <Route path="/search" element={<Search />} />
          <Route path="/search/results" element={<SearchResults />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="/series/:tmdbId" element={<SeriesDetail />} />
          <Route path="/torrents/tmdb/:tmdbId" element={<Torrents mode="tmdb" />} />
          <Route path="/torrents/search" element={<Torrents mode="direct" />} />
          <Route path="/torrents/series/:tmdbId" element={<Torrents mode="series" />} />
          <Route path="/torrents/dropped" element={<DroppedTorrent />} />
          <Route path="/player" element={<Player />} />
        </Routes>
      </BrowserRouter>
    </StrictMode>,
  )
}

void bootstrap()
