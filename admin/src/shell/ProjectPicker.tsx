import { useState, useRef, useEffect } from 'react'
import { createProject, deleteProject, renameProject } from '../lib/api'
import { I } from '../components/icons'
import { Dropdown } from '../components/Dropdown'
import { useOrgContext } from '../context/OrgContext'
import { usePermissions } from '../hooks/usePermissions'
import { PERMISSIONS } from '../lib/permissions'

function initials(name: string): string {
  return name
    .split(/\s+/)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? '')
    .join('')
}

export function ProjectPicker() {
  const { orgId, projectId, setProjectId, projects, projectsLoading, refreshProjects } =
    useOrgContext()
  const { can } = usePermissions()

  const [open, setOpen] = useState(false)
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const [deletingId, setDeletingId] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [newName, setNewName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const renameRef = useRef<HTMLInputElement>(null)
  const newRef = useRef<HTMLInputElement>(null)

  const canCreate = can(PERMISSIONS.PROJECT_CREATE)
  const canRename = can(PERMISSIONS.PROJECT_RENAME)
  const canDelete = can(PERMISSIONS.PROJECT_DELETE)

  const currentProject = projects.find((p) => p.project_id === projectId)
  const displayName = currentProject?.project_name ?? (projectsLoading ? 'Loading…' : 'Select project')
  const abbr = currentProject ? initials(currentProject.project_name) : '—'

  // Auto-select first project when list loads and nothing is selected
  useEffect(() => {
    if (!projectId && projects.length > 0) {
      setProjectId(projects[0].project_id)
    }
  }, [projects, projectId, setProjectId])

  async function submitRename(id: string) {
    if (!renameValue.trim()) return
    setBusy(true)
    setError(null)
    try {
      await renameProject(id, renameValue.trim())
      setRenamingId(null)
      refreshProjects()
    } catch {
      setError('Rename failed')
    } finally {
      setBusy(false)
    }
  }

  async function submitDelete(id: string) {
    setBusy(true)
    setError(null)
    try {
      await deleteProject(id)
      setDeletingId(null)
      if (projectId === id) setProjectId('')
      refreshProjects()
    } catch {
      setError('Delete failed')
    } finally {
      setBusy(false)
    }
  }

  async function submitCreate() {
    if (!newName.trim()) return
    setBusy(true)
    setError(null)
    try {
      const p = await createProject(orgId, newName.trim())
      setCreating(false)
      setNewName('')
      setOpen(false)
      refreshProjects()
      setProjectId(p.project_id)
    } catch {
      setError('Create failed')
    } finally {
      setBusy(false)
    }
  }

  const trigger = (
    <button
      className="org-switcher button-reset"
      onClick={() => setOpen((v) => !v)}
      style={{ width: '100%', cursor: 'pointer', textAlign: 'left' }}
      title={`Current project: ${displayName}`}
    >
      <div className="org-avatar" style={{ background: 'linear-gradient(135deg, #1a3a5c, #0d1f33)' }}>
        {abbr}
      </div>
      <div className="org-meta">
        <div style={{ fontSize: 10, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.05em', fontWeight: 600, marginBottom: 1 }}>
          Project
        </div>
        <div className="org-name">{displayName}</div>
      </div>
      <I.chevronDown
        size={14}
        className="org-chevron"
        style={{ transform: open ? 'rotate(180deg)' : undefined, transition: 'transform 0.15s' }}
      />
    </button>
  )

  return (
    <Dropdown trigger={trigger} isOpen={open} onOpenChange={(v) => {
      setOpen(v)
      if (!v) { setRenamingId(null); setDeletingId(null); setCreating(false) }
    }}>
      {error && (
        <div style={{ padding: '6px 14px', fontSize: 11, color: 'var(--danger)', borderBottom: '1px solid var(--border)' }}>
          {error}
        </div>
      )}

      {projects.length === 0 && !creating && (
        <div style={{ padding: '12px 14px', fontSize: 12, color: 'var(--fg-muted)' }}>
          No projects yet
        </div>
      )}

      {projects.length > 0 && (
        <>
          <div style={{ padding: '6px 14px 4px', fontSize: 10, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
            Switch to
          </div>
          {projects.map((p) => {
            const isSelected = p.project_id === projectId

            if (renamingId === p.project_id) {
              return (
                <div key={p.project_id} style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '6px 10px', borderTop: '1px solid var(--border)' }}>
                  <input
                    ref={renameRef}
                    className="input"
                    style={{ flex: 1, fontSize: 13, padding: '4px 8px' }}
                    value={renameValue}
                    onChange={(e) => setRenameValue(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') void submitRename(p.project_id)
                      if (e.key === 'Escape') setRenamingId(null)
                    }}
                    disabled={busy}
                    autoFocus
                  />
                  <button className="icon-btn" disabled={busy} onClick={() => void submitRename(p.project_id)}><I.check size={12} /></button>
                  <button className="icon-btn" disabled={busy} onClick={() => setRenamingId(null)}><I.x size={12} /></button>
                </div>
              )
            }

            if (deletingId === p.project_id) {
              return (
                <div key={p.project_id} style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '8px 14px', borderTop: '1px solid var(--border)' }}>
                  <span style={{ flex: 1, fontSize: 12, color: 'var(--fg-muted)' }}>Delete "{p.project_name}"?</span>
                  <button className="btn btn--danger btn--xs" disabled={busy} onClick={() => void submitDelete(p.project_id)}>Delete</button>
                  <button className="btn btn--xs" disabled={busy} onClick={() => setDeletingId(null)}>Cancel</button>
                </div>
              )
            }

            return (
              <div
                key={p.project_id}
                style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '8px 14px', cursor: 'pointer' }}
                className="sidebar-link dropdown-item"
                onClick={() => { setProjectId(p.project_id); setOpen(false) }}
              >
                <div className="org-avatar" style={{ width: 24, height: 24, fontSize: 10, borderRadius: 5, flexShrink: 0, background: 'linear-gradient(135deg, #1a3a5c, #0d1f33)' }}>
                  {initials(p.project_name)}
                </div>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 13, fontWeight: isSelected ? 600 : 400, color: 'var(--fg)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {p.project_name}
                  </div>
                </div>
                {isSelected && <I.check size={13} style={{ color: 'var(--accent)', flexShrink: 0 }} />}
                <div style={{ display: 'flex', gap: 2, flexShrink: 0 }} onClick={(e) => e.stopPropagation()}>
                  <button
                    className="icon-btn"
                    title={canRename ? 'Rename' : 'No permission to rename'}
                    disabled={!canRename || busy}
                    style={{ opacity: canRename ? 1 : 0.35 }}
                    onClick={() => {
                      setRenamingId(p.project_id)
                      setRenameValue(p.project_name)
                      setTimeout(() => renameRef.current?.focus(), 0)
                    }}
                  >
                    <I.pencil size={11} />
                  </button>
                  <button
                    className="icon-btn icon-btn--danger"
                    title={canDelete ? 'Delete' : 'No permission to delete'}
                    disabled={!canDelete || busy}
                    style={{ opacity: canDelete ? 1 : 0.35 }}
                    onClick={() => setDeletingId(p.project_id)}
                  >
                    <I.trash size={11} />
                  </button>
                </div>
              </div>
            )
          })}
        </>
      )}

      {creating && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '6px 10px', borderTop: projects.length > 0 ? '1px solid var(--border)' : undefined }}>
          <input
            ref={newRef}
            className="input"
            style={{ flex: 1, fontSize: 13, padding: '4px 8px' }}
            placeholder="Project name"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submitCreate()
              if (e.key === 'Escape') { setCreating(false); setNewName('') }
            }}
            disabled={busy}
            autoFocus
          />
          <button className="icon-btn" disabled={busy} onClick={() => void submitCreate()}><I.check size={12} /></button>
          <button className="icon-btn" disabled={busy} onClick={() => { setCreating(false); setNewName('') }}><I.x size={12} /></button>
        </div>
      )}

      {canCreate && !creating && (
        <button
          className="sidebar-link dropdown-item"
          style={{
            display: 'flex', alignItems: 'center', gap: 8,
            width: '100%', padding: '9px 14px',
            borderTop: '1px solid var(--border)',
            color: 'var(--fg-muted)', fontSize: 13,
          }}
          onClick={() => { setCreating(true); setNewName(''); setTimeout(() => newRef.current?.focus(), 0) }}
        >
          <I.plus size={13} />
          New project
        </button>
      )}
    </Dropdown>
  )
}
