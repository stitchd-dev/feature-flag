import { useState, useEffect } from 'react'
import { BrowserRouter, Routes, Route, Navigate, Outlet } from 'react-router-dom'
import { useTweaks } from './hooks/useTweaks'
import { Sidebar, TopbarNav } from './shell/Sidebar'
import { CommandPalette } from './shell/CommandPalette'
import { TweaksPanel } from './shell/TweaksPanel'
import { SuperAdminGuard, OrgGuard } from './shell/guards'
import { OrgProvider } from './context/OrgContext'
import { LoginPage } from './pages/Login'
import { OidcCallbackPage } from './pages/OidcCallback'
import { OrgDashboard } from './pages/OrgDashboard'
import { FlagsList } from './pages/flags/FlagsList'
import { FlagDetail } from './pages/flags/FlagDetail'
import { SegmentsList } from './pages/segments/SegmentsList'
import { SegmentDetail } from './pages/segments/SegmentDetail'
import { ExperimentsList } from './pages/experiments/ExperimentsList'
import { ExperimentDetail } from './pages/experiments/ExperimentDetail'
import { OrgsList, OrgDetail, SeedUser } from './pages/superadmin'
import {
  EventsRegistry, Environments, Members, AuditLog,
} from './pages/stubs'
import { I } from './components/icons'

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

function OrgShell() {
  return (
    <OrgProvider>
      <AppShell />
    </OrgProvider>
  )
}

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        {/* Public */}
        <Route path="/login" element={<LoginPage />} />
        <Route path="/auth/callback" element={<OidcCallbackPage />} />

        {/* Superadmin section — is_system=true only */}
        <Route element={<SuperAdminGuard />}>
          <Route element={<AppShell />}>
            <Route path="/superadmin" element={<Navigate to="/superadmin/orgs" replace />} />
            <Route path="/superadmin/orgs" element={<OrgsList />} />
            <Route path="/superadmin/orgs/:orgId" element={<OrgDetail />} />
            <Route path="/superadmin/orgs/:orgId/users" element={<SeedUser />} />
          </Route>
        </Route>

        {/* Per-org section — is_system=false only */}
        <Route element={<OrgGuard />}>
          <Route element={<OrgShell />}>
            <Route path="/org/:orgId" element={<OrgDashboard />} />
            <Route path="/org/:orgId/flags" element={<FlagsList />} />
            <Route path="/org/:orgId/flags/:key" element={<FlagDetail />} />
            <Route path="/org/:orgId/segments" element={<SegmentsList />} />
            <Route path="/org/:orgId/segments/:key" element={<SegmentDetail />} />
            <Route path="/org/:orgId/experiments" element={<ExperimentsList />} />
            <Route path="/org/:orgId/experiments/:key" element={<ExperimentDetail />} />
            <Route path="/org/:orgId/events" element={<EventsRegistry />} />
            <Route path="/org/:orgId/environments" element={<Environments />} />
            <Route path="/org/:orgId/members" element={<Members />} />
            <Route path="/org/:orgId/audit" element={<AuditLog />} />
          </Route>
        </Route>

        {/* Catch-all → login */}
        <Route path="*" element={<Navigate to="/login" replace />} />
      </Routes>
    </BrowserRouter>
  )
}
