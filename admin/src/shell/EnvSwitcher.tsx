import { useState, useRef, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { I } from '../components/icons'
import { useOrgContext } from '../context/OrgContext'

export function EnvSwitcher() {
  const navigate = useNavigate()
  const { orgId, envId, setEnvId, environments, envsLoading, projectId } = useOrgContext()
  const [open, setOpen] = useState(false)
  const dropRef = useRef<HTMLDivElement>(null)

  // Fall back to first env if stored envId doesn't match (e.g. after project switch)
  const currentEnv = environments.find((e) => e.environment_id === envId) ?? environments[0]

  // Sync envId when the fallback kicks in
  useEffect(() => {
    if (currentEnv && currentEnv.environment_id !== envId) {
      setEnvId(currentEnv.environment_id)
    }
  }, [currentEnv, envId, setEnvId])

  const displayName = currentEnv?.environment_name
    ?? (envsLoading ? 'Loading…' : projectId ? 'No environment' : 'No project')

  useEffect(() => {
    function onOutside(e: MouseEvent) {
      if (dropRef.current && !dropRef.current.contains(e.target as Node)) setOpen(false)
    }
    if (open) document.addEventListener('mousedown', onOutside)
    return () => document.removeEventListener('mousedown', onOutside)
  }, [open])

  const envPath = `/org/${orgId}/environments`

  return (
    <div ref={dropRef} style={{ margin: '0 12px 8px', position: 'relative' }}>
      <div
        className="env-pill"
        style={{ cursor: 'pointer', userSelect: 'none' }}
        onClick={() => { if (environments.length > 0) setOpen((v) => !v); else navigate(envPath) }}
      >
        <span className="env-dot" />
        <span className="env-name" style={{ flex: 1 }}>{displayName}</span>
        {environments.length > 0
          ? <I.chevronDown size={12} style={{ color: 'var(--fg-subtle)', transform: open ? 'rotate(180deg)' : undefined, transition: 'transform 0.15s' }} />
          : <span className="env-switch" onClick={(e) => { e.stopPropagation(); navigate(envPath) }}>manage</span>
        }
      </div>

      {open && environments.length > 1 && (
        <div style={{
          position: 'absolute', top: '100%', left: 0, right: 0, zIndex: 100,
          background: 'var(--surface)', border: '1px solid var(--border)',
          borderRadius: 8, boxShadow: 'var(--shadow-md)', overflow: 'hidden',
          marginTop: 4,
        }}>
          <div style={{ padding: '6px 14px 4px', fontSize: 10, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
            Switch to
          </div>
          {environments.map((env) => {
            const isActive = env.environment_id === envId
            return (
              <button
                key={env.environment_id}
                className="sidebar-link"
                style={{ display: 'flex', alignItems: 'center', gap: 10, width: '100%', padding: '9px 14px', background: 'none', border: 'none', cursor: 'pointer', textAlign: 'left', color: 'var(--fg)' }}
                onClick={() => { setEnvId(env.environment_id); setOpen(false) }}
              >
                <span className="env-dot" style={{ flexShrink: 0 }} />
                <span style={{ flex: 1, fontSize: 13, fontWeight: isActive ? 600 : 400, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {env.environment_name}
                </span>
                {isActive && <I.check size={13} style={{ color: 'var(--accent)', flexShrink: 0 }} />}
              </button>
            )
          })}
          <button
            className="sidebar-link"
            style={{ display: 'flex', alignItems: 'center', gap: 8, width: '100%', padding: '9px 14px', color: 'var(--fg-muted)', fontSize: 13, background: 'none', border: 'none', borderTop: '1px solid var(--border)', cursor: 'pointer' }}
            onClick={() => { setOpen(false); navigate(envPath) }}
          >
            <I.key size={13} />
            Manage environments
          </button>
        </div>
      )}
    </div>
  )
}
