import { useState } from 'react'
import { I } from '../icons'
import { ConditionClauseEditor } from './ConditionClauseEditor'
import { PercentageRolloutEditor } from './PercentageRolloutEditor'
import type { ConditionExpr, Condition, RuleOutputJson, AllocationOutput } from '../../lib/ruleTypes'
import { exprKey, isVariantOutput, localId, defaultCondition, defaultAllocationOutput } from '../../lib/ruleTypes'

// ─── ConditionExprEditor (recursive) ─────────────────────────────────────────

function ConditionExprEditor({
  expr, onChange, onDelete, depth = 0, envId, orgId,
}: {
  expr: ConditionExpr
  onChange: (e: ConditionExpr) => void
  onDelete?: () => void
  depth?: number
  envId?: string | null
  orgId?: string
}) {
  const key = exprKey(expr)

  if (key === 'Leaf') {
    const condition = (expr as { Leaf: Condition }).Leaf
    return (
      <div style={{ padding: '4px 0' }}>
        <ConditionClauseEditor
          condition={condition}
          onChange={(c) => onChange({ Leaf: c })}
          onDelete={() => onDelete?.()}
          envId={envId}
          orgId={orgId}
        />
      </div>
    )
  }

  if (key === 'Not') {
    const inner = (expr as { Not: ConditionExpr }).Not
    const opColor = 'var(--danger)'
    return (
      <div style={{
        padding: '8px 10px 8px 14px', borderLeft: `2px solid ${opColor}`,
        background: depth > 0 ? 'var(--bg-sunken)' : 'transparent', borderRadius: 4,
      }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10, fontWeight: 700, color: opColor, padding: '2px 6px', background: 'var(--surface)', border: `1px solid ${opColor}`, borderRadius: 3 }}>NOT</span>
          <button className="btn sm" style={{ fontSize: 11 }} onClick={() => onChange(inner)}>Remove NOT</button>
          {onDelete && <button className="icon-btn" style={{ color: 'var(--danger)', marginLeft: 'auto' }} onClick={onDelete}><I.x size={12} /></button>}
        </div>
        <ConditionExprEditor expr={inner} depth={depth + 1} onChange={(e) => onChange({ Not: e })} envId={envId} orgId={orgId} />
      </div>
    )
  }

  // And / Or group
  const isAnd = key === 'And'
  const children = (expr as { And?: ConditionExpr[]; Or?: ConditionExpr[] })[key as 'And' | 'Or'] ?? []
  const opColor = isAnd ? 'var(--info)' : '#A892FF'

  function updateChild(i: number, child: ConditionExpr) {
    const next = children.map((c, j) => j === i ? child : c)
    onChange({ [key]: next } as ConditionExpr)
  }

  function deleteChild(i: number) {
    const next = children.filter((_, j) => j !== i)
    if (next.length === 0) {
      // Replace empty group with a new default leaf
      onChange(defaultCondition())
    } else if (next.length === 1 && depth > 0) {
      // Collapse single-child group
      onChange(next[0])
    } else {
      onChange({ [key]: next } as ConditionExpr)
    }
  }

  function addLeaf() {
    onChange({ [key]: [...children, defaultCondition()] } as ConditionExpr)
  }

  function addGroup() {
    onChange({ [key]: [...children, { And: [defaultCondition(), defaultCondition()] }] } as ConditionExpr)
  }

  function toggleAndOr() {
    const newKey = isAnd ? 'Or' : 'And'
    onChange({ [newKey]: children } as ConditionExpr)
  }

  return (
    <div style={{
      position: 'relative',
      padding: depth > 0 ? '10px 10px 10px 14px' : '0',
      background: depth > 0 ? 'var(--bg-sunken)' : 'transparent',
      borderRadius: 6,
      border: depth > 0 ? '1px solid var(--border-faint)' : 'none',
      borderLeft: depth > 0 ? `2px solid ${opColor}` : 'none',
    }}>
      {depth > 0 && (
        <div style={{ position: 'absolute', top: -9, left: 10, display: 'flex', alignItems: 'center', gap: 6 }}>
          <button
            style={{ padding: '1px 7px', background: 'var(--surface)', border: `1px solid ${opColor}`, borderRadius: 4, fontFamily: 'var(--font-mono)', fontSize: 10, fontWeight: 700, color: opColor, cursor: 'pointer', letterSpacing: '0.05em' }}
            onClick={toggleAndOr}
          >{key}</button>
          {onDelete && <button className="icon-btn" style={{ color: 'var(--danger)' }} onClick={onDelete}><I.x size={11} /></button>}
        </div>
      )}

      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        {children.map((child, i) => (
          <div key={i} style={{ display: 'flex', alignItems: 'flex-start', gap: 6 }}>
            {i > 0 && (
              <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10, fontWeight: 700, color: opColor, padding: '4px 6px', background: 'var(--surface)', border: `1px solid ${opColor}33`, borderRadius: 3, marginTop: 6, letterSpacing: '0.05em', flexShrink: 0 }}>{key}</span>
            )}
            <div style={{ flex: 1 }}>
              <ConditionExprEditor
                expr={child}
                depth={depth + 1}
                onChange={(e) => updateChild(i, e)}
                onDelete={() => deleteChild(i)}
                envId={envId}
                orgId={orgId}
              />
            </div>
          </div>
        ))}
      </div>

      <div style={{ display: 'flex', gap: 6, marginTop: 8, flexWrap: 'wrap' }}>
        <button className="btn sm" onClick={addLeaf}><I.plus size={11} /> Add condition</button>
        <button className="btn sm" onClick={addGroup}><I.plus size={11} /> Add group</button>
        {depth === 0 && (
          <button className="btn sm" onClick={() => onChange({ Not: expr })}>Wrap in NOT</button>
        )}
      </div>
    </div>
  )
}

// ─── OutputEditor ─────────────────────────────────────────────────────────────

export function OutputEditor({
  output, variants, onChange,
}: {
  output: RuleOutputJson
  variants: string[]
  onChange: (o: RuleOutputJson) => void
}) {
  const isVariant = isVariantOutput(output)
  const radioName = `output-${localId()}`

  return (
    <div>
      <div style={{ display: 'flex', gap: 16, marginBottom: 10 }}>
        <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 13, cursor: 'pointer' }}>
          <input type="radio" name={radioName} checked={isVariant}
            onChange={() => onChange({ variant_key: variants[0] ?? '' })} />
          Serve variant
        </label>
        <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 13, cursor: 'pointer' }}>
          <input type="radio" name={radioName} checked={!isVariant}
            onChange={() => onChange({ allocation: defaultAllocationOutput(variants) })} />
          Percentage rollout
        </label>
      </div>

      {isVariant ? (
        <select className="input" style={{ width: 160, fontFamily: 'var(--font-mono)', fontSize: 12 }}
          value={(output as { variant_key: string }).variant_key}
          onChange={(e) => onChange({ variant_key: e.target.value })}>
          {variants.map((v) => <option key={v} value={v}>{v}</option>)}
        </select>
      ) : (
        <PercentageRolloutEditor
          value={(output as { allocation: AllocationOutput }).allocation}
          variants={variants}
          onChange={(alloc) => onChange({ allocation: alloc })}
        />
      )}
    </div>
  )
}

// ─── RuleCard ─────────────────────────────────────────────────────────────────

interface Props {
  index: number
  condition: ConditionExpr
  output: RuleOutputJson
  variants: string[]
  onChange: (condition: ConditionExpr, output: RuleOutputJson) => void
  onDelete: () => void
  dragHandleProps?: React.HTMLAttributes<HTMLDivElement>
  envId?: string | null
  orgId?: string
}

export function RuleCard({ index, condition, output, variants, onChange, onDelete, dragHandleProps, envId, orgId }: Props) {
  const [expanded, setExpanded] = useState(true)

  return (
    <div className="card" style={{ marginBottom: 8, overflow: 'hidden' }}>
      <div className="card-header" style={{ gap: 8 }}>
        <div {...dragHandleProps} style={{ cursor: 'grab', color: 'var(--fg-subtle)', flexShrink: 0, ...dragHandleProps?.style }}>
          <I.grip size={14} />
        </div>
        <div style={{ width: 26, height: 26, borderRadius: 6, background: 'var(--bg-sunken)', color: 'var(--fg-muted)', display: 'grid', placeItems: 'center', fontFamily: 'var(--font-mono)', fontSize: 12, fontWeight: 600, flexShrink: 0 }}>
          {index + 1}
        </div>
        <div style={{ flex: 1, fontSize: 13, color: 'var(--fg-muted)' }}>
          {isVariantOutput(output)
            ? <span>→ <span className="badge accent">{(output as { variant_key: string }).variant_key}</span></span>
            : <span>→ percentage rollout ({(output as { allocation: AllocationOutput }).allocation.buckets.length} variants)</span>
          }
        </div>
        <button className="icon-btn" onClick={() => setExpanded((v) => !v)}>
          {expanded ? <I.chevronUp size={14} /> : <I.chevronDown size={14} />}
        </button>
        <button className="icon-btn" style={{ color: 'var(--danger)' }} onClick={onDelete}>
          <I.trash size={13} />
        </button>
      </div>

      {expanded && (
        <div style={{ padding: 14, borderTop: '1px solid var(--border-faint)' }}>
          <div style={{ fontSize: 11, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.06em', fontWeight: 600, marginBottom: 8 }}>When</div>
          <ConditionExprEditor
            expr={condition}
            onChange={(c) => onChange(c, output)}
            envId={envId}
            orgId={orgId}
          />

          <div style={{ fontSize: 11, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.06em', fontWeight: 600, marginTop: 16, marginBottom: 8 }}>Serve</div>
          <OutputEditor
            output={output}
            variants={variants}
            onChange={(o) => onChange(condition, o)}
          />
        </div>
      )}
    </div>
  )
}
