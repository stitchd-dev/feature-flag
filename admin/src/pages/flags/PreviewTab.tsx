import { useState } from 'react'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import { SuggestionInput } from '../../components/SuggestionInput'
import { useContextTypeSuggestions, useContextParamSuggestions } from '../../hooks/useContextSuggestions'

// ─── Types ────────────────────────────────────────────────────────────────────

export interface ContextParam {
  key: string
  value: string
}

export interface ContextCard {
  _type: string
  key: string
  params: ContextParam[]
}

interface ConditionTrace {
  predicate: string
  result: boolean
}

interface RuleTrace {
  rule_name: string | null
  outcome: 'match' | 'no_match' | 'skipped'
  conditions: ConditionTrace[]
}

interface VariantRange {
  variant_key: string
  from: number
  to: number
}

interface RolloutDebug {
  hash_input: string
  bucket: number
  variant_ranges: VariantRange[]
}

interface ContextResult {
  context_index: number
  context_key: string
  variant_key: string
  variant_value: unknown
  disabled: boolean
  fired_rule_index: number | null
  fired_rule_name: string | null
  rule_traces: RuleTrace[]
  rollout_debug: RolloutDebug | null
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

function parseContextsJson(json: string): { contexts: ContextCard[]; error: string | null } {
  let parsed: unknown
  try {
    parsed = JSON.parse(json)
  } catch {
    return { contexts: [], error: 'Invalid JSON: ' + (json.length > 40 ? json.slice(0, 40) + '…' : json) }
  }
  if (!Array.isArray(parsed)) {
    return { contexts: [], error: 'Expected a JSON array of context objects' }
  }
  const contexts: ContextCard[] = parsed.map((item: unknown) => {
    if (typeof item !== 'object' || item === null) {
      return { _type: '', key: '', params: [] }
    }
    const obj = item as Record<string, unknown>
    const _type = typeof obj['_type'] === 'string' ? obj['_type'] : ''
    const key = typeof obj['key'] === 'string' ? obj['key'] : ''
    const parameters = obj['parameters']
    const params: ContextParam[] =
      parameters && typeof parameters === 'object' && !Array.isArray(parameters)
        ? Object.entries(parameters as Record<string, unknown>).map(([k, v]) => ({
            key: k,
            value: String(v),
          }))
        : []
    return { _type, key, params }
  })
  return { contexts, error: null }
}

function contextsToJson(contexts: ContextCard[]): string {
  const arr = contexts.map((c) => {
    const parameters: Record<string, string> = {}
    for (const p of c.params) {
      if (p.key !== '') parameters[p.key] = p.value
    }
    return { _type: c._type, key: c.key, parameters }
  })
  return JSON.stringify(arr, null, 2)
}

const DEFAULT_JSON = JSON.stringify([{ _type: '', key: '', parameters: {} }], null, 2)

// ─── Sub-components ───────────────────────────────────────────────────────────

function RuleTraceRow({ trace, isCatchAll }: { trace: RuleTrace; isCatchAll: boolean }) {
  const [open, setOpen] = useState(false)
  // Fallback label: the last rule is always the always-true catch-all (see
  // `isCatchAll` in `lib/ruleTypes.ts`), so render it as "Default rule"
  // rather than the generic "Unnamed rule" used for user-authored rules.
  const fallbackName = isCatchAll ? 'Default rule' : 'Unnamed rule'
  return (
    <div style={{ borderLeft: '2px solid var(--border-faint)', paddingLeft: 10, marginBottom: 6 }}>
      <button
        onClick={() => setOpen((o) => !o)}
        style={{
          background: 'none', border: 'none', cursor: 'pointer', padding: 0,
          display: 'flex', alignItems: 'center', gap: 6, fontSize: 12,
          color: trace.outcome === 'match' ? 'var(--success, #22c55e)' : 'var(--fg-muted)',
        }}
      >
        <span>{trace.outcome === 'match' ? '✓' : '✗'}</span>
        <span>{trace.rule_name ?? fallbackName}</span>
        <span style={{ opacity: 0.5 }}>{open ? '▾' : '▸'}</span>
      </button>
      {open && (
        <div style={{ marginTop: 4, display: 'flex', flexDirection: 'column', gap: 2 }}>
          {trace.conditions.map((c, i) => (
            <div key={i} style={{ fontSize: 11, display: 'flex', gap: 6, color: 'var(--fg-muted)' }}>
              <span style={{ color: c.result ? 'var(--success, #22c55e)' : 'var(--danger)' }}>
                {c.result ? '✓' : '✗'}
              </span>
              <span>{c.predicate}</span>
            </div>
          ))}
          {trace.conditions.length === 0 && (
            <div style={{ fontSize: 11, color: 'var(--fg-muted)', fontStyle: 'italic' }}>No conditions</div>
          )}
        </div>
      )}
    </div>
  )
}

function RolloutDebugPanel({ debug }: { debug: RolloutDebug }) {
  return (
    <div style={{
      marginTop: 8, padding: '8px 10px', background: 'var(--surface-raised, var(--surface))',
      borderRadius: 4, border: '1px solid var(--border-faint)', fontSize: 11,
    }}>
      <div style={{ fontWeight: 600, marginBottom: 4, color: 'var(--fg-muted)' }}>Rollout debug</div>
      <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap' }}>
        <span><span style={{ opacity: 0.6 }}>hash input:</span> <code>{debug.hash_input}</code></span>
        <span><span style={{ opacity: 0.6 }}>bucket:</span> <code>{debug.bucket}</code></span>
      </div>
      <div style={{ marginTop: 4, display: 'flex', flexDirection: 'column', gap: 2 }}>
        {debug.variant_ranges.map((r, i) => (
          <div key={i} style={{ display: 'flex', gap: 8 }}>
            <span style={{ opacity: 0.6 }}>{r.from}–{r.to}</span>
            <span>{r.variant_key}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

function ContextResultCard({ result }: { result: ContextResult }) {
  return (
    <div style={{
      border: '1px solid var(--border)',
      borderRadius: 6,
      overflow: 'hidden',
    }}>
      <div style={{
        padding: '10px 14px',
        background: 'var(--surface)',
        borderBottom: '1px solid var(--border-faint)',
        display: 'flex', alignItems: 'center', gap: 10,
      }}>
        <span style={{ fontSize: 12, color: 'var(--fg-muted)', fontFamily: 'monospace' }}>
          {result.context_key || '(no key)'}
        </span>
        {result.disabled ? (
          <span style={{
            fontSize: 11, padding: '1px 7px', borderRadius: 10,
            background: 'var(--fg-muted)', color: 'var(--bg)', opacity: 0.7,
          }}>disabled</span>
        ) : (
          <span style={{
            fontSize: 11, padding: '1px 7px', borderRadius: 10,
            background: 'var(--accent)', color: '#fff',
          }}>{result.variant_key ?? 'default'}</span>
        )}
        {result.fired_rule_name && (
          <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>via {result.fired_rule_name}</span>
        )}
      </div>
      <div style={{ padding: '10px 14px' }}>
        {result.disabled && (
          <div style={{
            marginBottom: 8, padding: '6px 10px', borderRadius: 4,
            background: 'var(--warning-bg, #fef3c7)', color: 'var(--warning-fg, #92400e)',
            fontSize: 12,
          }}>
            Flag is disabled — default rule applied
          </div>
        )}
        {result.rule_traces.map((trace, i) => (
          <RuleTraceRow
            key={i}
            trace={trace}
            isCatchAll={i === result.rule_traces.length - 1}
          />
        ))}
        {result.rollout_debug && <RolloutDebugPanel debug={result.rollout_debug} />}
      </div>
    </div>
  )
}

// ─── FormBuilder mode ─────────────────────────────────────────────────────────

function ContextCardEditor({
  card,
  envId,
  onUpdate,
  onRemove,
}: {
  card: ContextCard
  envId: string | null
  onUpdate: (patch: Partial<ContextCard>) => void
  onRemove: () => void
}) {
  const { data: typeData } = useContextTypeSuggestions(envId)
  const { data: paramData } = useContextParamSuggestions(envId, card._type || null)

  const typeSuggestions = typeData.map((t) => ({ label: t.context_type }))
  const paramSuggestions = paramData.map((p) => ({ label: p.param_key, isPrivate: p.is_private }))

  function updateParam(pi: number, patch: Partial<ContextParam>) {
    onUpdate({ params: card.params.map((p, i) => (i === pi ? { ...p, ...patch } : p)) })
  }

  function addParam() {
    onUpdate({ params: [...card.params, { key: '', value: '' }] })
  }

  function removeParam(pi: number) {
    onUpdate({ params: card.params.filter((_, i) => i !== pi) })
  }

  return (
    <div style={{ border: '1px solid var(--border)', borderRadius: 6, padding: '10px 12px' }}>
      <div style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
        <SuggestionInput
          className="input"
          style={{ flex: 1, fontSize: 12 }}
          placeholder="_type (e.g. user, org)"
          value={card._type}
          suggestions={typeSuggestions}
          onChange={(v) => onUpdate({ _type: v })}
        />
        <input
          className="input"
          style={{ flex: 1, fontSize: 12 }}
          placeholder="key (e.g. alice)"
          value={card.key}
          onChange={(e) => onUpdate({ key: e.target.value })}
        />
        <button
          className="icon-btn"
          onClick={onRemove}
          title="Remove context"
          style={{ color: 'var(--danger)', flexShrink: 0 }}
        >×</button>
      </div>
      {card.params.map((param, pi) => (
        <div key={pi} style={{ display: 'flex', gap: 6, marginBottom: 4 }}>
          <SuggestionInput
            className="input"
            style={{ flex: 1, fontSize: 11 }}
            placeholder="param key"
            value={param.key}
            suggestions={paramSuggestions}
            onChange={(v) => updateParam(pi, { key: v })}
          />
          <input
            className="input"
            style={{ flex: 1, fontSize: 11 }}
            placeholder="param value"
            value={param.value}
            onChange={(e) => updateParam(pi, { value: e.target.value })}
          />
          <button
            className="icon-btn"
            onClick={() => removeParam(pi)}
            style={{ color: 'var(--fg-muted)', flexShrink: 0 }}
          >×</button>
        </div>
      ))}
      <button
        className="btn sm"
        onClick={addParam}
        style={{ fontSize: 11, marginTop: 4 }}
      >+ param</button>
    </div>
  )
}

function FormBuilder({
  contexts,
  envId,
  onChange,
}: {
  contexts: ContextCard[]
  envId: string | null
  onChange: (updated: ContextCard[]) => void
}) {
  function addCard() {
    onChange([...contexts, { _type: '', key: '', params: [] }])
  }

  function updateCard(idx: number, patch: Partial<ContextCard>) {
    onChange(contexts.map((c, i) => (i === idx ? { ...c, ...patch } : c)))
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      {contexts.map((card, ci) => (
        <ContextCardEditor
          key={ci}
          card={card}
          envId={envId}
          onUpdate={(patch) => updateCard(ci, patch)}
          onRemove={() => onChange(contexts.filter((_, i) => i !== ci))}
        />
      ))}
      <button className="btn sm" onClick={addCard}>+ Add context</button>
    </div>
  )
}

// ─── PreviewTab ───────────────────────────────────────────────────────────────

type InputMode = 'json' | 'form'

interface PreviewTabProps {
  flagId: string
}

export function PreviewTab({ flagId }: PreviewTabProps) {
  const { projectId, envId } = useOrgContext()

  const [mode, setMode] = useState<InputMode>('json')
  const [jsonText, setJsonText] = useState(DEFAULT_JSON)
  const [jsonError, setJsonError] = useState<string | null>(null)
  const [formContexts, setFormContexts] = useState<ContextCard[]>([{ _type: '', key: '', params: [] }])

  const [loading, setLoading] = useState(false)
  const [results, setResults] = useState<ContextResult[] | null>(null)
  const [apiError, setApiError] = useState<string | null>(null)
  const [stale, setStale] = useState(false)

  function handleJsonChange(value: string) {
    setJsonText(value)
    const { error } = parseContextsJson(value)
    setJsonError(error)
    setStale(true)
    setResults(null)
  }

  function handleFormChange(updated: ContextCard[]) {
    setFormContexts(updated)
    setJsonText(contextsToJson(updated))
    setJsonError(null)
    setStale(true)
    setResults(null)
  }

  function switchToJson() {
    setJsonText(contextsToJson(formContexts))
    setJsonError(null)
    setMode('json')
  }

  function switchToForm() {
    const { contexts, error } = parseContextsJson(jsonText)
    if (error) {
      setJsonError(error)
      return
    }
    setFormContexts(contexts)
    setMode('form')
  }

  async function evaluate() {
    const { error } = parseContextsJson(jsonText)
    if (error) {
      setJsonError(error)
      return
    }
    setLoading(true)
    setApiError(null)
    setStale(false)
    try {
      const { data } = await api.post<{ results: ContextResult[] }>(
        `/v1/projects/${projectId}/flags/${flagId}/evaluate-preview`,
        { contexts: JSON.parse(jsonText), environment_id: envId ?? '' },
      )
      setResults(data.results)
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Request failed'
      setApiError(msg)
    } finally {
      setLoading(false)
    }
  }

  const canEvaluate = !jsonError && !loading

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
      {/* ── Input panel ── */}
      <div className="card">
        <div className="card-header">
          <div className="card-title">Evaluation contexts</div>
          <div style={{ display: 'flex', gap: 6 }}>
            <button
              className={`btn sm ${mode === 'json' ? 'primary' : ''}`}
              onClick={switchToJson}
            >JSON</button>
            <button
              className={`btn sm ${mode === 'form' ? 'primary' : ''}`}
              onClick={switchToForm}
            >Form</button>
          </div>
        </div>

        <div style={{ padding: '12px 16px' }}>
          {mode === 'json' ? (
            <div>
              <textarea
                className="input"
                style={{
                  width: '100%', minHeight: 140, fontFamily: 'monospace',
                  fontSize: 12, resize: 'vertical', boxSizing: 'border-box',
                }}
                value={jsonText}
                onChange={(e) => handleJsonChange(e.target.value)}
                spellCheck={false}
              />
              {jsonError && (
                <div style={{ marginTop: 4, fontSize: 12, color: 'var(--danger)' }}>{jsonError}</div>
              )}
            </div>
          ) : (
            <FormBuilder contexts={formContexts} envId={envId ?? null} onChange={handleFormChange} />
          )}
        </div>

        <div style={{ padding: '0 16px 14px', display: 'flex', alignItems: 'center', gap: 10 }}>
          <button
            className="btn primary"
            disabled={!canEvaluate}
            onClick={evaluate}
          >
            {loading ? 'Evaluating…' : 'Evaluate'}
          </button>
          {stale && results !== null && (
            <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>Results may be stale</span>
          )}
        </div>
      </div>

      {/* ── Results panel ── */}
      {apiError && (
        <div style={{
          padding: '10px 14px', borderRadius: 6, background: 'var(--danger-bg, #fee2e2)',
          color: 'var(--danger)', fontSize: 13,
        }}>
          {apiError}
        </div>
      )}

      {results !== null && results.length === 0 && (
        <div style={{ fontSize: 13, color: 'var(--fg-muted)', textAlign: 'center', padding: 24 }}>
          No contexts to evaluate
        </div>
      )}

      {results !== null && results.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          {results.map((r, i) => (
            <ContextResultCard key={i} result={r} />
          ))}
        </div>
      )}
    </div>
  )
}
