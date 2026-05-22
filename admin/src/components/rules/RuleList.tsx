import { useState } from 'react'
import { I } from '../icons'
import { RuleCard } from './RuleCard'
import { PercentageRolloutEditor } from './PercentageRolloutEditor'
import type { RuleState, RuleOutputJson, ConditionExpr, AllocationOutput } from '../../lib/ruleTypes'
import { defaultCondition, defaultOutput, localId, isVariantOutput, defaultAllocationOutput } from '../../lib/ruleTypes'
import { formatSelector } from '../../lib/hashInputTypes'

interface Props {
  rules: RuleState[]
  variants: string[]
  /** For the DISABLED state: the scalar default variant (PUT /flags/{key}). */
  defaultVariantKey: string | null
  /** For the ENABLED catch-all: the output of the last always-true rule. */
  catchAllOutput: RuleOutputJson
  /** Optional name for the catch-all rule (empty string = unnamed). */
  catchAllName?: string
  flagEnabled: boolean
  onChange: (rules: RuleState[]) => void
  /** Called when the catch-all output changes; parent marks dirty + saves with rules. */
  onCatchAllChange: (output: RuleOutputJson) => void
  /** Called when the catch-all name changes. */
  onCatchAllNameChange?: (name: string) => void
  canWrite?: boolean
  /** Only used when flag is DISABLED — auto-saves default_variant_key. */
  onSaveDefaultVariant?: (key: string) => Promise<void>
  /** Environment ID passed to the segment picker for lazy-loading segments. */
  envId?: string | null
  /** Org ID used to build segment detail links. */
  orgId?: string
}

export function RuleList({
  rules, variants, defaultVariantKey, catchAllOutput, catchAllName, flagEnabled,
  onChange, onCatchAllChange, onCatchAllNameChange,
  canWrite, onSaveDefaultVariant, envId, orgId,
}: Props) {
  const [dragSourceIndex, setDragSourceIndex] = useState<number | null>(null)
  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null)

  function addRule() {
    const newRule: RuleState = {
      _localId: localId(),
      // Start as And([leaf]) so the "Add condition" / "Add group" buttons are
      // immediately available for building multi-condition rules.
      condition: { And: [defaultCondition()] },
      output: defaultOutput(variants[0] ?? ''),
    }
    onChange([newRule, ...rules])
  }

  function updateRule(i: number, condition: ConditionExpr, output: RuleOutputJson) {
    onChange(rules.map((r, j) => j === i ? { ...r, condition, output } : r))
  }

  function updateRuleName(i: number, name: string) {
    onChange(rules.map((r, j) => j === i ? { ...r, name: name || undefined } : r))
  }

  function deleteRule(i: number) {
    onChange(rules.filter((_, j) => j !== i))
  }

  function onDragStart(i: number) { setDragSourceIndex(i) }
  function onDragOver(e: React.DragEvent, i: number) { e.preventDefault(); setDragOverIndex(i) }
  function onDrop(e: React.DragEvent, dropIndex: number) {
    e.preventDefault()
    if (dragSourceIndex === null || dragSourceIndex === dropIndex) {
      setDragSourceIndex(null); setDragOverIndex(null); return
    }
    const next = [...rules]
    const [moved] = next.splice(dragSourceIndex, 1)
    next.splice(dropIndex, 0, moved)
    onChange(next)
    setDragSourceIndex(null); setDragOverIndex(null)
  }
  function onDragEnd() { setDragSourceIndex(null); setDragOverIndex(null) }

  const footer = (
    <DefaultRuleFooter
      flagEnabled={flagEnabled}
      defaultVariantKey={defaultVariantKey}
      catchAllOutput={catchAllOutput}
      catchAllName={catchAllName}
      variants={variants}
      canWrite={canWrite}
      onCatchAllChange={onCatchAllChange}
      onCatchAllNameChange={onCatchAllNameChange}
      onSaveDefaultVariant={onSaveDefaultVariant}
      envId={envId}
    />
  )

  if (rules.length === 0) {
    return (
      <div>
        <div className="card">
          <div className="empty" style={{ padding: '32px 24px' }}>
            <div className="empty-icon"><I.toggle size={20} /></div>
            <div className="empty-title">No targeting rules yet</div>
            <div className="empty-desc">
              All contexts will receive the default variant.<br />
              Add a rule to target specific users or segments.
            </div>
            <button className="btn primary" style={{ marginTop: 8 }} onClick={addRule}>
              <I.plus size={13} /> Add first rule
            </button>
          </div>
        </div>
        {footer}
      </div>
    )
  }

  return (
    <div>
      <div style={{ display: 'flex', justifyContent: 'flex-end', marginBottom: 8 }}>
        <button className="btn sm" onClick={addRule}><I.plus size={12} /> Add rule</button>
      </div>

      {rules.map((rule, i) => (
        <div
          key={rule._localId}
          draggable
          onDragStart={() => onDragStart(i)}
          onDragOver={(e) => onDragOver(e, i)}
          onDrop={(e) => onDrop(e, i)}
          onDragEnd={onDragEnd}
          style={{
            opacity: dragSourceIndex === i ? 0.4 : 1,
            outline: dragOverIndex === i ? '2px solid var(--accent)' : 'none',
            outlineOffset: 2,
            borderRadius: 8,
            transition: 'outline 0.1s',
          }}
        >
          <RuleCard
            index={i}
            name={rule.name}
            condition={rule.condition}
            output={rule.output}
            variants={variants}
            onChange={(c, o) => updateRule(i, c, o)}
            onNameChange={(n) => updateRuleName(i, n)}
            onDelete={() => deleteRule(i)}
            dragHandleProps={{ style: { cursor: 'grab' } }}
            envId={envId}
            orgId={orgId}
          />
        </div>
      ))}

      {footer}
    </div>
  )
}

// ─── DefaultRuleFooter ────────────────────────────────────────────────────────

function DefaultRuleFooter({
  flagEnabled,
  defaultVariantKey,
  catchAllOutput,
  catchAllName,
  variants,
  canWrite,
  onCatchAllChange,
  onCatchAllNameChange,
  onSaveDefaultVariant,
  envId,
}: {
  flagEnabled: boolean
  defaultVariantKey: string | null
  catchAllOutput: RuleOutputJson
  catchAllName?: string
  variants: string[]
  canWrite?: boolean
  onCatchAllChange: (o: RuleOutputJson) => void
  onCatchAllNameChange?: (name: string) => void
  onSaveDefaultVariant?: (key: string) => Promise<void>
  envId?: string | null
}) {
  // ── DISABLED state: simple single-variant auto-save ──────────────────────
  if (!flagEnabled) {
    return (
      <DisabledDefaultFooter
        defaultVariantKey={defaultVariantKey}
        variants={variants}
        canWrite={canWrite}
        onSaveDefaultVariant={onSaveDefaultVariant}
      />
    )
  }

  // ── ENABLED state: full output editor (variant or allocation) ────────────
  return (
    <div style={{
      marginTop: 4,
      padding: '14px 16px',
      background: 'var(--bg-sunken)',
      border: '1px solid var(--border-faint)',
      borderRadius: 8,
    }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
        <I.info size={13} style={{ color: 'var(--fg-subtle)', flexShrink: 0 }} />
        <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--fg-muted)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
          Default rule (catch-all)
        </span>
        <span style={{ fontSize: 11, color: 'var(--fg-subtle)', marginLeft: 4 }}>
          — applied to all remaining contexts
        </span>
      </div>

      {canWrite && onCatchAllNameChange && (
        <div style={{ marginBottom: 12 }}>
          <div style={{ fontSize: 11, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.06em', fontWeight: 600, marginBottom: 6 }}>
            Rule name (optional)
          </div>
          <input
            className="input"
            placeholder="Default rule"
            value={catchAllName ?? ''}
            onChange={(e) => onCatchAllNameChange(e.target.value)}
            style={{ width: '100%', fontSize: 13 }}
          />
        </div>
      )}

      {canWrite ? (
        <CatchAllOutputEditor
          output={catchAllOutput}
          variants={variants}
          onChange={onCatchAllChange}
          envId={envId}
        />
      ) : (
        <CatchAllReadOnly output={catchAllOutput} />
      )}
    </div>
  )
}

// ─── Disabled state: simple variant picker with auto-save ────────────────────

function DisabledDefaultFooter({
  defaultVariantKey, variants, canWrite, onSaveDefaultVariant,
}: {
  defaultVariantKey: string | null
  variants: string[]
  canWrite?: boolean
  onSaveDefaultVariant?: (key: string) => Promise<void>
}) {
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleChange(e: React.ChangeEvent<HTMLSelectElement>) {
    if (!onSaveDefaultVariant) return
    setSaving(true); setError(null)
    try { await onSaveDefaultVariant(e.target.value) }
    catch { setError('Failed to save') }
    finally { setSaving(false) }
  }

  return (
    <div style={{ padding: '12px 14px', background: 'var(--bg-sunken)', border: '1px solid var(--border-faint)', borderRadius: 8, fontSize: 12, color: 'var(--fg-muted)', marginTop: 4 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
        <I.info size={13} style={{ flexShrink: 0 }} />
        <span style={{ whiteSpace: 'nowrap' }}>Flag disabled — all contexts receive</span>
        {canWrite && onSaveDefaultVariant ? (
          <select
            className="input"
            value={defaultVariantKey ?? variants[0] ?? ''}
            onChange={handleChange}
            disabled={saving}
            style={{ padding: '2px 6px', fontSize: 12, height: 26, minWidth: 100, fontFamily: 'var(--font-mono)', fontWeight: 600 }}
          >
            {variants.map((v) => <option key={v} value={v}>{v}</option>)}
          </select>
        ) : (
          <span className="badge">{defaultVariantKey ?? 'off'}</span>
        )}
        {saving && <span style={{ color: 'var(--fg-subtle)', fontStyle: 'italic' }}>saving…</span>}
      </div>
      {error && <div style={{ marginTop: 4, marginLeft: 23, color: 'var(--danger)', fontSize: 11 }}>{error}</div>}
    </div>
  )
}

// ─── Enabled catch-all: read-only summary ────────────────────────────────────

function CatchAllReadOnly({ output }: { output: RuleOutputJson }) {
  if (isVariantOutput(output)) {
    return (
      <div style={{ fontSize: 12, color: 'var(--fg-muted)', display: 'flex', alignItems: 'center', gap: 8 }}>
        <span>Serve</span>
        <span className="badge accent">{(output as { variant_key: string }).variant_key}</span>
        <span>to all remaining contexts</span>
      </div>
    )
  }
  const { hash_inputs, buckets } = (output as { allocation: AllocationOutput }).allocation
  const targetSummary = hash_inputs.map(formatSelector).join(', ')
  return (
    <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap', marginBottom: 4 }}>
        <span>Percentage rollout:</span>
        {buckets.map((b) => (
          <span key={b.variant_key} style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            <span className="badge accent">{b.variant_key}</span>
            <span>{(b.weight_milli / 10).toFixed(1)}%</span>
          </span>
        ))}
      </div>
      <div style={{ color: 'var(--fg-subtle)', fontSize: 11 }}>
        hash by: {targetSummary || '—'}
      </div>
    </div>
  )
}

// ─── Enabled catch-all: editable output (variant or allocation) ───────────────

function CatchAllOutputEditor({
  output, variants, onChange, envId,
}: {
  output: RuleOutputJson
  variants: string[]
  onChange: (o: RuleOutputJson) => void
  envId?: string | null
}) {
  const isVariant = isVariantOutput(output)
  const radioName = 'catchall-output-type'

  return (
    <div>
      <div style={{ display: 'flex', gap: 16, marginBottom: 12 }}>
        <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 13, cursor: 'pointer' }}>
          <input
            type="radio" name={radioName} value="variant" checked={isVariant}
            onChange={() => onChange({ variant_key: variants[0] ?? '' })}
          />
          Serve variant
        </label>
        <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 13, cursor: 'pointer' }}>
          <input
            type="radio" name={radioName} value="allocation" checked={!isVariant}
            onChange={() => onChange({ allocation: defaultAllocationOutput(variants) })}
          />
          Percentage rollout
        </label>
      </div>

      {isVariant ? (
        <select
          className="input"
          style={{ width: 160, fontFamily: 'var(--font-mono)', fontSize: 12 }}
          value={(output as { variant_key: string }).variant_key}
          onChange={(e) => onChange({ variant_key: e.target.value })}
        >
          {variants.map((v) => <option key={v} value={v}>{v}</option>)}
        </select>
      ) : (
        <PercentageRolloutEditor
          value={(output as { allocation: AllocationOutput }).allocation}
          variants={variants}
          onChange={(alloc) => onChange({ allocation: alloc })}
          envId={envId}
        />
      )}
    </div>
  )
}
