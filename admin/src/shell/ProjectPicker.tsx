import { useRef, useState } from 'react'
import { createProject, deleteProject, renameProject } from '../lib/api'
import { I } from '../components/icons'
import { useOrgContext } from '../context/OrgContext'
import { usePermissions } from '../hooks/usePermissions'
import { PERMISSIONS } from '../lib/permissions'

export function ProjectPicker() {
  const { orgId, projectId, setProjectId, projects, projectsLoading, refreshProjects } =
    useOrgContext()
  const { can } = usePermissions()

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
      refreshProjects()
      setProjectId(p.project_id)
    } catch {
      setError('Create failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="project-picker">
      <div className="project-picker-header">
        <span className="sidebar-section-label" style={{ padding: 0 }}>Project</span>
        {canCreate && (
          <button
            className="icon-btn"
            title="New project"
            onClick={() => { setCreating(true); setNewName(''); setTimeout(() => newRef.current?.focus(), 0) }}
          >
            <I.plus size={13} />
          </button>
        )}
      </div>

      {error && (
        <div style={{ fontSize: 11, color: 'var(--danger)', padding: '2px 4px' }}>{error}</div>
      )}

      <div className="project-list">
        {projectsLoading && (
          <div style={{ fontSize: 11, color: 'var(--fg-subtle)', padding: '4px 6px' }}>Loading…</div>
        )}

        {!projectsLoading && projects.length === 0 && !creating && (
          <div style={{ fontSize: 11, color: 'var(--fg-subtle)', padding: '4px 6px' }}>No projects yet</div>
        )}

        {projects.map((p) => {
          const isSelected = p.project_id === projectId

          if (renamingId === p.project_id) {
            return (
              <div key={p.project_id} className="project-item project-item--renaming">
                <input
                  ref={renameRef}
                  className="input project-rename-input"
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
              <div key={p.project_id} className="project-item project-item--confirm">
                <span style={{ fontSize: 11, color: 'var(--fg-subtle)', flex: 1 }}>Delete "{p.project_name}"?</span>
                <button className="btn btn--danger btn--xs" disabled={busy} onClick={() => void submitDelete(p.project_id)}>Delete</button>
                <button className="btn btn--xs" disabled={busy} onClick={() => setDeletingId(null)}>Cancel</button>
              </div>
            )
          }

          return (
            <div
              key={p.project_id}
              className={`project-item ${isSelected ? 'project-item--active' : ''}`}
              onClick={() => setProjectId(p.project_id)}
            >
              <span className="project-item-name">{p.project_name}</span>
              <div className="project-item-actions">
                {canRename && (
                  <button
                    className="icon-btn"
                    title="Rename"
                    onClick={(e) => {
                      e.stopPropagation()
                      setRenamingId(p.project_id)
                      setRenameValue(p.project_name)
                      setTimeout(() => renameRef.current?.focus(), 0)
                    }}
                  >
                    <I.pencil size={12} />
                  </button>
                )}
                {!canRename && (
                  <button className="icon-btn" title="No permission to rename" disabled style={{ opacity: 0.35 }}>
                    <I.pencil size={12} />
                  </button>
                )}
                {canDelete && (
                  <button
                    className="icon-btn icon-btn--danger"
                    title="Delete"
                    onClick={(e) => { e.stopPropagation(); setDeletingId(p.project_id) }}
                  >
                    <I.trash size={12} />
                  </button>
                )}
                {!canDelete && (
                  <button className="icon-btn" title="No permission to delete" disabled style={{ opacity: 0.35 }}>
                    <I.trash size={12} />
                  </button>
                )}
              </div>
            </div>
          )
        })}

        {creating && (
          <div className="project-item project-item--creating">
            <input
              ref={newRef}
              className="input project-rename-input"
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
      </div>
    </div>
  )
}
