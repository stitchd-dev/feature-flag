import { useState } from 'react'
import { PageHeader } from '../components/primitives'
import { I } from '../components/icons'
import { EVENTS, ENVIRONMENTS, SDK_KEYS, MEMBERS, AUDIT } from '../lib/mockData'

// ─── Events Registry ─────────────────────────────────────────────────────────

export function EventsRegistry() {
  return (
    <>
      <PageHeader
        crumbs={['stitchd-app', 'Events']}
        title="Events"
        subtitle="Pre-registered event definitions. Unknown events are rejected at the ingestion boundary."
        actions={
          <>
            <button className="btn"><I.download size={13} /> Schema</button>
            <button className="btn primary"><I.plus size={14} /> Register event</button>
          </>
        }
      />
      <div className="page-body">
        <div className="stat-grid" style={{ marginBottom: 18 }}>
          <div className="stat"><div className="stat-label">Registered</div><div className="stat-value">47</div></div>
          <div className="stat"><div className="stat-label">30d ingestion</div><div className="stat-value">128M</div><div className="stat-delta up">+8.2%</div></div>
          <div className="stat"><div className="stat-label">Rejected (unknown)</div><div className="stat-value">412</div><div className="stat-delta down">last 24h</div></div>
          <div className="stat"><div className="stat-label">CH ingest p95</div><div className="stat-value">14ms</div></div>
        </div>
        <div className="card">
          <table className="table">
            <thead>
              <tr><th>Key</th><th>Type</th><th>Description</th><th>30d volume</th><th>Last seen</th><th>Schema</th></tr>
            </thead>
            <tbody>
              {EVENTS.map((e) => (
                <tr key={e.key}>
                  <td><span className="mono-key">{e.key}</span></td>
                  <td><span className={`type-pill ${e.type}`}>{e.type}</span></td>
                  <td style={{ color: 'var(--fg-muted)' }}>{e.described}</td>
                  <td style={{ fontFamily: 'var(--font-mono)', fontWeight: 600 }}>{e.volume30d}</td>
                  <td style={{ color: 'var(--fg-muted)' }}>{e.lastSeen}</td>
                  <td><code style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{e.schema}</code></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  )
}

// ─── Environments & SDK Keys ──────────────────────────────────────────────────

export function Environments() {
  return (
    <>
      <PageHeader
        crumbs={['stitchd-app', 'Environments']}
        title="Environments & SDK Keys"
        subtitle="Each environment scopes its own rules, segments, experiments and SDK keys. Min 1 active key per environment."
        actions={<button className="btn primary"><I.plus size={14} /> New environment</button>}
      />
      <div className="page-body">
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(240px, 1fr))', gap: 12, marginBottom: 18 }}>
          {ENVIRONMENTS.map((e) => (
            <div key={e.key} className="card" style={{ padding: 16, position: 'relative', overflow: 'hidden' }}>
              <div style={{ position: 'absolute', top: 0, left: 0, right: 0, height: 3, background: e.color }} />
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
                <span className="env-dot" style={{ background: e.color, boxShadow: `0 0 0 3px ${e.color}22` }} />
                <span style={{ fontFamily: 'var(--font-mono)', fontWeight: 700, fontSize: 14 }}>{e.name}</span>
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10, fontSize: 12 }}>
                {[['Flags', String(e.flags)], ['Active keys', String(e.keys)], ['Eval p95', e.latencyP95], ['Rate', e.evalRate]].map(([label, val]) => (
                  <div key={label}>
                    <div style={{ color: 'var(--fg-muted)', fontSize: 10, textTransform: 'uppercase', letterSpacing: '0.05em' }}>{label}</div>
                    <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 600, fontSize: 16 }}>{val}</div>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
        <div className="card">
          <div className="card-header">
            <div className="card-title"><I.key size={14} /> SDK Keys</div>
            <button className="btn primary sm"><I.plus size={12} /> New key</button>
          </div>
          <table className="table">
            <thead>
              <tr><th>Key</th><th>Environment</th><th>Created</th><th>Last used</th><th>Status</th><th>Created by</th><th></th></tr>
            </thead>
            <tbody>
              {SDK_KEYS.map((k) => (
                <tr key={k.id}>
                  <td>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                      <I.key size={13} stroke="var(--fg-subtle)" />
                      <span className="mono-key">{k.id}</span>
                      <button className="icon-btn"><I.copy size={12} /></button>
                    </div>
                  </td>
                  <td><span className="badge">{k.env}</span></td>
                  <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--fg-muted)' }}>{k.created}</td>
                  <td style={{ color: 'var(--fg-muted)' }}>{k.lastUsed}</td>
                  <td>
                    <span className={`badge ${k.status === 'active' ? 'success' : 'warning'}`}>
                      <span className="dot" />
                      {k.status}
                    </span>
                  </td>
                  <td>{k.by}</td>
                  <td><button className="btn sm danger">Revoke</button></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  )
}

// ─── Members & Roles ─────────────────────────────────────────────────────────

export function Members() {
  const [tab, setTab] = useState<'members' | 'roles' | 'pending' | 'sso'>('members')

  return (
    <>
      <PageHeader
        crumbs={['Acme Cloud', 'Members']}
        title="Members & Roles"
        subtitle="Three-tier hierarchy: Organization → Project → Environment. Wildcard permissions resolve most-specific match wins."
        actions={
          <>
            <button className="btn"><I.upload size={13} /> Bulk invite</button>
            <button className="btn primary"><I.plus size={14} /> Invite member</button>
          </>
        }
      />
      <div className="page-body">
        <div className="tabs">
          <button className={`tab ${tab === 'members' ? 'active' : ''}`} onClick={() => setTab('members')}>Members <span className="count">{MEMBERS.length}</span></button>
          <button className={`tab ${tab === 'roles' ? 'active' : ''}`} onClick={() => setTab('roles')}>Roles <span className="count">4</span></button>
          <button className={`tab ${tab === 'pending' ? 'active' : ''}`} onClick={() => setTab('pending')}>Pending invites <span className="count">2</span></button>
          <button className={`tab ${tab === 'sso' ? 'active' : ''}`} onClick={() => setTab('sso')}>SSO providers</button>
        </div>

        {tab === 'members' && (
          <>
            <div className="card">
              <table className="table">
                <thead>
                  <tr><th>Member</th><th>Role</th><th>Projects</th><th>MFA</th><th>Last active</th><th></th></tr>
                </thead>
                <tbody>
                  {MEMBERS.map((m) => (
                    <tr key={m.email}>
                      <td>
                        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                          <div className="user-avatar" style={{ background: `linear-gradient(135deg, ${m.color}, ${m.color}cc)` }}>{m.initials}</div>
                          <div>
                            <div style={{ fontWeight: 600 }}>{m.name}</div>
                            <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{m.email}</div>
                          </div>
                        </div>
                      </td>
                      <td><span className={`badge ${m.role === 'Owner' ? 'accent' : m.role === 'Admin' ? 'info' : ''}`}>{m.role}</span></td>
                      <td>{m.projects}</td>
                      <td>{m.mfa ? <span className="badge success"><I.check size={10} /> on</span> : <span className="badge warning">off</span>}</td>
                      <td style={{ color: 'var(--fg-muted)' }}>{m.last}</td>
                      <td><button className="icon-btn"><I.more size={14} /></button></td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="card" style={{ marginTop: 18 }}>
              <div className="card-header">
                <div className="card-title">Custom role: <span className="mono-key">payments-write</span></div>
                <button className="btn sm">Edit</button>
              </div>
              <div className="card-body">
                <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 14, fontSize: 12 }}>
                  <div>
                    <div style={{ color: 'var(--fg-muted)', fontSize: 10, textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 }}>Resources</div>
                    <div className="mono-key" style={{ display: 'block', marginBottom: 4 }}>flag:payments-*</div>
                    <div className="mono-key" style={{ display: 'block' }}>segment:high-risk-*</div>
                  </div>
                  <div>
                    <div style={{ color: 'var(--fg-muted)', fontSize: 10, textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 }}>Environments</div>
                    <div className="mono-key" style={{ display: 'block', marginBottom: 4 }}>staging</div>
                    <div className="mono-key" style={{ display: 'block' }}>qa</div>
                  </div>
                  <div>
                    <div style={{ color: 'var(--fg-muted)', fontSize: 10, textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 }}>Actions</div>
                    {['read', 'write', 'toggle'].map((a) => <span key={a} className="badge success" style={{ marginRight: 4, marginBottom: 4 }}><I.check size={10} />{a}</span>)}
                    {['delete', 'admin'].map((a) => <span key={a} className="badge" style={{ marginRight: 4, marginBottom: 4, opacity: 0.5 }}>{a}</span>)}
                  </div>
                </div>
              </div>
            </div>
          </>
        )}

        {tab !== 'members' && (
          <div className="card">
            <div className="empty">
              <div className="empty-icon"><I.users size={22} stroke="var(--fg-subtle)" /></div>
              <div className="empty-title">Coming soon</div>
              <div className="empty-desc">This tab will be implemented in a future release.</div>
            </div>
          </div>
        )}
      </div>
    </>
  )
}

// ─── Audit Log ───────────────────────────────────────────────────────────────

const ACTION_BADGE: Record<string, string> = {
  'flag.update': 'info',
  'flag.create': 'success',
  'experiment.start': 'accent',
  'segment.update': 'info',
  'sdk_key.rotate': 'warning',
  'member.invite': 'success',
  'event.register': 'info',
}

export function AuditLog() {
  const [search, setSearch] = useState('')
  const filtered = AUDIT.filter((a) =>
    !search || a.actor.toLowerCase().includes(search.toLowerCase()) || a.resource.includes(search) || a.action.includes(search)
  )

  return (
    <>
      <PageHeader
        crumbs={['Acme Cloud', 'Audit']}
        title="Audit Log"
        subtitle="Every mutation is recorded with actor, resource, environment and diff. Retained for 365 days."
        actions={<button className="btn"><I.download size={13} /> Export CSV</button>}
      />
      <div className="page-body">
        <div style={{ display: 'flex', gap: 8, marginBottom: 16, flexWrap: 'wrap' }}>
          <div className="search-input">
            <I.search size={14} />
            <input className="input" placeholder="Search actor, resource…" value={search} onChange={(e) => setSearch(e.target.value)} />
          </div>
          <button className="btn"><I.filter size={13} /> All actions</button>
          <button className="btn">All actors</button>
          <button className="btn">Last 7 days</button>
        </div>
        <div className="card">
          <table className="table">
            <thead>
              <tr><th>When</th><th>Actor</th><th>Action</th><th>Resource</th><th>Environment</th><th>Change</th><th></th></tr>
            </thead>
            <tbody>
              {filtered.map((a, i) => (
                <tr key={i}>
                  <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--fg-muted)' }}>{a.t}</td>
                  <td>
                    {a.actor === 'system'
                      ? <span style={{ display: 'flex', alignItems: 'center', gap: 6, color: 'var(--fg-muted)' }}><I.cog size={12} /> system</span>
                      : <strong>{a.actor}</strong>
                    }
                  </td>
                  <td><span className={`badge ${ACTION_BADGE[a.action] ?? ''}`}>{a.action}</span></td>
                  <td><span className="mono-key">{a.resource}</span></td>
                  <td>{a.env === '—' ? <span style={{ color: 'var(--fg-faint)' }}>—</span> : <span className="badge">{a.env}</span>}</td>
                  <td style={{ color: 'var(--fg-muted)' }}>{a.change}</td>
                  <td><button className="btn sm">Diff</button></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  )
}

// ─── Super Admin ─────────────────────────────────────────────────────────────

const ORGS = [
  ['Acme Cloud', 'priya@stitchd.dev', '3', '12', '2025-12-04', 'self-hosted'],
  ['Northwind Ops', 'ops@northwind.test', '1', '4', '2026-02-11', 'self-hosted'],
  ['Globex Internal', 'sre@globex.test', '2', '7', '2026-03-22', 'self-hosted'],
]

const DATASTORES = [
  ['PostgreSQL 16', 'postgres:5432', 'healthy', '6.1 GB', 'var(--success)'],
  ['ClickHouse 24', 'clickhouse:9000', 'healthy', '2.3 GB', 'var(--success)'],
  ['pg_partman', 'extension', 'loaded', '—', 'var(--info)'],
]

const AUTH_PROVIDERS = [
  ['Password', 'global', 'always-on', '—'],
  ['OIDC · Okta', 'acme.okta.com', 'org: Acme Cloud', '14 users'],
  ['SAML 2.0', 'aad-globex', 'org: Globex', '7 users'],
]

export function SuperAdmin() {
  return (
    <>
      <PageHeader
        crumbs={['Super Admin']}
        title="Super Admin"
        badge={<span className="badge danger"><I.shield size={11} /> bootstrap account</span>}
        subtitle="Provision organizations and seed their first owners. Super Admin has no access to project-level resources."
        actions={<button className="btn primary"><I.plus size={14} /> New organization</button>}
      />
      <div className="page-body">
        <div style={{ background: 'var(--warning-bg)', border: '1px solid rgba(184,118,26,0.35)', borderRadius: 8, padding: '10px 14px', marginBottom: 18, display: 'flex', alignItems: 'center', gap: 10 }}>
          <I.shield size={16} stroke="var(--warning)" />
          <div style={{ flex: 1, fontSize: 13 }}>
            <strong>Self-hosted bootstrap mode.</strong> This account exists above all organizations and is purely for provisioning. Once orgs have Owners, Super Admin should be retired.
          </div>
        </div>

        <div className="stat-grid" style={{ marginBottom: 18 }}>
          <div className="stat"><div className="stat-label">Organizations</div><div className="stat-value">3</div></div>
          <div className="stat"><div className="stat-label">Total users</div><div className="stat-value">42</div></div>
          <div className="stat"><div className="stat-label">DB size</div><div className="stat-value">8.4 GB</div><div className="stat-delta">PostgreSQL + ClickHouse</div></div>
          <div className="stat"><div className="stat-label">Uptime</div><div className="stat-value">23d 14h</div></div>
        </div>

        <div className="card">
          <div className="card-header"><div className="card-title"><I.building size={14} /> Organizations</div></div>
          <table className="table">
            <thead>
              <tr><th>Organization</th><th>Owner</th><th>Projects</th><th>Members</th><th>Created</th><th>Plan</th><th></th></tr>
            </thead>
            <tbody>
              {ORGS.map((r) => (
                <tr key={r[0]}>
                  <td><strong>{r[0]}</strong></td>
                  <td><span className="mono-key">{r[1]}</span></td>
                  <td>{r[2]}</td>
                  <td>{r[3]}</td>
                  <td style={{ fontFamily: 'var(--font-mono)', color: 'var(--fg-muted)' }}>{r[4]}</td>
                  <td><span className="badge">{r[5]}</span></td>
                  <td>
                    <div style={{ display: 'flex', gap: 4 }}>
                      <button className="btn sm">Provision user</button>
                      <button className="icon-btn"><I.more size={14} /></button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <div className="split-2" style={{ marginTop: 18 }}>
          <div className="card">
            <div className="card-header"><div className="card-title"><I.database size={14} /> Datastores</div></div>
            <div className="card-body">
              {DATASTORES.map(([name, addr, status, size, color]) => (
                <div key={name} style={{ display: 'flex', alignItems: 'center', padding: '8px 0', borderBottom: '1px solid var(--border-faint)', fontSize: 12 }}>
                  <span className="env-dot" style={{ background: color, boxShadow: `0 0 0 3px ${color}22` }} />
                  <span style={{ marginLeft: 10, fontFamily: 'var(--font-mono)', flex: 1 }}>{name}</span>
                  <span style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11, marginRight: 12 }}>{addr}</span>
                  <span className="badge success">{status}</span>
                  <span style={{ marginLeft: 12, fontFamily: 'var(--font-mono)', fontWeight: 600 }}>{size}</span>
                </div>
              ))}
            </div>
          </div>
          <div className="card">
            <div className="card-header">
              <div className="card-title"><I.fingerprint size={14} /> Auth providers</div>
              <button className="btn sm"><I.plus size={12} /> Add</button>
            </div>
            <div className="card-body">
              {AUTH_PROVIDERS.map(([name, host, scope, users]) => (
                <div key={name} style={{ display: 'flex', alignItems: 'center', padding: '10px 0', borderBottom: '1px solid var(--border-faint)', gap: 10 }}>
                  <I.lock size={14} stroke="var(--fg-muted)" />
                  <div style={{ flex: 1 }}>
                    <div style={{ fontWeight: 600, fontSize: 13 }}>{name}</div>
                    <div style={{ fontSize: 11, color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)' }}>{host} · {scope}</div>
                  </div>
                  <span className="badge">{users}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>
    </>
  )
}
