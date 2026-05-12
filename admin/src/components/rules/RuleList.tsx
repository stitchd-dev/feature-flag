import { useState } from 'react'
import { I } from '../icons'
import { RuleCard } from './RuleCard'
import type { RuleState, RuleOutputJson, ConditionExpr } from '../../lib/ruleTypes'
import { defaultCondition, defaultOutput, localId } from '../../lib/ruleTypes'

interface Props {
  rules: RuleState[]
  variants: string[]
  defaultVariantKey: string | null
  onChange: (rules: RuleState[]) => void
  canWrite?: boolean
  onSaveDefaultVariant?: (key: string) => Promise<void>
}

export function RuleList({ rules, variants, defaultVariantKey, onChange, canWrite, onSaveDefaultVariant }: Props) {
  const [dragSourceIndex, setDragSourceIndex] = useState<number | null>(null)
  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null)

  function addRule() {
    const newRule: RuleState = {
      _localId: localId(),
      condition: defaultCondition(),
      output: defaultOutput(variants[0] ?? ''),
    }
    onChange([newRule, ...rules])
  }

  function updateRule(i: number, condition: ConditionExpr, output: RuleOutputJson) {
    onChange(rules.map((r, j) => j === i ? { ...r, condition, output } : r))
  }

  function deleteRule(i: number) {
    onChange(rules.filter((_, j) => j !== i))
  }

  function onDragStart(i: number) {
    setDragSourceIndex(i)
  }

  function onDragOver(e: React.DragEvent, i: number) {
    e.preventDefault()
    setDragOverIndex(i)
  }

  function onDrop(e: React.DragEvent, dropIndex: number) {
    e.preventDefault()
    if (dragSourceIndex === null || dragSourceIndex === dropIndex) {
      setDragSourceIndex(null)
      setDragOverIndex(null)
      return
    }
    const next = [...rules]
    const [moved] = next.splice(dragSourceIndex, 1)
    next.splice(dropIndex, 0, moved)
    onChange(next)
    setDragSourceIndex(null)
    setDragOverIndex(null)
  }

  function onDragEnd() {
    setDragSourceIndex(null)
    setDragOverIndex(null)
  }

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
        <DefaultRuleFooter
          variantKey={defaultVariantKey}
          variants={variants}
          canWrite={canWrite}
          onSaveDefaultVariant={onSaveDefaultVariant}
        />
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
            condition={rule.condition}
            output={rule.output}
            variants={variants}
            onChange={(c, o) => updateRule(i, c, o)}
            onDelete={() => deleteRule(i)}
            dragHandleProps={{ style: { cursor: 'grab' } }}
          />
        </div>
      ))}

      <DefaultRuleFooter
        variantKey={defaultVariantKey}
        variants={variants}
        canWrite={canWrite}
        onSaveDefaultVariant={onSaveDefaultVariant}
      />
    </div>
  )
}

function DefaultRuleFooter({
  variantKey,
  variants,
  canWrite,
  onSaveDefaultVariant,
}: {
  variantKey: string | null
  variants: string[]
  canWrite?: boolean
  onSaveDefaultVariant?: (key: string) => Promise<void>
}) {
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleChange(e: React.ChangeEvent<HTMLSelectElement>) {
    const next = e.target.value
    if (!onSaveDefaultVariant) return
    setSaving(true)
    setError(null)
    try {
      await onSaveDefaultVariant(next)
    } catch {
      setError('Failed to save')
    } finally {
      setSaving(false)
    }
  }

  return (
    <div style={{ padding: '12px 14px', background: 'var(--bg-sunken)', border: '1px solid var(--border-faint)', borderRadius: 8, fontSize: 12, color: 'var(--fg-muted)', marginTop: 4 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
        <I.info size={13} style={{ flexShrink: 0 }} />
        <span style={{ whiteSpace: 'nowrap' }}>Default rule (catch-all): serve</span>
        {canWrite && onSaveDefaultVariant ? (
          <select
            className="input"
            value={variantKey ?? variants[0] ?? ''}
            onChange={handleChange}
            disabled={saving}
            style={{ padding: '2px 6px', fontSize: 12, height: 26, minWidth: 100, fontFamily: 'var(--font-mono)', fontWeight: 600 }}
          >
            {variants.map((v) => (
              <option key={v} value={v}>{v}</option>
            ))}
          </select>
        ) : (
          <span className="badge">{variantKey ?? 'off'}</span>
        )}
        <span style={{ whiteSpace: 'nowrap' }}>to all remaining contexts</span>
        {saving && <span style={{ color: 'var(--fg-subtle)', fontStyle: 'italic' }}>saving…</span>}
      </div>
      {error && (
        <div style={{ marginTop: 4, marginLeft: 23, color: 'var(--danger)', fontSize: 11 }}>{error}</div>
      )}
    </div>
  )
}
