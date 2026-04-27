import { useState, useEffect } from 'react'
import { BrowserRouter, Routes, Route, Outlet } from 'react-router-dom'
import { useTweaks } from './hooks/useTweaks'
import { Sidebar, TopbarNav } from './shell/Sidebar'
import { CommandPalette } from './shell/CommandPalette'
import { TweaksPanel } from './shell/TweaksPanel'
import { ProtectedRoute } from './shell/ProtectedRoute'
import { LoginPage } from './pages/Login'
import { OidcCallbackPage } from './pages/OidcCallback'
import {
  Dashboard, FlagsList, FlagDetail,
  SegmentsList, SegmentDetail,
  ExperimentsList, ExperimentDetail,
  EventsRegistry, Environments, Members, AuditLog, SuperAdmin,
} from './pages/stubs'
import { I } from './components/icons'
// eslint-disable-next-line @typescript-eslint/no-unused-vars -- used in TweaksPanel button below

function AppShell() {
  const { tweaks, setTweak } = useTweaks()
  const [cmdkOpen, setCmdkOpen] = useState(false)
  const [tweaksOpen, setTweaksOpen] = useState(false)

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setCmdkOpen((v) => !v)
      }
      if (e.key === 'Escape') {
        setCmdkOpen(false)
        setTweaksOpen(false)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div className="app-shell" data-nav={tweaks.navStyle}>
      <Sidebar onCmdK={() => setCmdkOpen(true)} />
      <TopbarNav />
      <div className="main-area">
        {/* Tweaks toggle button */}
        <button
          onClick={() => setTweaksOpen((v) => !v)}
          style={{ position: 'fixed', bottom: 20, right: 20, zIndex: 90, background: 'var(--surface)', border: '1px solid var(--border-strong)', borderRadius: 8, padding: '8px 12px', display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, fontWeight: 500, boxShadow: 'var(--shadow-md)', cursor: 'pointer', color: 'var(--fg-muted)' }}
        >
          <I.cog size={14} /> Tweaks
        </button>
        <Outlet />
      </div>
      <CommandPalette open={cmdkOpen} onClose={() => setCmdkOpen(false)} />
      {tweaksOpen && (
        <TweaksPanel tweaks={tweaks} setTweak={setTweak} onClose={() => setTweaksOpen(false)} />
      )}
    </div>
  )
}

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/auth/callback" element={<OidcCallbackPage />} />
        <Route element={<ProtectedRoute />}>
          <Route element={<AppShell />}>
            <Route path="/" element={<Dashboard />} />
            <Route path="/flags" element={<FlagsList />} />
            <Route path="/flags/:key" element={<FlagDetail />} />
            <Route path="/segments" element={<SegmentsList />} />
            <Route path="/segments/:key" element={<SegmentDetail />} />
            <Route path="/experiments" element={<ExperimentsList />} />
            <Route path="/experiments/:key" element={<ExperimentDetail />} />
            <Route path="/events" element={<EventsRegistry />} />
            <Route path="/environments" element={<Environments />} />
            <Route path="/members" element={<Members />} />
            <Route path="/audit" element={<AuditLog />} />
            <Route path="/super-admin" element={<SuperAdmin />} />
          </Route>
        </Route>
      </Routes>
    </BrowserRouter>
  )
}
