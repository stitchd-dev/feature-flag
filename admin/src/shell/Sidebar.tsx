import { useNavigate, useLocation } from 'react-router-dom'
import { I } from '../components/icons'
import { StitchdMark } from '../components/primitives'

const NAV_ITEMS = [
  { id: 'dashboard', path: '/', label: 'Dashboard', icon: 'home' },
  { id: 'flags', path: '/flags', label: 'Feature Flags', icon: 'flag', badge: '142' },
  { id: 'segments', path: '/segments', label: 'Segments', icon: 'segment', badge: '24' },
  { id: 'experiments', path: '/experiments', label: 'Experiments', icon: 'beaker', badge: '6' },
  { id: 'events', path: '/events', label: 'Events', icon: 'event', badge: '47' },
]

const NAV_ADMIN = [
  { id: 'environments', path: '/environments', label: 'Environments & SDK Keys', icon: 'key' },
  { id: 'members', path: '/members', label: 'Members & Roles', icon: 'users' },
  { id: 'audit', path: '/audit', label: 'Audit Log', icon: 'history' },
  { id: 'super-admin', path: '/super-admin', label: 'Super Admin', icon: 'shield' },
]

interface SidebarProps {
  onCmdK: () => void
}

export function Sidebar({ onCmdK }: SidebarProps) {
  const navigate = useNavigate()
  const location = useLocation()

  function isActive(path: string) {
    if (path === '/') return location.pathname === '/'
    return location.pathname.startsWith(path)
  }

  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <StitchdMark size={28} />
        <div className="brand-text">Stitchd</div>
        <div style={{ marginLeft: 'auto', fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--fg-subtle)', padding: '2px 5px', border: '1px solid var(--border)', borderRadius: 3 }}>v0.9</div>
      </div>

      <div className="org-switcher">
        <div className="org-avatar">AC</div>
        <div className="org-meta">
          <div className="org-name">Acme Cloud</div>
          <div className="org-sub">stitchd-app · production</div>
        </div>
        <I.chevronDown size={14} className="org-chevron" />
      </div>

      <div className="env-pill" onClick={() => navigate('/environments')}>
        <span className="env-dot" />
        <span className="env-name">production</span>
        <span className="env-switch">switch</span>
      </div>

      <div style={{ padding: '0 12px 8px' }}>
        <div className="search-input" style={{ maxWidth: '100%' }}>
          <I.search size={14} />
          <input className="input" placeholder="Search… flags, segments" onFocus={onCmdK} readOnly />
          <kbd>⌘K</kbd>
        </div>
      </div>

      <div className="sidebar-section">
        <div className="sidebar-section-label">Project</div>
        <nav className="sidebar-nav">
          {NAV_ITEMS.map((item) => {
            const Ic = I[item.icon]
            return (
              <button
                key={item.id}
                className={`sidebar-link ${isActive(item.path) ? 'active' : ''}`}
                onClick={() => navigate(item.path)}
              >
                <Ic size={16} className="sidebar-icon" />
                <span>{item.label}</span>
                {item.badge && <span className="sidebar-badge">{item.badge}</span>}
              </button>
            )
          })}
        </nav>
      </div>

      <div className="sidebar-section">
        <div className="sidebar-section-label">Organization</div>
        <nav className="sidebar-nav">
          {NAV_ADMIN.map((item) => {
            const Ic = I[item.icon]
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

  function isActive(path: string) {
    if (path === '/') return location.pathname === '/'
    return location.pathname.startsWith(path)
  }

  return (
    <div className="topbar-nav">
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, paddingRight: 18, borderRight: '1px solid var(--border)' }}>
        <StitchdMark size={26} />
        <div className="brand-text" style={{ fontSize: 16 }}>Stitchd</div>
      </div>
      <div className="topbar-links">
        {NAV_ITEMS.map((item) => (
          <button key={item.id} className={`topbar-link ${isActive(item.path) ? 'active' : ''}`} onClick={() => navigate(item.path)}>
            {item.label}
          </button>
        ))}
        <div style={{ width: 1, background: 'var(--border)', margin: '0 6px' }} />
        {NAV_ADMIN.slice(0, 3).map((item) => (
          <button key={item.id} className={`topbar-link ${isActive(item.path) ? 'active' : ''}`} onClick={() => navigate(item.path)}>
            {item.label}
          </button>
        ))}
      </div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <span className="badge"><span className="env-dot" style={{ width: 6, height: 6 }} />production</span>
        <div className="user-avatar">PR</div>
      </div>
    </div>
  )
}
