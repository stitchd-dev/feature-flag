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

/** Recursive condition evaluation node — mirrors ConditionExpr from the rule engine. */
type ConditionNode =
  | { kind: 'leaf'; predicate: string; result: boolean }
  | { kind: 'and' | 'or'; result: boolean; children: ConditionNode[] }
  | { kind: 'not'; result: boolean; child: ConditionNode }

interface RuleTrace {
  rule_name: string | null
  outcome: 'match' | 'no_match' | 'skipped'
  condition_tree?: ConditionNode | null
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

// ── Recursive condition tree renderer ────────────────────────────────────────

/** Render a single condition leaf as a colored pill. */
function LeafPill({ predicate, result }: { predicate: string; result: boolean }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
      <span style={{
        fontSize: 11, fontWeight: 700, width: 12, flexShrink: 0,
        color: result ? 'var(--success, #22c55e)' : 'var(--danger)',
      }}>
        {result ? '✓' : '✗'}
      </span>
      <code style={{
        fontSize: 11, padding: '2px 7px', borderRadius: 4,
        background: result ? 'var(--success-bg, #f0fdf4)' : 'var(--danger-bg, #fee2e2)',
        color: result ? 'var(--success-fg, #166534)' : 'var(--danger-fg, #991b1b)',
        border: `1px solid ${result ? 'var(--success-border, #86efac)' : 'var(--danger-border, #fca5a5)'}`,
      }}>
        {predicate}
      </code>
    </div>
  )
}

/** Recursively render a ConditionNode tree (AND / OR / NOT / Leaf). */
function ConditionTree({ node, depth = 0 }: { node: ConditionNode; depth?: number }) {
  if (node.kind === 'leaf') {
    return <LeafPill predicate={node.predicate} result={node.result} />
  }

  if (node.kind === 'not') {
    return (
      <div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 4 }}>
          <span style={{
            fontSize: 10, fontWeight: 700, padding: '1px 6px', borderRadius: 3,
            background: 'var(--bg-sunken)', color: 'var(--fg-muted)',
            border: '1px solid var(--border)',
          }}>NOT</span>
          <span style={{ fontSize: 11, color: node.result ? 'var(--success, #22c55e)' : 'var(--danger)', fontWeight: 600 }}>
            {node.result ? '✓' : '✗'}
          </span>
        </div>
        <div style={{ marginLeft: 12, paddingLeft: 10, borderLeft: '2px solid var(--border-faint)' }}>
          <ConditionTree node={node.child} depth={depth + 1} />
        </div>
      </div>
    )
  }

  // AND / OR group — use inter-item separators so the operator is visible
  // between every pair of siblings at every nesting level.
  const isOr = node.kind === 'or'
  const label = isOr ? 'OR' : 'AND'

  // Separator colours: OR gets a blue accent, AND stays neutral grey.
  const sepBg     = isOr ? 'var(--accent-subtle, #eff6ff)'    : 'var(--bg-sunken)'
  const sepColor  = isOr ? 'var(--accent, #3b82f6)'           : 'var(--fg-muted)'
  const sepBorder = isOr ? 'var(--accent-border, #bfdbfe)'    : 'var(--border)'
  // Left-border for nested groups mirrors the separator colour.
  const leftBorder = isOr ? '2px solid var(--accent-border, #bfdbfe)' : '2px solid var(--border-faint)'

  const children = (
    <div style={{ display: 'flex', flexDirection: 'column' }}>
      {node.children.map((child, i) => (
        <div key={i}>
          {/* Inter-item separator — shown BETWEEN siblings, never before the first */}
          {i > 0 && (
            <div style={{ display: 'flex', alignItems: 'center', padding: '4px 0' }}>
              <span style={{
                fontSize: 10, fontWeight: 700, padding: '1px 6px', borderRadius: 3,
                background: sepBg, color: sepColor, border: `1px solid ${sepBorder}`,
                letterSpacing: '0.04em',
              }}>{label}</span>
            </div>
          )}
          <ConditionTree node={child} depth={depth + 1} />
        </div>
      ))}
    </div>
  )

  // Top-level group: render flat — the separators alone make the operator obvious.
  if (depth === 0) return children

  // Nested group: wrap in an indented bordered container so the scope is clear.
  return (
    <div style={{ marginLeft: 6, paddingLeft: 10, borderLeft: leftBorder }}>
      {children}
    </div>
  )
}

/** One rule row — always expanded, with recursive condition tree. */
function RuleRow({
  trace,
  index,
  isCatchAll,
  firedVariant,
}: {
  trace: RuleTrace
  index: number
  isCatchAll: boolean
  firedVariant: string | null
}) {
  const matched = trace.outcome === 'match'
  const skipped = trace.outcome === 'skipped'
  const name = trace.rule_name ?? (isCatchAll ? 'Default rule' : `Rule ${index + 1}`)
  const hasTree = !!trace.condition_tree

  return (
    <div style={{
      borderRadius: 6,
      border: `1px solid ${matched ? 'var(--success-border, #86efac)' : 'var(--border)'}`,
      overflow: 'hidden',
      opacity: skipped ? 0.45 : 1,
    }}>
      {/* Rule header */}
      <div style={{
        display: 'flex', alignItems: 'center', gap: 8,
        padding: '7px 12px',
        background: matched ? 'var(--success-bg, #f0fdf4)' : 'var(--surface)',
        borderBottom: hasTree ? '1px solid var(--border-faint)' : 'none',
      }}>
        <span style={{
          width: 20, height: 20, borderRadius: '50%', flexShrink: 0,
          background: matched ? 'var(--success, #22c55e)' : 'var(--bg-sunken)',
          color: matched ? '#fff' : 'var(--fg-muted)',
          fontSize: 10, fontWeight: 700,
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }}>
          {matched ? '✓' : index + 1}
        </span>

        <span style={{ fontSize: 12, fontWeight: matched ? 600 : 400, flex: 1, color: matched ? 'var(--fg)' : 'var(--fg-muted)' }}>
          {name}
        </span>

        {matched && (
          <span style={{
            fontSize: 11, padding: '1px 8px', borderRadius: 10, fontWeight: 600,
            background: 'var(--success, #22c55e)', color: '#fff',
          }}>
            matched → {firedVariant ?? 'on'}
          </span>
        )}
        {trace.outcome === 'no_match' && (
          <span style={{ fontSize: 11, color: 'var(--fg-subtle)' }}>no match</span>
        )}
        {skipped && (
          <span style={{ fontSize: 11, color: 'var(--fg-subtle)' }}>skipped</span>
        )}
      </div>

      {/* Condition tree — always expanded */}
      {hasTree && (
        <div style={{ padding: '8px 12px 10px 40px' }}>
          <ConditionTree node={trace.condition_tree!} />
        </div>
      )}

      {/* Catch-all / skipped with no conditions */}
      {!hasTree && !skipped && (
        <div style={{ padding: '6px 12px 8px 40px', fontSize: 11, color: 'var(--fg-subtle)', fontStyle: 'italic' }}>
          catch-all — applies to all remaining contexts
        </div>
      )}
    </div>
  )
}

/** Full rule evaluation tree for one context. */
function ContextRuleTree({ result }: { result: ContextResult }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
      {result.rule_traces.map((trace, i) => (
        <RuleRow
          key={i}
          trace={trace}
          index={i}
          isCatchAll={i === result.rule_traces.length - 1}
          firedVariant={trace.outcome === 'match' ? (result.variant_key ?? null) : null}
        />
      ))}
      {result.rollout_debug && <RolloutDebugPanel debug={result.rollout_debug} />}
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

/** Unified result panel — one card regardless of how many contexts were evaluated. */
function EvaluationResults({ results }: { results: ContextResult[] }) {
  const single = results.length === 1

  return (
    <div className="card">
      {results.map((result, ri) => (
        <div key={ri}>
          {/* ── Context header ── */}
          <div style={{
            display: 'flex', alignItems: 'center', gap: 8,
            padding: single ? '12px 16px 10px' : ri === 0 ? '12px 16px 10px' : '14px 16px 10px',
            borderTop: ri > 0 ? '1px solid var(--border)' : 'none',
          }}>
            <span style={{ fontSize: 12, fontFamily: 'var(--font-mono)', color: 'var(--fg-muted)' }}>
              {result.context_key || '(no key)'}
            </span>
            {result.disabled ? (
              <span style={{
                fontSize: 11, padding: '1px 8px', borderRadius: 10,
                background: 'var(--fg-muted)', color: 'var(--bg)', opacity: 0.7,
              }}>disabled</span>
            ) : (
              <span style={{
                fontSize: 11, padding: '1px 8px', borderRadius: 10, fontWeight: 600,
                background: 'var(--accent)', color: '#fff',
              }}>{result.variant_key ?? 'default'}</span>
            )}
            {result.disabled && (
              <span style={{ fontSize: 11, color: 'var(--warning, #d97706)' }}>
                flag disabled — default rule applied
              </span>
            )}
          </div>

          {/* ── Rule tree ── */}
          <div style={{ padding: '0 16px 14px' }}>
            <ContextRuleTree result={result} />
          </div>
        </div>
      ))}
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

  const hasValidContext = mode === 'form'
    ? formContexts.some((c) => c._type.trim() !== '' && c.key.trim() !== '')
    : (() => { const { contexts } = parseContextsJson(jsonText); return contexts.some((c) => c._type.trim() !== '' && c.key.trim() !== '') })()
  const canEvaluate = !jsonError && !loading && hasValidContext

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
            title={!hasValidContext ? 'Fill in _type and key for at least one context' : undefined}
            onClick={evaluate}
          >
            {loading ? 'Evaluating…' : 'Evaluate'}
          </button>
          {!hasValidContext && !jsonError && (
            <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
              Set <code>_type</code> and <code>key</code> on at least one context to evaluate
            </span>
          )}
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
        <EvaluationResults results={results} />
      )}
    </div>
  )
}
