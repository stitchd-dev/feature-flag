import { useState, useEffect } from 'react'
import { BrowserRouter, Routes, Route, useNavigate, Outlet } from 'react-router-dom'
import { useTweaks } from './hooks/useTweaks'
import { Sidebar, TopbarNav } from './shell/Sidebar'
import { CommandPalette } from './shell/CommandPalette'
import { TweaksPanel } from './shell/TweaksPanel'
import { ProtectedRoute } from './shell/ProtectedRoute'
import {
  Dashboard, FlagsList, FlagDetail,
  SegmentsList, SegmentDetail,
  ExperimentsList, ExperimentDetail,
  EventsRegistry, Environments, Members, AuditLog, SuperAdmin,
} from './pages/stubs'
import { I } from './components/icons'
import { StitchdMark } from './components/primitives'

// Login screen — wired in Phase 3, stub for now
function LoginPage() {
  const navigate = useNavigate()
  return (
    <div style={{ minHeight: '100vh', display: 'grid', placeItems: 'center', background: 'var(--bg)' }}>
      <div className="card" style={{ width: 400, padding: 32, textAlign: 'center' }}>
        <StitchdMark size={48} radius={12} />
        <div style={{ fontFamily: 'var(--font-display)', fontWeight: 800, fontSize: 26, margin: '16px 0 8px', letterSpacing: '-0.02em' }}>Sign in to Stitchd</div>
        <div style={{ color: 'var(--fg-muted)', fontSize: 13, marginBottom: 24 }}>Feature flags &amp; experiments, self-hosted.</div>
        <button
          className="btn primary lg"
          style={{ width: '100%' }}
          onClick={() => {
            localStorage.setItem('stitchd_jwt', 'dev-token')
            navigate('/')
          }}
        >
          Continue (dev mode) <I.arrowRight size={14} />
        </button>
        <div style={{ marginTop: 12, fontSize: 12, color: 'var(--fg-subtle)' }}>Real auth implemented in Phase 3</div>
      </div>
    </div>
  )
}

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
