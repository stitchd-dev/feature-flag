import { useNavigate, useLocation, useParams } from 'react-router-dom'
import { I } from '../components/icons'
import { StitchdMark } from '../components/primitives'

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
  ]
}

function getOrgAdmin(orgId: string) {
  const p = `/org/${orgId}`
  return [
    { id: 'environments', path: `${p}/environments`, label: 'Environments & SDK Keys', icon: 'key' },
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

  const navItems = orgId ? getOrgNav(orgId) : SUPERADMIN_NAV
  const adminItems = orgId ? getOrgAdmin(orgId) : []

  function isActive(path: string) {
    if (orgId && path === `/org/${orgId}`) return location.pathname === `/org/${orgId}`
    if (!orgId && path === '/') return location.pathname === '/'
    return location.pathname.startsWith(path)
  }

  const envPath = orgId ? `/org/${orgId}/environments` : '/superadmin'

  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <StitchdMark size={28} />
        <div className="brand-text">Stitchd</div>
        <div style={{ marginLeft: 'auto', fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--fg-subtle)', padding: '2px 5px', border: '1px solid var(--border)', borderRadius: 3 }}>v0.9</div>
      </div>

      {orgId && (
        <>
          <div className="org-switcher">
            <div className="org-avatar">AC</div>
            <div className="org-meta">
              <div className="org-name">Acme Cloud</div>
              <div className="org-sub">{orgId} · production</div>
            </div>
            <I.chevronDown size={14} className="org-chevron" />
          </div>

          <div className="env-pill" onClick={() => navigate(envPath)}>
            <span className="env-dot" />
            <span className="env-name">production</span>
            <span className="env-switch">switch</span>
          </div>
        </>
      )}

      <div style={{ padding: '0 12px 8px' }}>
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
        <div className="user-avatar">PR</div>
        <div className="user-meta">
          <div className="user-name">Priya Reddy</div>
          <div className="user-email">priya@stitchd.dev</div>
        </div>
        <button className="icon-btn" title="Notifications"><I.bell size={14} /></button>
      </div>
    </aside>
  )
}

export function TopbarNav() {
  const navigate = useNavigate()
  const location = useLocation()
  const { orgId } = useParams<{ orgId: string }>()

  const navItems = orgId ? getOrgNav(orgId) : SUPERADMIN_NAV
  const adminItems = orgId ? getOrgAdmin(orgId) : []

  function isActive(path: string) {
    if (orgId && path === `/org/${orgId}`) return location.pathname === `/org/${orgId}`
    if (!orgId && path === '/') return location.pathname === '/'
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
