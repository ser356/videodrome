import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Route, Routes } from 'react-router-dom'
import './index.css'
import { getPreferences, isTauri } from './lib/api'
import { applyGlassOpacity } from './lib/theme'
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
    .then((p) => applyGlassOpacity(p.glass_opacity))
    .catch(() => {})
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
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
        <Route path="/player" element={<Player />} />
      </Routes>
    </BrowserRouter>
  </StrictMode>,
)
