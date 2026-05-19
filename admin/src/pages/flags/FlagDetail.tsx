import { useState, useEffect } from 'react'
import { useNavigate, useParams, useBlocker } from 'react-router-dom'
import { useToast } from '../../hooks/useToast'
import { usePermissions } from '../../hooks/usePermissions'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { ToastContainer } from '../../components/ToastContainer'
import { ConfirmDialog } from '../../components/ConfirmDialog'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import { PERMISSIONS } from '../../lib/permissions'
import { extractErrorMessage } from '../../lib/errors'
import type { AdminFlagResponse, VariantJson } from '../../lib/types'
import { PreviewTab } from './PreviewTab'
import { AnalyticsTab } from './AnalyticsTab'
import type { RuleState, ConditionExpr, RuleOutputJson, AllocationBucket } from '../../lib/ruleTypes'
import { localId, allocationSum, isCatchAll, defaultCatchAll, normalizeOutput } from '../../lib/ruleTypes'
import { RuleList } from '../../components/rules/RuleList'

// ─── Helpers ─────────────────────────────────────────────────────────────────

/** Returns an error string if the raw value doesn't match the flag type, null if valid. */
function validateVariantValue(raw: string, flagType: string): string | null {
  if (flagType === 'bool') return null
  if (flagType === 'int') {
    if (!/^-?\d+$/.test(raw.trim())) return 'Must be a whole number (e.g. 42)'
  }
  if (flagType === 'double') {
    const n = Number(raw.trim())
    if (raw.trim() === '' || isNaN(n) || !isFinite(n)) return 'Must be a decimal number (e.g. 3.14)'
  }
  if (flagType === 'json') {
    try { JSON.parse(raw.trim()) } catch { return 'Must be valid JSON (e.g. {"key": "value"})' }
  }
  return null
}

function parseRuleState(raw: { condition: unknown; output: unknown; name?: unknown }): RuleState {
  return {
    _localId: localId(),
    name: typeof raw.name === 'string' && raw.name ? raw.name : undefined,
    condition: raw.condition as ConditionExpr,
    // normalizeOutput migrates legacy bare-array allocation to the new object form
    output: normalizeOutput(raw.output),
  }
}

function formatVariantValue(v: unknown): string {
  if (v === null || v === undefined) return ''
  if (typeof v === 'object') return JSON.stringify(v)
  return String(v)
}

function parseValue(raw: string, flagType: string): unknown {
  if (flagType === 'bool') return raw === 'true' || raw === '1'
  if (flagType === 'int') return parseInt(raw, 10)
  if (flagType === 'double') return parseFloat(raw)
  if (flagType === 'json') { try { return JSON.parse(raw) } catch { return raw } }
  return raw
}


function isConflict(err: unknown): boolean {
  return (err as { response?: { status?: number } })?.response?.status === 409
}

// ─── LockOverlay ──────────────────────────────────────────────────────────────

function LockOverlay() {
  return (
    <div className="page-body">
      <div className="card" style={{ padding: 48, textAlign: 'center' }}>
        <I.lock size={28} stroke="var(--fg-subtle)" style={{ marginBottom: 12 }} />
        <div style={{ fontWeight: 600, fontSize: 15, marginBottom: 6 }}>Access restricted</div>
        <div style={{ color: 'var(--fg-muted)', fontSize: 13 }}>
          You do not have permission to view feature flags.
        </div>
      </div>
    </div>
  )
}

// ─── VariantsPanel ────────────────────────────────────────────────────────────

const VARIANT_PALETTE = ['#F0461F', '#5BAEF5', '#3DD68C', '#A892FF', '#E6B14F']

function VariantsPanel({
  flag, canWrite, onSaved, onConflict,
}: {
  flag: AdminFlagResponse
  canWrite: boolean
  onSaved: (updated: AdminFlagResponse) => void
  onConflict: () => void
}) {
  const { projectId } = useOrgContext()
  const [editing, setEditing] = useState(false)
  const [variants, setVariants] = useState<{ key: string; value: string }[]>([])
  const [variantErrors, setVariantErrors] = useState<(string | null)[]>([])
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Bool flags: only names (keys) are editable — values are always true/false.
  const isBool = flag.flag_type === 'bool'

  function startEdit() {
    const initial = flag.variants.map((v) => ({ key: v.key, value: formatVariantValue(v.value) }))
    setVariants(initial)
    setVariantErrors(initial.map(() => null))
    setEditing(true)
    setError(null)
  }

  function cancel() {
    setEditing(false)
    setError(null)
    setVariantErrors([])
  }

  function addVariant() {
    setVariants([...variants, { key: '', value: '' }])
    setVariantErrors([...variantErrors, null])
  }

  function removeVariant(i: number) {
    if (variants.length <= 1) return
    const removed = variants[i].key
    const usedInRules = flag.rules.some((r) => {
      const o = r.output as { variant_key?: string; allocation?: { buckets?: AllocationBucket[] } }
      if (o.variant_key === removed) return true
      return o.allocation?.buckets?.some((b) => b.variant_key === removed) ?? false
    })
    if (usedInRules) {
      setError(`Variant "${removed}" is referenced in a rule — remove it from rules first.`)
      return
    }
    setVariants(variants.filter((_, j) => j !== i))
    setVariantErrors(variantErrors.filter((_, j) => j !== i))
    setError(null)
  }

  async function save() {
    if (!projectId) return
    const validVariants = variants.filter((v) => v.key.trim())
    if (validVariants.length === 0) { setError('At least one variant is required'); return }

    // Duplicate key check
    const keys = validVariants.map((v) => v.key.trim())
    if (new Set(keys).size !== keys.length) { setError('Variant keys must be unique'); return }

    // Type-specific value validation
    const errs = variants.map((v) => validateVariantValue(v.value, flag.flag_type))
    if (errs.some(Boolean)) {
      setVariantErrors(errs)
      setError('Fix the highlighted variant values before saving')
      return
    }

    setSaving(true)
    setError(null)
    try {
      const body = {
        variants: validVariants.map((v) => ({ key: v.key.trim(), value: parseValue(v.value, flag.flag_type) })),
        version: flag.version,
      }
      const { data } = await api.put<AdminFlagResponse>(`/v1/projects/${projectId}/flags/${flag.key}/variants`, body)
      setEditing(false)
      onSaved(data)
    } catch (err: unknown) {
      if (isConflict(err)) { onConflict(); return }
      setError(extractErrorMessage(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="card">
      <div className="card-header">
        <div className="card-title"><I.layers size={14} /> Variants</div>
        {!editing
          ? canWrite && <button className="btn sm" onClick={startEdit}><I.pencil size={12} /> Edit</button>
          : <div style={{ display: 'flex', gap: 6 }}>
              <button className="btn sm" onClick={cancel}>Cancel</button>
              <button className="btn primary sm" disabled={saving} onClick={save}>{saving ? 'Saving…' : 'Save'}</button>
            </div>
        }
      </div>
      <div style={{ padding: 14 }}>
        {isBool && (
          <div style={{ padding: '6px 10px', background: 'var(--bg-sunken)', border: '1px solid var(--border)', borderRadius: 6, fontSize: 11, color: 'var(--fg-muted)', marginBottom: 10 }}>
            Boolean flag — values are fixed (<code style={{ fontFamily: 'var(--font-mono)' }}>true</code> / <code style={{ fontFamily: 'var(--font-mono)' }}>false</code>). Only names are editable.
          </div>
        )}
        {error && (
          <div style={{ padding: '8px 12px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 6, color: 'var(--danger)', fontSize: 12, marginBottom: 10 }}>
            {error}
          </div>
        )}

        {!editing ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {flag.variants.map((v, i) => (
              <div key={v.key} style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '8px 10px', border: '1px solid var(--border)', borderRadius: 6, background: 'var(--surface-2)' }}>
                <div style={{ width: 10, height: 10, borderRadius: 3, background: VARIANT_PALETTE[i % VARIANT_PALETTE.length] }} />
                <span style={{ fontFamily: 'var(--font-mono)', fontWeight: 600, flex: 1 }}>{v.key}</span>
                {v.value !== undefined && <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--fg-muted)' }}>= {formatVariantValue(v.value)}</span>}
                {flag.default_variant_key === v.key && <span className="badge">default</span>}
              </div>
            ))}
          </div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {variants.map((v, i) => (
              <div key={i} style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                  <div style={{ width: 10, height: 10, borderRadius: 3, background: VARIANT_PALETTE[i % VARIANT_PALETTE.length], flexShrink: 0 }} />
                  <input className="input" style={{ width: 110, fontFamily: 'var(--font-mono)', fontSize: 12 }}
                    placeholder="name" value={v.key}
                    onChange={(e) => setVariants(variants.map((x, j) => j === i ? { ...x, key: e.target.value } : x))} />
                  {isBool ? (
                    // Bool flags: value is fixed, shown as read-only chip
                    <span style={{ flex: 1, fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--fg-muted)', padding: '0 8px' }}>
                      = {v.value}
                    </span>
                  ) : (
                    <input
                      className="input"
                      style={{
                        flex: 1,
                        fontFamily: 'var(--font-mono)',
                        fontSize: 12,
                        borderColor: variantErrors[i] ? 'var(--danger)' : undefined,
                      }}
                      placeholder={
                        flag.flag_type === 'json' ? '{}' :
                        flag.flag_type === 'int' ? '42' :
                        flag.flag_type === 'double' ? '3.14' : 'value'
                      }
                      value={v.value}
                      onChange={(e) => {
                        const next = variants.map((x, j) => j === i ? { ...x, value: e.target.value } : x)
                        setVariants(next)
                        setVariantErrors(next.map((x) => validateVariantValue(x.value, flag.flag_type)))
                      }}
                    />
                  )}
                  {!isBool && (
                    <button className="icon-btn" style={{ color: variants.length <= 1 ? 'var(--fg-faint)' : 'var(--danger)' }}
                      disabled={variants.length <= 1} onClick={() => removeVariant(i)}>
                      <I.x size={12} />
                    </button>
                  )}
                </div>
                {variantErrors[i] && (
                  <div style={{ fontSize: 11, color: 'var(--danger)', paddingLeft: 130 }}>
                    {variantErrors[i]}
                  </div>
                )}
              </div>
            ))}
            {!isBool && (
              <button className="btn sm" style={{ alignSelf: 'flex-start' }} onClick={addVariant}><I.plus size={11} /> Add variant</button>
            )}
          </div>
        )}

      </div>
    </div>
  )
}

// ─── TargetingPanel (rule builder) ───────────────────────────────────────────

function validateRules(rules: RuleState[], catchAllOutput?: RuleOutputJson): string | null {
  for (let i = 0; i < rules.length; i++) {
    const o = rules[i].output
    if ('allocation' in o) {
      const sum = allocationSum((o as { allocation: { buckets: AllocationBucket[] } }).allocation.buckets)
      if (sum !== 1000) return `Rule ${i + 1}: allocation must sum to 100% (currently ${(sum / 10).toFixed(1)}%)`
    }
  }
  if (catchAllOutput && 'allocation' in catchAllOutput) {
    const sum = allocationSum((catchAllOutput as { allocation: { buckets: AllocationBucket[] } }).allocation.buckets)
    if (sum !== 1000) return `Default catch-all: allocation must sum to 100% (currently ${(sum / 10).toFixed(1)}%)`
  }
  return null
}

function TargetingPanel({
  flag, canWrite, onSaved, onConflict, onDirtyChange,
}: {
  flag: AdminFlagResponse
  canWrite: boolean
  onSaved: (updated: AdminFlagResponse) => void
  onConflict: () => void
  onDirtyChange: (dirty: boolean) => void
}) {
  const { projectId, envId, orgId } = useOrgContext()

  const variantKeys = flag.variants.map((v: VariantJson) => v.key)

  // Split stored rules into user-authored rules and the catch-all (last rule
  // with an empty AND condition = always-true sentinel).
  const [rules, setRules] = useState<RuleState[]>(() => {
    const all = flag.rules.map(parseRuleState)
    const last = all[all.length - 1]
    return last && isCatchAll(last) ? all.slice(0, -1) : all
  })
  const [catchAllOutput, setCatchAllOutput] = useState<RuleOutputJson>(() => {
    const all = flag.rules.map(parseRuleState)
    const last = all[all.length - 1]
    return last && isCatchAll(last)
      ? last.output
      : { variant_key: variantKeys[0] ?? '' }
  })
  const [catchAllName, setCatchAllName] = useState<string>(() => {
    const all = flag.rules.map(parseRuleState)
    const last = all[all.length - 1]
    return last && isCatchAll(last) ? (last.name ?? '') : ''
  })

  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [dirty, setDirty] = useState(false)

  function markDirty() {
    if (!dirty) { setDirty(true); onDirtyChange(true) }
  }

  function updateRules(next: RuleState[]) {
    setRules(next)
    markDirty()
  }

  function updateCatchAll(output: RuleOutputJson) {
    setCatchAllOutput(output)
    markDirty()
  }

  function updateCatchAllName(name: string) {
    setCatchAllName(name)
    markDirty()
  }

  async function saveRules() {
    const err = validateRules(rules, catchAllOutput)
    if (err) { setError(err); return }
    if (!projectId) return
    setSaving(true)
    setError(null)
    try {
      // Append the catch-all as the last rule (always-true sentinel AND: []).
      const catchAll = defaultCatchAll('')
      catchAll.output = catchAllOutput
      const trimmedCatchAllName = catchAllName.trim()
      if (trimmedCatchAllName) {
        catchAll.name = trimmedCatchAllName
      }
      const allRules = [...rules, catchAll]
      const body = {
        rules: allRules.map((r) => ({ ...(r.name ? { name: r.name } : {}), condition: r.condition, output: r.output })),
        version: flag.version,
      }
      const { data } = await api.put<AdminFlagResponse>(`/v1/projects/${projectId}/flags/${flag.key}/rules`, body)
      setDirty(false)
      onDirtyChange(false)
      onSaved(data)
    } catch (err: unknown) {
      if (isConflict(err)) { onConflict(); return }
      setError(extractErrorMessage(err))
    } finally {
      setSaving(false)
    }
  }

  async function saveDefaultVariant(key: string) {
    if (!projectId) return
    try {
      const { data } = await api.put<AdminFlagResponse>(`/v1/projects/${projectId}/flags/${flag.key}`, {
        default_variant_key: key,
        version: flag.version,
      })
      onSaved(data)
    } catch (err: unknown) {
      if (isConflict(err)) { onConflict() }
      throw err
    }
  }

  return (
    <div className="card">
      <div className="card-header">
        <div className="card-title"><I.toggle size={14} /> Targeting rules</div>
        {dirty && canWrite && (
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 11, color: 'var(--warning)', fontWeight: 600 }}>Unsaved changes</span>
            <button className="btn primary sm" disabled={saving} onClick={saveRules}>
              {saving ? 'Saving…' : 'Save rules'}
            </button>
          </div>
        )}
      </div>
      <div style={{ padding: 14 }}>
        {error && (
          <div style={{ padding: '8px 12px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 6, color: 'var(--danger)', fontSize: 12, marginBottom: 12 }}>
            {error}
          </div>
        )}
        {!canWrite && (
          <div style={{ padding: '8px 12px', background: 'var(--bg-sunken)', border: '1px solid var(--border)', borderRadius: 6, fontSize: 12, color: 'var(--fg-muted)', marginBottom: 12, display: 'flex', alignItems: 'center', gap: 8 }}>
            <I.lock size={12} /> View-only — you do not have permission to edit rules.
          </div>
        )}
        <RuleList
          rules={rules}
          variants={variantKeys}
          defaultVariantKey={flag.default_variant_key}
          catchAllOutput={catchAllOutput}
          catchAllName={catchAllName}
          flagEnabled={flag.enabled}
          onChange={canWrite ? updateRules : () => undefined}
          onCatchAllChange={canWrite ? updateCatchAll : () => undefined}
          onCatchAllNameChange={canWrite ? updateCatchAllName : undefined}
          canWrite={canWrite}
          onSaveDefaultVariant={canWrite ? saveDefaultVariant : undefined}
          envId={envId}
          orgId={orgId}
        />
      </div>
    </div>
  )
}

// ─── SdkSnippet ───────────────────────────────────────────────────────────────

function SdkSnippet({ flag }: { flag: AdminFlagResponse }) {
  const typeMap: Record<string, string> = { bool: 'bool', string: 'String', double: 'f64', int: 'i64', json: 'serde_json::Value' }
  return (
    <div className="split-2">
      <div className="card">
        <div className="card-header"><div className="card-title">Rust SDK</div><button className="btn sm"><I.copy size={12} /> Copy</button></div>
        <div className="card-body">
          <pre className="code">{`use stitchd_sdk::{SdkClient, Context};

let client = SdkClient::init(config).await?;

let ctx = Context::user("u_8421")
    .param("country", "US")
    .param("plan", "pro");

let value = client.evaluate::<${typeMap[flag.flag_type] ?? flag.flag_type}>(
    "${flag.key}", &ctx
).await?;`}</pre>
        </div>
      </div>
      <div className="card">
        <div className="card-header"><div className="card-title">Test evaluation</div><button className="btn primary sm"><I.play size={12} /> Run</button></div>
        <div className="card-body">
          <label className="label">Context</label>
          <pre className="code" style={{ marginBottom: 12 }}>{`{
  "_type": "user",
  "key": "u_8421",
  "parameters": {
    "country": "US",
    "plan": "pro"
  }
}`}</pre>
          <div style={{ padding: 12, background: 'var(--success-bg)', border: '1px solid rgba(17,122,61,0.25)', borderRadius: 6 }}>
            <div style={{ fontSize: 11, color: 'var(--success)', textTransform: 'uppercase', letterSpacing: '0.05em', fontWeight: 600 }}>Result</div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 6 }}>
              <span style={{ fontFamily: 'var(--font-mono)', fontSize: 18, fontWeight: 700 }}>{flag.variants[0]?.key ?? '—'}</span>
              <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>matched rule #1 · evaluated in 0.4ms</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

// ─── FlagHistory ──────────────────────────────────────────────────────────────

function FlagHistory() {
  return (
    <div className="card">
      <div className="empty" style={{ padding: '32px 24px' }}>
        <div className="empty-icon"><I.history size={20} /></div>
        <div className="empty-title">History coming soon</div>
        <div className="empty-desc">Audit log will show all flag mutations here.</div>
      </div>
    </div>
  )
}

// ─── MetadataEditor (inline edit) ────────────────────────────────────────────

function MetadataEditor({
  flag, canWrite, onSaved, onConflict,
}: {
  flag: AdminFlagResponse
  canWrite: boolean
  onSaved: (updated: AdminFlagResponse) => void
  onConflict: () => void
}) {
  const { projectId } = useOrgContext()
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(flag.name)
  const [description, setDescription] = useState(flag.description)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function startEdit() {
    setName(flag.name)
    setDescription(flag.description)
    setEditing(true)
    setError(null)
  }

  function cancel() {
    setEditing(false)
    setError(null)
  }

  async function save() {
    if (!projectId) return
    setSaving(true)
    setError(null)
    try {
      const { data } = await api.put<AdminFlagResponse>(`/v1/projects/${projectId}/flags/${flag.key}`, {
        name: name.trim(),
        description: description.trim(),
        version: flag.version,
      })
      setEditing(false)
      onSaved(data)
    } catch (err: unknown) {
      if (isConflict(err)) { onConflict(); return }
      setError(extractErrorMessage(err))
    } finally {
      setSaving(false)
    }
  }

  if (!editing) {
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <span style={{ fontSize: 13, color: 'var(--fg-muted)' }}>
          {flag.name}{flag.description ? ` — ${flag.description}` : ''}
        </span>
        {canWrite && <button className="icon-btn" onClick={startEdit}><I.pencil size={12} /></button>}
      </div>
    )
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6, minWidth: 300 }}>
      {error && <div style={{ fontSize: 12, color: 'var(--danger)' }}>{error}</div>}
      <input className="input" style={{ fontSize: 13 }} placeholder="Name" value={name}
        onChange={(e) => setName(e.target.value)} autoFocus />
      <input className="input" style={{ fontSize: 12 }} placeholder="Description (optional)" value={description}
        onChange={(e) => setDescription(e.target.value)} />
      <div style={{ display: 'flex', gap: 6 }}>
        <button className="btn sm" onClick={cancel}>Cancel</button>
        <button className="btn primary sm" disabled={saving} onClick={save}>{saving ? '…' : 'Save'}</button>
      </div>
    </div>
  )
}

// ─── FlagDetail ───────────────────────────────────────────────────────────────

type Tab = 'targeting' | 'variants' | 'evals' | 'preview' | 'code' | 'history'

export function FlagDetail() {
  const { key } = useParams<{ key: string }>()
  const navigate = useNavigate()
  const { projectId, orgId } = useOrgContext()
  const { can, loading: permLoading } = usePermissions()
  const { toasts, toast, dismiss } = useToast()

  const [flag, setFlag] = useState<AdminFlagResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [tab, setTab] = useState<Tab>('targeting')
  const [enabled, setEnabled] = useState(false)
  const [togglingEnabled, setTogglingEnabled] = useState(false)
  const [showArchiveConfirm, setShowArchiveConfirm] = useState(false)
  const [rulesDirty, setRulesDirty] = useState(false)

  const canRead = can(PERMISSIONS.FLAG_READ)
  const canWrite = can(PERMISSIONS.FLAG_WRITE)

  // Unsaved-changes blocker
  const blocker = useBlocker(rulesDirty)

  useEffect(() => {
    if (!projectId || !key) return
    setLoading(true)
    setLoadError(null)
    api.get<AdminFlagResponse>(`/v1/projects/${projectId}/flags/${key}`)
      .then(({ data }) => {
        setFlag(data)
        setEnabled(data.enabled)
      })
      .catch((err) => setLoadError(extractErrorMessage(err)))
      .finally(() => setLoading(false))
  }, [projectId, key])

  function handleConflict() {
    toast('Flag was modified by someone else — refresh to reload', 'error', {
      label: 'Refresh',
      onClick: () => {
        setFlag(null)
        setLoading(true)
        if (!projectId || !key) return
        api.get<AdminFlagResponse>(`/v1/projects/${projectId}/flags/${key}`)
          .then(({ data }) => { setFlag(data); setEnabled(data.enabled) })
          .catch(() => setLoadError('Failed to reload'))
          .finally(() => setLoading(false))
      },
    })
  }

  async function toggleEnabled() {
    if (!flag || !projectId || togglingEnabled || !canWrite) return
    const next = !enabled
    setEnabled(next)
    setTogglingEnabled(true)
    try {
      const { data } = await api.put<AdminFlagResponse>(`/v1/projects/${projectId}/flags/${key}`, {
        enabled: next,
        version: flag.version,
      })
      setFlag(data)
      setEnabled(data.enabled)
    } catch (err: unknown) {
      setEnabled(!next)
      if (isConflict(err)) { handleConflict(); return }
      toast(extractErrorMessage(err), 'error')
    } finally {
      setTogglingEnabled(false)
    }
  }

  async function archiveFlag() {
    if (!flag || !projectId) return
    setShowArchiveConfirm(false)
    try {
      await api.post(`/v1/projects/${projectId}/flags/${flag.key}/archive`, {})
      toast('Flag archived', 'success')
      setTimeout(() => navigate(`/org/${orgId}/flags`), 1200)
    } catch (err: unknown) {
      toast(extractErrorMessage(err), 'error')
    }
  }

  if (permLoading || loading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
        <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading…</span>
      </div>
    )
  }

  if (!permLoading && !canRead) return <LockOverlay />

  if (loadError || !flag) {
    return (
      <div className="page-body">
        <div style={{ padding: '12px 16px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 8, color: 'var(--danger)', fontSize: 13 }}>
          <I.alert size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
          {loadError ?? 'Flag not found'}
        </div>
      </div>
    )
  }

  return (
    <>
      <PageHeader
        crumbs={[<a key="1" onClick={() => navigate(`/org/${orgId}/flags`)} style={{ cursor: 'pointer' }}>Flags</a>, flag.key]}
        title={flag.key}
        mono
        subtitle={
          <MetadataEditor
            flag={flag}
            canWrite={canWrite}
            onSaved={(updated) => { setFlag(updated); toast('Saved', 'success') }}
            onConflict={handleConflict}
          />
        }
        badge={
          <span className={`type-pill ${flag.flag_type}`} style={{ fontSize: 11, padding: '2px 7px' }}>{flag.flag_type}</span>
        }
        actions={
          <>
            <div style={{
              display: 'flex', alignItems: 'center', gap: 8, padding: '6px 12px',
              border: '1px solid var(--border-strong)', borderRadius: 6, background: 'var(--surface)',
              opacity: togglingEnabled || !canWrite ? 0.6 : 1,
            }}>
              <span style={{ fontSize: 11, color: 'var(--fg-muted)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>{enabled ? 'Enabled' : 'Disabled'}</span>
              <button className={`toggle lg ${enabled ? 'on' : ''}`} onClick={toggleEnabled} disabled={!canWrite} />
            </div>
            <button className="btn" onClick={() => { navigator.clipboard.writeText(flag.key) }}><I.copy size={13} /> Copy key</button>
            {canWrite && (
              <button className="btn" style={{ color: 'var(--danger)' }} onClick={() => setShowArchiveConfirm(true)}>
                <I.archive size={13} /> Archive
              </button>
            )}
          </>
        }
      />
      <div className="page-body">
        <div className="tabs">
          <button className={`tab ${tab === 'targeting' ? 'active' : ''}`} onClick={() => setTab('targeting')}>
            <I.toggle size={13} /> Targeting <span className="count">{flag.rules.length}</span>
          </button>
          <button className={`tab ${tab === 'variants' ? 'active' : ''}`} onClick={() => setTab('variants')}>
            <I.layers size={13} /> Variants <span className="count">{flag.variants.length}</span>
          </button>
          <button className={`tab ${tab === 'evals' ? 'active' : ''}`} onClick={() => setTab('evals')}>
            <I.zap size={13} /> Evaluations
          </button>
          <button className={`tab ${tab === 'preview' ? 'active' : ''}`} onClick={() => setTab('preview')}>
            <I.toggle size={13} /> Preview
          </button>
          <button className={`tab ${tab === 'code' ? 'active' : ''}`} onClick={() => setTab('code')}>
            <I.command size={13} /> SDK snippet
          </button>
          <button className={`tab ${tab === 'history' ? 'active' : ''}`} onClick={() => setTab('history')}>
            <I.history size={13} /> History
          </button>
        </div>

        {tab === 'targeting' && (
          <TargetingPanel
            flag={flag}
            canWrite={canWrite}
            onSaved={(updated) => { setFlag(updated); setRulesDirty(false); toast('Rules saved', 'success') }}
            onConflict={handleConflict}
            onDirtyChange={setRulesDirty}
          />
        )}
        {tab === 'variants' && (
          <VariantsPanel
            flag={flag}
            canWrite={canWrite}
            onSaved={(updated) => { setFlag(updated); toast('Variants saved', 'success') }}
            onConflict={handleConflict}
          />
        )}
        {tab === 'evals' && projectId && <AnalyticsTab projectId={projectId} flagId={flag.flag_id} />}
        {tab === 'preview' && <PreviewTab flagId={flag.key} />}
        {tab === 'code' && <SdkSnippet flag={flag} />}
        {tab === 'history' && <FlagHistory />}
      </div>

      <ToastContainer toasts={toasts} onDismiss={dismiss} />

      {showArchiveConfirm && (
        <ConfirmDialog
          title="Archive flag?"
          message={`Flag "${flag.key}" will be archived and excluded from evaluations. You can restore it with ?include_archived=true.`}
          confirmLabel="Archive"
          confirmDanger
          onConfirm={archiveFlag}
          onCancel={() => setShowArchiveConfirm(false)}
        />
      )}

      {blocker.state === 'blocked' && (
        <ConfirmDialog
          title="Unsaved changes"
          message="You have unsaved rule changes. Leave anyway?"
          confirmLabel="Leave"
          confirmDanger
          onConfirm={() => blocker.proceed()}
          onCancel={() => blocker.reset()}
        />
      )}
    </>
  )
}
