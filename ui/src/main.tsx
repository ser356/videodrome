import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Route, Routes } from 'react-router-dom'
import './index.css'
import { Home } from './views/Home'
import { Login } from './views/Login'
import { Recommendations } from './views/Recommendations'
import { Search } from './views/Search'
import { Torrents } from './views/Torrents'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/login" element={<Login />} />
        <Route path="/recs" element={<Recommendations />} />
        <Route path="/search" element={<Search />} />
        <Route path="/torrents/tmdb/:tmdbId" element={<Torrents mode="tmdb" />} />
        <Route path="/torrents/search" element={<Torrents mode="direct" />} />
      </Routes>
    </BrowserRouter>
  </StrictMode>,
)
