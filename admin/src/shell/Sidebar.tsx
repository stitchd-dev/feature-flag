import { useNavigate, useLocation, useParams } from 'react-router-dom'
import { I } from '../components/icons'
import { StitchdMark } from '../components/primitives'
import { OrgSwitcher } from './OrgSwitcher'
import { ProjectPicker } from './ProjectPicker'
import { EnvSwitcher } from './EnvSwitcher'
import { auth } from '../lib/auth'

const SUPERADMIN_NAV = [
  { id: 'orgs', path: '/superadmin/orgs', label: 'Organisations', icon: 'home' },
]

function getOrgNav(orgId: string) {
  const p = `/org/${orgId}`
  return [
    { id: 'dashboard', path: p, label: 'Dashboard', icon: 'home' },
    { id: 'flags', path: `${p}/flags`, label: 'Feature Flags', icon: 'flag', badge: '' },
    { id: 'segments', path: `${p}/segments`, label: 'Segments', icon: 'segment', badge: '' },
    { id: 'experiments', path: `${p}/experiments`, label: 'Experiments', icon: 'beaker', badge: '' },
    { id: 'events', path: `${p}/events`, label: 'Events', icon: 'event', badge: '' },
    { id: 'metrics', path: `${p}/metrics`, label: 'Metrics', icon: 'metric', badge: '' },
  ]
}

function getOrgAdmin(orgId: string) {
  const p = `/org/${orgId}`
  return [
    { id: 'environments', path: `${p}/environments`, label: 'Environments & SDK Keys', icon: 'key' },
    { id: 'context-explorer', path: `${p}/context-explorer`, label: 'Context Explorer', icon: 'database' },
    { id: 'members', path: `${p}/members`, label: 'Members & Roles', icon: 'users' },
    { id: 'audit', path: `${p}/audit`, label: 'Audit Log', icon: 'history' },
  ]
}

interface SidebarProps {
  onCmdK: () => void
}

export function Sidebar({ onCmdK }: SidebarProps) {
  const navigate = useNavigate()
  const location = useLocation()
  const { orgId } = useParams<{ orgId: string }>()

  // Only treat orgId as an org context when we're under /org/:orgId.
  // /superadmin/orgs/:orgId also captures orgId but must show the superadmin nav.
  const isOrgSection = location.pathname.startsWith('/org/')
  const activeOrgId = isOrgSection ? orgId : undefined

  const navItems = activeOrgId ? getOrgNav(activeOrgId) : SUPERADMIN_NAV
  const adminItems = activeOrgId ? getOrgAdmin(activeOrgId) : []

  function isActive(path: string) {
    if (activeOrgId && path === `/org/${activeOrgId}`) return location.pathname === `/org/${activeOrgId}`
    if (!activeOrgId && path === '/') return location.pathname === '/'
    return location.pathname.startsWith(path)
  }

  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <StitchdMark size={28} />
        <div className="brand-text">Stitchd</div>
        <div style={{ marginLeft: 'auto', fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--fg-subtle)', padding: '2px 5px', border: '1px solid var(--border)', borderRadius: 3 }}>v0.9</div>
      </div>

      {activeOrgId && (
        <>
          <OrgSwitcher currentOrgId={activeOrgId} />
          <ProjectPicker />
          <EnvSwitcher />
        </>
      )}

      {/* Uniform 12px top spacing — keeps consistent rhythm with the switcher blocks above,
          and ensures the search bar isn't flush against the brand divider in superadmin mode. */}
      <div style={{ padding: '12px 12px 4px' }}>
        <div className="search-input" style={{ maxWidth: '100%' }}>
          <I.search size={14} />
          <input className="input" placeholder="Search… flags, segments" onFocus={onCmdK} readOnly />
          <kbd>⌘K</kbd>
        </div>
      </div>

      <div className="sidebar-section">
        <div className="sidebar-section-label">{orgId ? 'Project' : 'Admin'}</div>
        <nav className="sidebar-nav">
          {navItems.map((item) => {
            const Ic = I[item.icon as keyof typeof I]
            return (
              <button
                key={item.id}
                className={`sidebar-link ${isActive(item.path) ? 'active' : ''}`}
                onClick={() => navigate(item.path)}
              >
                <Ic size={16} className="sidebar-icon" />
                <span>{item.label}</span>
                {'badge' in item && (item as { badge?: string }).badge && <span className="sidebar-badge">{(item as { badge?: string }).badge}</span>}
              </button>
            )
          })}
        </nav>
      </div>

      {adminItems.length > 0 && (
        <div className="sidebar-section">
          <div className="sidebar-section-label">Organization</div>
          <nav className="sidebar-nav">
            {adminItems.map((item) => {
              const Ic = I[item.icon as keyof typeof I]
              return (
                <button
                  key={item.id}
                  className={`sidebar-link ${isActive(item.path) ? 'active' : ''}`}
                  onClick={() => navigate(item.path)}
                >
                  <Ic size={16} className="sidebar-icon" />
                  <span>{item.label}</span>
                </button>
              )
            })}
          </nav>
        </div>
      )}

      <div className="sidebar-footer">
        <div className="user-avatar">{(() => { const s = auth.getSession(); return s ? s.userId.slice(0, 2).toUpperCase() : '??' })()}</div>
        <div className="user-meta">
          <div className="user-name">{auth.isSystem() ? 'Super Admin' : 'Org User'}</div>
          <div className="user-email" style={{ fontFamily: 'var(--font-mono)', fontSize: 10, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{auth.getSession()?.userId ?? '—'}</div>
        </div>
        <button className="icon-btn" title="Sign out" onClick={() => { auth.clearSession(); window.location.href = '/login' }}><I.lock size={14} /></button>
      </div>
    </aside>
  )
}

export function TopbarNav() {
  const navigate = useNavigate()
  const location = useLocation()
  const { orgId } = useParams<{ orgId: string }>()

  const isOrgSection = location.pathname.startsWith('/org/')
  const activeOrgId = isOrgSection ? orgId : undefined

  const navItems = activeOrgId ? getOrgNav(activeOrgId) : SUPERADMIN_NAV
  const adminItems = activeOrgId ? getOrgAdmin(activeOrgId) : []

  function isActive(path: string) {
    if (activeOrgId && path === `/org/${activeOrgId}`) return location.pathname === `/org/${activeOrgId}`
    if (!activeOrgId && path === '/') return location.pathname === '/'
    return location.pathname.startsWith(path)
  }

  return (
    <div className="topbar-nav">
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, paddingRight: 18, borderRight: '1px solid var(--border)' }}>
        <StitchdMark size={26} />
        <div className="brand-text" style={{ fontSize: 16 }}>Stitchd</div>
      </div>
      <div className="topbar-links">
        {navItems.map((item) => (
          <button key={item.id} className={`topbar-link ${isActive(item.path) ? 'active' : ''}`} onClick={() => navigate(item.path)}>
            {item.label}
          </button>
        ))}
        {adminItems.length > 0 && (
          <>
            <div style={{ width: 1, background: 'var(--border)', margin: '0 6px' }} />
            {adminItems.map((item) => (
              <button key={item.id} className={`topbar-link ${isActive(item.path) ? 'active' : ''}`} onClick={() => navigate(item.path)}>
                {item.label}
              </button>
            ))}
          </>
        )}
      </div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <span className="badge"><span className="env-dot" style={{ width: 6, height: 6 }} />production</span>
        <div className="user-avatar">PR</div>
      </div>
    </div>
  )
}
