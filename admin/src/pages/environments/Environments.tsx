import { useCallback, useEffect, useRef, useState } from 'react'
import type { EnvironmentSummary, SdkKeySummary } from '../../lib/api'
import {
  createEnvironment,
  createSdkKey,
  deleteEnvironment,
  listEnvironments,
  listSdkKeys,
  renameEnvironment,
  revokeSdkKey,
} from '../../lib/api'
import { I } from '../../components/icons'
import { PageHeader } from '../../components/primitives'
import { useOrgContext } from '../../context/OrgContext'
import { usePermissions } from '../../hooks/usePermissions'
import { PERMISSIONS } from '../../lib/permissions'

// ─── Section lock overlay ─────────────────────────────────────────────────────

function LockOverlay() {
  return (
    <div style={{ position: 'relative' }}>
      <PageHeader
        crumbs={['Environments']}
        title="Environments & SDK Keys"
        subtitle=""
      />
      <div className="page-body">
        <div className="card" style={{ padding: 48, textAlign: 'center' }}>
          <I.lock size={28} stroke="var(--fg-subtle)" style={{ marginBottom: 12 }} />
          <div style={{ fontWeight: 600, fontSize: 15, marginBottom: 6 }}>Access restricted</div>
          <div style={{ color: 'var(--fg-muted)', fontSize: 13 }}>
            You do not have permission to view environments for this project.
          </div>
        </div>
      </div>
    </div>
  )
}

// ─── Empty state (no project selected) ───────────────────────────────────────

function NoProjectSelected() {
  return (
    <div>
      <PageHeader
        crumbs={['Environments']}
        title="Environments & SDK Keys"
        subtitle="Select a project to manage its environments and SDK keys."
      />
      <div className="page-body">
        <div className="card" style={{ padding: 48, textAlign: 'center' }}>
          <I.key size={28} stroke="var(--fg-subtle)" style={{ marginBottom: 12 }} />
          <div style={{ fontWeight: 600, fontSize: 15, marginBottom: 6 }}>No project selected</div>
          <div style={{ color: 'var(--fg-muted)', fontSize: 13, maxWidth: 320, margin: '0 auto' }}>
            Choose a project from the sidebar to view and manage its environments and SDK keys.
          </div>
        </div>
      </div>
    </div>
  )
}

// ─── Raw key reveal banner (shown once after key creation) ───────────────────

function NewKeyBanner({ rawKey, onDismiss }: { rawKey: string; onDismiss: () => void }) {
  const [copied, setCopied] = useState(false)

  function copy() {
    void navigator.clipboard.writeText(rawKey)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  return (
    <div style={{
      background: 'var(--success-bg)',
      border: '1px solid rgba(34,160,96,0.35)',
      borderRadius: 8,
      padding: '12px 16px',
      display: 'flex',
      alignItems: 'center',
      gap: 12,
      marginBottom: 16,
    }}>
      <I.check size={16} stroke="var(--success)" />
      <div style={{ flex: 1 }}>
        <div style={{ fontWeight: 600, fontSize: 13, marginBottom: 4 }}>SDK key created — copy it now</div>
        <code style={{ fontFamily: 'var(--font-mono)', fontSize: 12, background: 'rgba(0,0,0,0.08)', padding: '2px 6px', borderRadius: 4 }}>
          {rawKey}
        </code>
      </div>
      <button className="btn sm" onClick={copy}><I.copy size={12} /> {copied ? 'Copied!' : 'Copy'}</button>
      <button className="icon-btn" onClick={onDismiss}><I.x size={14} /></button>
    </div>
  )
}

// ─── SDK Keys section ─────────────────────────────────────────────────────────

interface SdkKeysSectionProps {
  environmentId: string
  environmentName: string
  canCreate: boolean
  canRevoke: boolean
}

function SdkKeysSection({ environmentId, environmentName, canCreate, canRevoke }: SdkKeysSectionProps) {
  const [keys, setKeys] = useState<SdkKeySummary[]>([])
  const [loading, setLoading] = useState(true)
  const [newRawKey, setNewRawKey] = useState<string | null>(null)
  const [revokingId, setRevokingId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(() => {
    setLoading(true)
    listSdkKeys(environmentId)
      .then(setKeys)
      .catch(() => setKeys([]))
      .finally(() => setLoading(false))
  }, [environmentId])

  useEffect(() => { load() }, [load])

  async function handleCreate() {
    setBusy(true)
    setError(null)
    try {
      const res = await createSdkKey(environmentId)
      setNewRawKey(res.raw_key)
      load()
    } catch {
      setError('Failed to create SDK key')
    } finally {
      setBusy(false)
    }
  }

  async function handleRevoke(keyId: string) {
    setBusy(true)
    setError(null)
    try {
      await revokeSdkKey(environmentId, keyId)
      setRevokingId(null)
      load()
    } catch (err: unknown) {
      const status = (err as { response?: { status?: number } }).response?.status
      if (status === 409) {
        setError('Cannot revoke the last active SDK key for this environment')
      } else {
        setError('Revoke failed')
      }
      setRevokingId(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="card" style={{ marginTop: 18 }}>
      <div className="card-header">
        <div className="card-title">
          <I.key size={14} /> SDK Keys — <span style={{ fontFamily: 'var(--font-mono)', fontWeight: 400 }}>{environmentName}</span>
        </div>
        <button
          className="btn primary sm"
          disabled={!canCreate || busy}
          title={canCreate ? 'Create new SDK key' : 'No permission to create SDK keys'}
          onClick={() => void handleCreate()}
        >
          <I.plus size={12} /> New key
        </button>
      </div>
      <div style={{ padding: '0 16px' }}>
        {error && (
          <div style={{ fontSize: 12, color: 'var(--danger)', padding: '8px 0' }}>{error}</div>
        )}
        {newRawKey && <NewKeyBanner rawKey={newRawKey} onDismiss={() => setNewRawKey(null)} />}
      </div>
      {loading ? (
        <div style={{ fontSize: 12, color: 'var(--fg-subtle)', padding: '12px 16px' }}>Loading…</div>
      ) : keys.length === 0 ? (
        <div style={{ padding: '24px 16px', textAlign: 'center', color: 'var(--fg-muted)', fontSize: 13 }}>
          No SDK keys yet.
        </div>
      ) : (
        <table className="table">
          <thead>
            <tr><th>Key ID</th><th>Created</th><th>Revoked</th><th>Status</th><th></th></tr>
          </thead>
          <tbody>
            {keys.map((k) => (
              <tr key={k.sdk_key_id}>
                <td><span className="mono-key">{k.sdk_key_id.slice(0, 16)}…</span></td>
                <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--fg-muted)' }}>
                  {new Date(k.created_at).toLocaleDateString()}
                </td>
                <td style={{ color: 'var(--fg-muted)', fontSize: 12 }}>
                  {k.revoked_at ? new Date(k.revoked_at).toLocaleDateString() : '—'}
                </td>
                <td>
                  <span className={`badge ${k.is_active ? 'success' : 'warning'}`}>
                    <span className="dot" />{k.is_active ? 'active' : 'revoked'}
                  </span>
                </td>
                <td>
                  {k.is_active && revokingId === k.sdk_key_id ? (
                    <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                      <span style={{ fontSize: 11, color: 'var(--fg-subtle)' }}>Revoke?</span>
                      <button className="btn sm danger" disabled={busy} onClick={() => void handleRevoke(k.sdk_key_id)}>Confirm</button>
                      <button className="btn sm" disabled={busy} onClick={() => setRevokingId(null)}>Cancel</button>
                    </div>
                  ) : k.is_active ? (
                    <button
                      className="btn sm danger"
                      disabled={!canRevoke || busy}
                      title={canRevoke ? 'Revoke this key' : 'No permission to revoke SDK keys'}
                      style={{ opacity: canRevoke ? 1 : 0.45 }}
                      onClick={() => setRevokingId(k.sdk_key_id)}
                    >
                      Revoke
                    </button>
                  ) : null}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}

// ─── Main page ────────────────────────────────────────────────────────────────

export function Environments() {
  const { projectId, envId, setEnvId } = useOrgContext()
  const { can, loading: permLoading } = usePermissions()

  const [envs, setEnvs] = useState<EnvironmentSummary[]>([])
  const [envsLoading, setEnvsLoading] = useState(false)
  const [creatingEnv, setCreatingEnv] = useState(false)
  const [newEnvName, setNewEnvName] = useState('')
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const [deletingId, setDeletingId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const newEnvRef = useRef<HTMLInputElement>(null)
  const renameRef = useRef<HTMLInputElement>(null)

  const canRead = can(PERMISSIONS.ENVIRONMENT_READ)
  const canCreate = can(PERMISSIONS.ENVIRONMENT_CREATE)
  const canRename = can(PERMISSIONS.ENVIRONMENT_RENAME)
  const canDelete = can(PERMISSIONS.ENVIRONMENT_DELETE)
  const canCreateKey = can(PERMISSIONS.SDK_KEY_CREATE)
  const canRevokeKey = can(PERMISSIONS.SDK_KEY_REVOKE)

  const loadEnvs = useCallback(() => {
    if (!projectId) return
    setEnvsLoading(true)
    listEnvironments(projectId)
      .then((list) => {
        setEnvs(list)
        if (list.length > 0 && !envId) setEnvId(list[0].environment_id)
      })
      .catch(() => setEnvs([]))
      .finally(() => setEnvsLoading(false))
  }, [projectId, envId, setEnvId])

  useEffect(() => { loadEnvs() }, [loadEnvs])

  if (!projectId) return <NoProjectSelected />
  if (!permLoading && !canRead) return <LockOverlay />

  const selectedEnv = envs.find((e) => e.environment_id === envId) ?? envs[0] ?? null

  async function handleCreateEnv() {
    if (!newEnvName.trim() || !projectId) return
    setBusy(true)
    setError(null)
    try {
      const res = await createEnvironment(projectId, newEnvName.trim())
      setCreatingEnv(false)
      setNewEnvName('')
      loadEnvs()
      setEnvId(res.environment_id)
    } catch {
      setError('Failed to create environment')
    } finally {
      setBusy(false)
    }
  }

  async function handleRename(id: string) {
    if (!renameValue.trim()) return
    setBusy(true)
    setError(null)
    try {
      await renameEnvironment(id, renameValue.trim())
      setRenamingId(null)
      loadEnvs()
    } catch {
      setError('Rename failed')
    } finally {
      setBusy(false)
    }
  }

  async function handleDelete(id: string) {
    setBusy(true)
    setError(null)
    try {
      await deleteEnvironment(id)
      setDeletingId(null)
      if (envId === id) setEnvId(envs.find((e) => e.environment_id !== id)?.environment_id ?? '')
      loadEnvs()
    } catch {
      setError('Delete failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <PageHeader
        crumbs={['Environments']}
        title="Environments & SDK Keys"
        subtitle="Each environment scopes its own rules, segments, experiments and SDK keys. Min 1 active key per environment."
        actions={
          <button
            className="btn primary"
            disabled={!canCreate || busy}
            title={canCreate ? 'Create new environment' : 'No permission to create environments'}
            style={{ opacity: canCreate ? 1 : 0.45 }}
            onClick={() => {
              setCreatingEnv(true)
              setNewEnvName('')
              setTimeout(() => newEnvRef.current?.focus(), 0)
            }}
          >
            <I.plus size={14} /> New environment
          </button>
        }
      />
      <div className="page-body">
        {error && (
          <div style={{ fontSize: 12, color: 'var(--danger)', marginBottom: 12, padding: '8px 12px', background: 'var(--danger-bg)', borderRadius: 6 }}>
            {error}
          </div>
        )}

        {/* Inline create form */}
        {creatingEnv && (
          <div className="card" style={{ padding: '12px 16px', marginBottom: 12, display: 'flex', gap: 8, alignItems: 'center' }}>
            <input
              ref={newEnvRef}
              className="input"
              placeholder="Environment name"
              value={newEnvName}
              onChange={(e) => setNewEnvName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void handleCreateEnv()
                if (e.key === 'Escape') { setCreatingEnv(false); setNewEnvName('') }
              }}
              disabled={busy}
              style={{ maxWidth: 240 }}
              autoFocus
            />
            <button className="btn primary sm" disabled={busy} onClick={() => void handleCreateEnv()}>Create</button>
            <button className="btn sm" disabled={busy} onClick={() => { setCreatingEnv(false); setNewEnvName('') }}>Cancel</button>
          </div>
        )}

        {/* Environment cards */}
        {envsLoading ? (
          <div style={{ fontSize: 12, color: 'var(--fg-subtle)', padding: '8px 0' }}>Loading environments…</div>
        ) : envs.length === 0 && !creatingEnv ? (
          <div className="card" style={{ padding: 32, textAlign: 'center', color: 'var(--fg-muted)', fontSize: 13 }}>
            No environments yet.{' '}
            {canCreate && (
              <button className="link-btn" onClick={() => { setCreatingEnv(true); setTimeout(() => newEnvRef.current?.focus(), 0) }}>
                Create one
              </button>
            )}
          </div>
        ) : (
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(200px, 1fr))', gap: 10 }}>
            {envs.map((env) => {
              const isSelected = env.environment_id === (envId ?? selectedEnv?.environment_id)

              if (renamingId === env.environment_id) {
                return (
                  <div key={env.environment_id} className={`card env-card ${isSelected ? 'env-card--selected' : ''}`} style={{ padding: '12px 14px' }}>
                    <input
                      ref={renameRef}
                      className="input"
                      value={renameValue}
                      onChange={(e) => setRenameValue(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') void handleRename(env.environment_id)
                        if (e.key === 'Escape') setRenamingId(null)
                      }}
                      disabled={busy}
                      autoFocus
                      style={{ marginBottom: 8 }}
                    />
                    <div style={{ display: 'flex', gap: 6 }}>
                      <button className="btn primary sm" disabled={busy} onClick={() => void handleRename(env.environment_id)}><I.check size={12} /></button>
                      <button className="btn sm" disabled={busy} onClick={() => setRenamingId(null)}><I.x size={12} /></button>
                    </div>
                  </div>
                )
              }

              if (deletingId === env.environment_id) {
                return (
                  <div key={env.environment_id} className="card env-card" style={{ padding: '12px 14px', border: '1px solid var(--danger)' }}>
                    <div style={{ fontSize: 12, marginBottom: 10, color: 'var(--fg)' }}>
                      Delete <strong>{env.environment_name}</strong>?
                    </div>
                    <div style={{ display: 'flex', gap: 6 }}>
                      <button className="btn sm danger" disabled={busy} onClick={() => void handleDelete(env.environment_id)}>Delete</button>
                      <button className="btn sm" disabled={busy} onClick={() => setDeletingId(null)}>Cancel</button>
                    </div>
                  </div>
                )
              }

              return (
                <div
                  key={env.environment_id}
                  className={`card env-card ${isSelected ? 'env-card--selected' : ''}`}
                  style={{ padding: '12px 14px', cursor: 'pointer', position: 'relative' }}
                  onClick={() => setEnvId(env.environment_id)}
                >
                  <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
                    <span className="env-dot" />
                    <span style={{ fontFamily: 'var(--font-mono)', fontWeight: 700, fontSize: 13, flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {env.environment_name}
                    </span>
                  </div>
                  <div style={{ fontSize: 11, color: 'var(--fg-subtle)', marginBottom: 10 }}>
                    Created {new Date(env.created_at).toLocaleDateString()}
                  </div>
                  <div style={{ display: 'flex', gap: 4 }}>
                    <button
                      className="icon-btn"
                      title={canRename ? 'Rename' : 'No permission to rename'}
                      disabled={!canRename || busy}
                      style={{ opacity: canRename ? 1 : 0.35 }}
                      onClick={(e) => {
                        e.stopPropagation()
                        setRenamingId(env.environment_id)
                        setRenameValue(env.environment_name)
                        setTimeout(() => renameRef.current?.focus(), 0)
                      }}
                    >
                      <I.pencil size={12} />
                    </button>
                    <button
                      className="icon-btn icon-btn--danger"
                      title={canDelete ? 'Delete' : 'No permission to delete'}
                      disabled={!canDelete || busy}
                      style={{ opacity: canDelete ? 1 : 0.35 }}
                      onClick={(e) => { e.stopPropagation(); setDeletingId(env.environment_id) }}
                    >
                      <I.trash size={12} />
                    </button>
                  </div>
                </div>
              )
            })}
          </div>
        )}

        {/* SDK Keys for selected environment */}
        {selectedEnv && (
          <SdkKeysSection
            key={selectedEnv.environment_id}
            environmentId={selectedEnv.environment_id}
            environmentName={selectedEnv.environment_name}
            canCreate={canCreateKey}
            canRevoke={canRevokeKey}
          />
        )}
      </div>
    </>
  )
}
