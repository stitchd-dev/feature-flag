import { useState, useEffect, useRef } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import type { Segment } from './types'
import { EditSegmentModal } from './EditSegmentModal'

// ─── CSV import modal ──────────────────────────────────────────────────────────

type CsvAction = 'merge' | 'replace' | 'remove'
type ListTarget = 'include' | 'exclude'

interface CsvImportModalProps {
  contextType: string
  onClose: () => void
  onApply: (keys: string[], action: CsvAction, target: ListTarget) => Promise<void>
  saving: boolean
}

function CsvImportModal({ contextType, onClose, onApply, saving }: CsvImportModalProps) {
  const [parsedKeys, setParsedKeys] = useState<string[]>([])
  const [action, setAction] = useState<CsvAction>('merge')
  const [target, setTarget] = useState<ListTarget>('include')
  const [fileName, setFileName] = useState('')
  const fileRef = useRef<HTMLInputElement>(null)

  function handleFile(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0]
    if (!file) return
    setFileName(file.name)
    const reader = new FileReader()
    reader.onload = (ev) => {
      const text = ev.target?.result as string
      // Parse: split by newlines and commas, strip quotes, deduplicate
      const keys = text
        .split(/[\r\n,]+/)
        .map((k) => k.replace(/^["']|["']$/g, '').trim())
        .filter((k) => k.length > 0)
      setParsedKeys([...new Set(keys)])
    }
    reader.readAsText(file)
  }

  const actionDescriptions: Record<CsvAction, string> = {
    merge: 'Add CSV keys to the existing list (union)',
    replace: 'Replace the entire list with CSV keys',
    remove: 'Remove CSV keys from the list',
  }

  // Map UI action names to API action values
  const apiAction: Record<CsvAction, string> = {
    merge: 'add',
    replace: 'replace',
    remove: 'remove',
  }

  return (
    <div style={{ position: 'fixed', inset: 0, zIndex: 200, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
      <div style={{ position: 'absolute', inset: 0, background: 'rgba(0,0,0,0.5)' }} onClick={onClose} />
      <div className="card" style={{ position: 'relative', width: 520, maxHeight: '85vh', overflow: 'auto', zIndex: 1, padding: 0 }}>
        <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)' }}>
          <div className="card-title"><I.upload size={14} /> Import CSV</div>
          <button className="icon-btn" onClick={onClose}><I.x size={16} /></button>
        </div>
        <div style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 16 }}>

          {/* File picker */}
          <div>
            <label className="label">CSV File</label>
            <div
              style={{ border: '2px dashed var(--border)', borderRadius: 8, padding: '24px 16px', textAlign: 'center', cursor: 'pointer', background: 'var(--bg-sunken)' }}
              onClick={() => fileRef.current?.click()}
            >
              {fileName ? (
                <div style={{ fontSize: 13, fontWeight: 600 }}>{fileName} — {parsedKeys.length} keys found</div>
              ) : (
                <>
                  <I.upload size={18} style={{ color: 'var(--fg-muted)', marginBottom: 6 }} />
                  <div style={{ fontSize: 13, color: 'var(--fg-muted)' }}>Click to upload a CSV file</div>
                  <div style={{ fontSize: 11, color: 'var(--fg-faint)', marginTop: 4 }}>
                    One <code>{contextType}</code> key per row or column
                  </div>
                </>
              )}
            </div>
            <input ref={fileRef} type="file" accept=".csv,.txt" style={{ display: 'none' }} onChange={handleFile} />
          </div>

          {parsedKeys.length > 0 && (
            <>
              {/* Preview */}
              <div>
                <label className="label">Preview ({parsedKeys.length} keys)</label>
                <div style={{
                  background: 'var(--bg-sunken)', border: '1px solid var(--border)', borderRadius: 6,
                  padding: '8px 12px', fontFamily: 'var(--font-mono)', fontSize: 12,
                  maxHeight: 120, overflow: 'auto', lineHeight: 1.8,
                }}>
                  {parsedKeys.slice(0, 20).join('\n')}{parsedKeys.length > 20 ? `\n… and ${parsedKeys.length - 20} more` : ''}
                </div>
              </div>

              {/* Target list */}
              <div>
                <label className="label">Apply to</label>
                <div style={{ display: 'flex', gap: 8 }}>
                  {(['include', 'exclude'] as ListTarget[]).map((t) => (
                    <button key={t} type="button" onClick={() => setTarget(t)} style={{
                      padding: '6px 14px', borderRadius: 6, fontSize: 12, fontWeight: 600, cursor: 'pointer',
                      border: `1.5px solid ${target === t ? 'var(--primary)' : 'var(--border)'}`,
                      background: target === t ? 'var(--primary-bg, rgba(229,79,53,0.06))' : 'var(--surface)',
                      color: target === t ? 'var(--primary)' : 'var(--fg)',
                    }}>{t === 'include' ? 'Include list' : 'Exclude list'}</button>
                  ))}
                </div>
              </div>

              {/* Action */}
              <div>
                <label className="label">Action</label>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                  {([
                    { value: 'merge' as CsvAction, label: 'Merge', icon: 'plus' },
                    { value: 'replace' as CsvAction, label: 'Replace all', icon: 'refresh' },
                    { value: 'remove' as CsvAction, label: 'Remove from list', icon: 'trash' },
                  ] as { value: CsvAction; label: string; icon: keyof typeof I }[]).map(({ value, label, icon }) => {
                    const Ic = I[icon] as React.FC<{ size?: number }>
                    return (
                      <label key={value} style={{
                        display: 'flex', alignItems: 'flex-start', gap: 10, padding: '10px 12px', borderRadius: 8, cursor: 'pointer',
                        border: `1.5px solid ${action === value ? 'var(--primary)' : 'var(--border)'}`,
                        background: action === value ? 'var(--primary-bg, rgba(229,79,53,0.06))' : 'var(--surface)',
                      }}>
                        <input type="radio" name="csv-action" value={value} checked={action === value} onChange={() => setAction(value)} style={{ marginTop: 2 }} />
                        <div>
                          <div style={{ fontSize: 13, fontWeight: 600, display: 'flex', alignItems: 'center', gap: 6 }}>
                            <Ic size={12} /> {label}
                          </div>
                          <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 2 }}>{actionDescriptions[value]}</div>
                        </div>
                      </label>
                    )
                  })}
                </div>
              </div>
            </>
          )}

          <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
            <button type="button" className="btn" onClick={onClose} disabled={saving}>Cancel</button>
            <button
              type="button" className="btn primary"
              disabled={parsedKeys.length === 0 || saving}
              onClick={() => onApply(parsedKeys, action, target)}
            >
              {saving ? 'Applying…' : <><I.check size={13} /> Apply {parsedKeys.length > 0 ? `(${parsedKeys.length} keys)` : ''}</>}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

// ─── Add entry input ───────────────────────────────────────────────────────────

interface AddEntryInputProps {
  contextType: string
  listType: ListTarget
  saving: boolean
  onAdd: (key: string) => Promise<void>
}

function AddEntryInput({ contextType, listType, saving, onAdd }: AddEntryInputProps) {
  const [newKey, setNewKey] = useState('')

  async function handleAdd() {
    const k = newKey.trim()
    if (!k) return
    await onAdd(k)
    setNewKey('')
  }

  return (
    <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
      <input
        className="input"
        style={{ flex: 1, fontFamily: 'var(--font-mono)', fontSize: 12 }}
        placeholder={`Add ${contextType} key to ${listType} list…`}
        value={newKey}
        onChange={(e) => setNewKey(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void handleAdd() } }}
      />
      <button className="btn primary" disabled={!newKey.trim() || saving} onClick={() => void handleAdd()}>
        <I.plus size={12} /> Add
      </button>
    </div>
  )
}

// ─── Main page ─────────────────────────────────────────────────────────────────

export function SegmentDetail() {
  const { key: segmentId } = useParams<{ key: string }>()
  const navigate = useNavigate()
  const { envId, orgId } = useOrgContext()

  const [segment, setSegment] = useState<Segment | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Count state (replaces full include/exclude arrays)
  const [includedCount, setIncludedCount] = useState(0)
  const [excludedCount, setExcludedCount] = useState(0)
  const [activeTab, setActiveTab] = useState<ListTarget>('include')
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)

  // Lookup
  const [lookupQuery, setLookupQuery] = useState('')
  const [lookupResult, setLookupResult] = useState<{ key: string; inInclude: boolean; inExclude: boolean } | null>(null)
  const [lookupLoading, setLookupLoading] = useState(false)
  const [lookupError, setLookupError] = useState<string | null>(null)

  // CSV
  const [showCsv, setShowCsv] = useState(false)

  // Edit metadata modal
  const [showEdit, setShowEdit] = useState(false)

  // ── Load segment ──────────────────────────────────────────────────────────────
  useEffect(() => {
    if (!segmentId) return
    setLoading(true)
    setError(null)
    api.get<Segment>(`/v1/segments/${segmentId}`)
      .then(({ data }) => {
        setSegment(data)
        setIncludedCount(data.include_count ?? 0)
        setExcludedCount(data.exclude_count ?? 0)
      })
      .catch((err: unknown) => {
        const e = err as { response?: { data?: { message?: string } }; message?: string }
        setError(e?.response?.data?.message ?? e?.message ?? 'Failed to load segment')
      })
      .finally(() => setLoading(false))
  }, [segmentId])

  // ── Patch entries (add/remove/replace) ───────────────────────────────────────
  async function patchEntries(keys: string[], listType: ListTarget, action: string) {
    if (!segment) return
    setSaving(true)
    setSaveError(null)
    try {
      const { data } = await api.post<{ include_count: number; exclude_count: number }>(
        `/v1/segments/${segment.id}/entries`,
        { keys, list_type: listType, action }
      )
      setIncludedCount(data.include_count)
      setExcludedCount(data.exclude_count)
    } catch (err: unknown) {
      const e = err as { response?: { data?: { message?: string } }; message?: string }
      setSaveError(e?.response?.data?.message ?? e?.message ?? 'Failed to save')
    } finally {
      setSaving(false)
    }
  }

  async function addEntry(key: string) {
    await patchEntries([key], activeTab, 'add')
  }

  async function applyCsv(keys: string[], action: CsvAction, target: ListTarget) {
    const apiAction: Record<CsvAction, string> = { merge: 'add', replace: 'replace', remove: 'remove' }
    await patchEntries(keys, target, apiAction[action])
    setShowCsv(false)
  }

  // ── Lookup ────────────────────────────────────────────────────────────────────
  async function runLookup() {
    if (!segment || !lookupQuery.trim()) return
    setLookupLoading(true)
    setLookupError(null)
    setLookupResult(null)
    try {
      const { data } = await api.get<{ in_include: boolean; in_exclude: boolean }>(
        `/v1/segments/${segment.id}/entries/lookup`,
        { params: { key: lookupQuery.trim() } }
      )
      setLookupResult({ key: lookupQuery.trim(), inInclude: data.in_include, inExclude: data.in_exclude })
    } catch (err: unknown) {
      const e = err as { response?: { data?: { message?: string } }; message?: string }
      setLookupError(e?.response?.data?.message ?? e?.message ?? 'Lookup failed')
    } finally {
      setLookupLoading(false)
    }
  }

  // ── Render guards ─────────────────────────────────────────────────────────────
  if (loading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
        <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading segment…</span>
      </div>
    )
  }

  if (error) {
    return (
      <div className="page-body">
        <div style={{ padding: '12px 16px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 8, color: 'var(--danger)', fontSize: 13 }}>
          <I.alert size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />{error}
        </div>
      </div>
    )
  }

  if (!segment) return null

  const isListType = segment.segment_type === 'list' ||
    (segment.segment_type == null && (includedCount > 0 || excludedCount > 0))

  const contextType = segment.context_type || 'user'

  return (
    <>
      <PageHeader
        crumbs={[<a key="1" onClick={() => navigate(`/org/${orgId}/segments`)} style={{ cursor: 'pointer' }}>Segments</a>, segment.name]}
        title={segment.name}
        subtitle={segment.description}
        badge={
          <span style={{
            fontSize: 11, fontWeight: 600, padding: '2px 8px', borderRadius: 4,
            background: isListType ? 'rgba(22,163,74,0.1)' : 'rgba(59,130,246,0.1)',
            color: isListType ? '#16a34a' : '#2563eb',
            border: `1px solid ${isListType ? 'rgba(22,163,74,0.25)' : 'rgba(59,130,246,0.25)'}`,
          }}>
            {isListType ? 'List-based' : 'Rule-based'}
          </span>
        }
        actions={
          <button className="btn" onClick={() => setShowEdit(true)}>
            <I.pencil size={13} /> Edit
          </button>
        }
      />
      <div className="page-body">
        {/* Stat row */}
        <div className="stat-grid" style={{ marginBottom: 18 }}>
          <div className="stat">
            <div className="stat-label">Type</div>
            <div className="stat-value" style={{ fontFamily: 'var(--font-mono)', fontSize: 20 }}>
              {isListType ? 'list' : 'rule'}
            </div>
          </div>
          {isListType && (
            <>
              <div className="stat">
                <div className="stat-label">Context Type</div>
                <div className="stat-value" style={{ fontFamily: 'var(--font-mono)', fontSize: 18 }}>{contextType}</div>
              </div>
              <div className="stat">
                <div className="stat-label">Included Keys</div>
                <div className="stat-value">{includedCount.toLocaleString()}</div>
              </div>
              <div className="stat">
                <div className="stat-label">Excluded Keys</div>
                <div className="stat-value">{excludedCount.toLocaleString()}</div>
              </div>
            </>
          )}
          <div className="stat">
            <div className="stat-label">Version</div>
            <div className="stat-value">v{segment.version}</div>
          </div>
          <div className="stat">
            <div className="stat-label">Created</div>
            <div className="stat-value" style={{ fontSize: 18 }}>{new Date(segment.created_at).toLocaleDateString()}</div>
          </div>
          <div className="stat">
            <div className="stat-label">Updated</div>
            <div className="stat-value" style={{ fontSize: 18 }}>{new Date(segment.updated_at).toLocaleDateString()}</div>
          </div>
        </div>

        <div className="split-2-third">
          {/* ── Main panel ── */}
          {isListType ? (
            <div className="stack">
              {/* Entry Lookup */}
              <div className="card">
                <div className="card-header">
                  <div className="card-title"><I.search size={14} /> Entry Lookup</div>
                </div>
                <div className="card-body">
                  <div style={{ display: 'flex', gap: 8 }}>
                    <input
                      className="input"
                      style={{ flex: 1, fontFamily: 'var(--font-mono)', fontSize: 12 }}
                      placeholder={`Enter a ${contextType} key to check…`}
                      value={lookupQuery}
                      onChange={(e) => { setLookupQuery(e.target.value); setLookupResult(null); setLookupError(null) }}
                      onKeyDown={(e) => { if (e.key === 'Enter') void runLookup() }}
                    />
                    <button className="btn" onClick={() => void runLookup()} disabled={!lookupQuery.trim() || lookupLoading}>
                      {lookupLoading ? '…' : <><I.play size={12} /> Check</>}
                    </button>
                  </div>
                  {lookupError && (
                    <div style={{ marginTop: 10, fontSize: 12, color: 'var(--danger)' }}>
                      <I.alert size={12} style={{ verticalAlign: 'middle', marginRight: 4 }} />{lookupError}
                    </div>
                  )}
                  {lookupResult && (
                    <div style={{ marginTop: 12, display: 'flex', flexDirection: 'column', gap: 6 }}>
                      {!lookupResult.inInclude && !lookupResult.inExclude && (
                        <div style={{ padding: '8px 12px', borderRadius: 6, background: 'var(--bg-sunken)', border: '1px solid var(--border)', fontSize: 12, color: 'var(--fg-muted)' }}>
                          <I.info size={12} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                          <strong>{lookupResult.key}</strong> is not in either list
                        </div>
                      )}
                      {lookupResult.inInclude && (
                        <div style={{ padding: '8px 12px', borderRadius: 6, background: 'var(--success-bg)', border: '1px solid rgba(22,163,74,0.25)', fontSize: 12, color: 'var(--success)', fontWeight: 600 }}>
                          <I.check size={12} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                          <strong>{lookupResult.key}</strong> is in the <strong>include</strong> list → matches segment
                        </div>
                      )}
                      {lookupResult.inExclude && (
                        <div style={{ padding: '8px 12px', borderRadius: 6, background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.25)', fontSize: 12, color: 'var(--danger)', fontWeight: 600 }}>
                          <I.alert size={12} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                          <strong>{lookupResult.key}</strong> is in the <strong>exclude</strong> list → does not match
                        </div>
                      )}
                      {lookupResult.inInclude && lookupResult.inExclude && (
                        <div style={{ padding: '8px 12px', borderRadius: 6, background: 'rgba(234,179,8,0.08)', border: '1px solid rgba(234,179,8,0.3)', fontSize: 12, color: '#92400e' }}>
                          <I.alert size={12} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                          <strong>Conflict:</strong> key is in both lists — exclude takes precedence
                        </div>
                      )}
                    </div>
                  )}
                </div>
              </div>

              {/* Include / Exclude manager */}
              <div className="card">
                <div className="card-header">
                  <div className="card-title"><I.list size={14} /> List entries</div>
                  <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                    {saveError && (
                      <span style={{ fontSize: 11, color: 'var(--danger)' }}>{saveError}</span>
                    )}
                    {saving && <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>Saving…</span>}
                    <button className="btn sm" onClick={() => setShowCsv(true)} disabled={saving}>
                      <I.upload size={12} /> Import CSV
                    </button>
                  </div>
                </div>

                {/* Tab bar */}
                <div style={{ display: 'flex', borderBottom: '1px solid var(--border)', padding: '0 16px' }}>
                  {(['include', 'exclude'] as const).map((tab) => {
                    const count = tab === 'include' ? includedCount : excludedCount
                    const active = activeTab === tab
                    return (
                      <button
                        key={tab}
                        type="button"
                        onClick={() => setActiveTab(tab)}
                        style={{
                          padding: '10px 14px', border: 'none', background: 'none', cursor: 'pointer',
                          fontSize: 13, fontWeight: active ? 600 : 400,
                          color: active ? 'var(--fg)' : 'var(--fg-muted)',
                          borderBottom: active ? '2px solid var(--primary)' : '2px solid transparent',
                          marginBottom: -1,
                        }}
                      >
                        {tab === 'include' ? 'Include' : 'Exclude'}
                        <span style={{
                          marginLeft: 6, fontSize: 11, padding: '1px 6px', borderRadius: 10,
                          background: active ? 'var(--primary-bg, rgba(229,79,53,0.1))' : 'var(--bg-sunken)',
                          color: active ? 'var(--primary)' : 'var(--fg-muted)',
                        }}>{count.toLocaleString()}</span>
                      </button>
                    )
                  })}
                </div>

                <div style={{ padding: 16 }}>
                  {activeTab === 'include' ? (
                    <>
                      <div style={{ fontSize: 12, color: 'var(--fg-muted)', marginBottom: 12 }}>
                        These <code>{contextType}</code> keys will <strong>always match</strong> this segment.
                        This segment has <strong>{includedCount.toLocaleString()}</strong> included key{includedCount !== 1 ? 's' : ''}.
                        Use CSV import to manage large lists.
                      </div>
                      <AddEntryInput
                        contextType={contextType}
                        listType="include"
                        saving={saving}
                        onAdd={addEntry}
                      />
                    </>
                  ) : (
                    <>
                      <div style={{ fontSize: 12, color: 'var(--fg-muted)', marginBottom: 12 }}>
                        These <code>{contextType}</code> keys will <strong>never match</strong> this segment, even if also included.
                        This segment has <strong>{excludedCount.toLocaleString()}</strong> excluded key{excludedCount !== 1 ? 's' : ''}.
                        Use CSV import to manage large lists.
                      </div>
                      <AddEntryInput
                        contextType={contextType}
                        listType="exclude"
                        saving={saving}
                        onAdd={addEntry}
                      />
                    </>
                  )}
                </div>
              </div>
            </div>
          ) : (
            /* Rule-based placeholder */
            <div className="card">
              <div className="card-header">
                <div className="card-title"><I.toggle size={14} /> Rule definition</div>
              </div>
              <div style={{ padding: '32px 20px', textAlign: 'center', color: 'var(--fg-muted)', fontSize: 13 }}>
                <I.info size={18} style={{ display: 'block', margin: '0 auto 8px' }} />
                Rule-based condition builder coming soon.
              </div>
            </div>
          )}

          {/* ── Sidebar ── */}
          <div className="stack">
            <div className="card">
              <div className="card-header"><div className="card-title">Details</div></div>
              <div className="card-body">
                <div style={{ fontSize: 12, lineHeight: 2 }}>
                  {segment.description && (
                    <div style={{ marginBottom: 10, color: 'var(--fg-muted)', lineHeight: 1.5 }}>{segment.description}</div>
                  )}
                  {[
                    ['Segment ID', segment.id],
                    ['Environment', segment.created_at ? envId : '—'],
                    ['Version', `v${segment.version}`],
                    ...(isListType ? [['Context type', contextType]] : []),
                  ].map(([k, v]) => (
                    <div key={k} style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                      <span style={{ color: 'var(--fg-muted)', flexShrink: 0 }}>{k}</span>
                      <span className="mono-key" style={{ fontSize: 11, textAlign: 'right', wordBreak: 'break-all' }}>{v}</span>
                    </div>
                  ))}
                </div>
              </div>
            </div>

            {segment.tags && segment.tags.length > 0 && (
              <div className="card">
                <div className="card-header"><div className="card-title">Tags</div></div>
                <div className="card-body">
                  <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
                    {segment.tags.map((tag) => (
                      <span key={tag} className="badge">{tag}</span>
                    ))}
                  </div>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {showCsv && (
        <CsvImportModal
          contextType={contextType}
          onClose={() => setShowCsv(false)}
          onApply={applyCsv}
          saving={saving}
        />
      )}

      {showEdit && segment && (
        <EditSegmentModal
          segment={segment}
          onClose={() => setShowEdit(false)}
          onSaved={(updated) => {
            setSegment(updated)
            setIncludedCount(updated.include_count ?? 0)
            setExcludedCount(updated.exclude_count ?? 0)
            setShowEdit(false)
          }}
        />
      )}
    </>
  )
}
