import { I } from '../icons'
import type { Condition, ParameterValue } from '../../lib/ruleTypes'
import { SegmentPicker } from './SegmentPicker'

const COMPARE_OPS = ['Eq', 'Ne', 'Lt', 'Lte', 'Gt', 'Gte'] as const
const STRING_OPS = ['Contains', 'StartsWith', 'EndsWith'] as const
const SEMVER_OPS = ['SemverGte', 'SemverTilde', 'SemverCaret'] as const
const SEGMENT_OPS = ['InSegment', 'NotInSegment'] as const
const FLAG_OPS = ['FlagEvaluatedAs'] as const

const OP_LABELS: Record<string, string> = {
  Eq: '==', Ne: '!=', Lt: '<', Lte: '<=', Gt: '>', Gte: '>=',
  Contains: 'contains', StartsWith: 'starts with', EndsWith: 'ends with',
  SemverGte: 'semver >=', SemverTilde: 'semver ~', SemverCaret: 'semver ^',
  InSegment: 'in segment', NotInSegment: 'not in segment',
  FlagEvaluatedAs: 'flag is',
}

const ALL_OPS = [...COMPARE_OPS, ...STRING_OPS, ...SEMVER_OPS, ...SEGMENT_OPS, ...FLAG_OPS]

function currentOpKey(c: Condition): string {
  return Object.keys(c)[0]
}

function buildCondition(op: string, prev: Condition): Condition {
  const pk = currentOpKey(prev)
  // Extract reusable fields from previous condition
  let ctx = 'user'; let param = ''
  if (COMPARE_OPS.includes(pk as (typeof COMPARE_OPS)[number]) || STRING_OPS.includes(pk as (typeof STRING_OPS)[number]) || SEMVER_OPS.includes(pk as (typeof SEMVER_OPS)[number])) {
    const payload = (prev as Record<string, { context_type: string; param: string }>)[pk]
    ctx = payload.context_type
    param = payload.param
  }

  if (COMPARE_OPS.includes(op as (typeof COMPARE_OPS)[number])) {
    return { [op]: { context_type: ctx, param, value: '' } } as Condition
  }
  if (op === 'Contains') return { Contains: { context_type: ctx, param, substr: '' } }
  if (op === 'StartsWith') return { StartsWith: { context_type: ctx, param, prefix: '' } }
  if (op === 'EndsWith') return { EndsWith: { context_type: ctx, param, suffix: '' } }
  if (SEMVER_OPS.includes(op as (typeof SEMVER_OPS)[number])) {
    return { [op]: { context_type: ctx, param, value: '' } } as Condition
  }
  if (op === 'InSegment') return { InSegment: '' }
  if (op === 'NotInSegment') return { NotInSegment: '' }
  if (op === 'FlagEvaluatedAs') return { FlagEvaluatedAs: { flag_id: '', variant_id: '' } }
  return { Eq: { context_type: 'user', param: '', value: '' } }
}

/** Ops that are forbidden inside a segment's own condition expression. */
const SEGMENT_FORBIDDEN_OPS = new Set(['InSegment', 'NotInSegment', 'FlagEvaluatedAs'])

interface Props {
  condition: Condition
  onChange: (c: Condition) => void
  onDelete: () => void
  /** Needed for the segment picker lazy-load. */
  envId?: string | null
  orgId?: string
  /**
   * 'flag' (default) — all operators available.
   * 'segment' — InSegment / NotInSegment / FlagEvaluatedAs are hidden
   *             (segments cannot self-reference or depend on flag evaluations).
   */
  mode?: 'flag' | 'segment'
}

export function ConditionClauseEditor({ condition, onChange, onDelete, envId, orgId, mode = 'flag' }: Props) {
  const op = currentOpKey(condition)

  function setOp(newOp: string) {
    onChange(buildCondition(newOp, condition))
  }

  // In segment mode, forbidden ops are hidden. If the current condition somehow
  // uses one (e.g. copy-paste), display a warning placeholder instead.
  const isForbidden = mode === 'segment' && SEGMENT_FORBIDDEN_OPS.has(op)

  const iInput = isForbidden ? (
    <span style={{ fontSize: 12, color: 'var(--danger)', fontFamily: 'var(--font-mono)', padding: '4px 6px', background: 'var(--danger-bg, rgba(220,53,69,0.1))', borderRadius: 3 }}>
      ⚠ {OP_LABELS[op] ?? op} (not allowed in segments)
    </span>
  ) : (
    <select
      className="input"
      style={{ width: 130, fontFamily: 'var(--font-mono)', fontSize: 12 }}
      value={op}
      onChange={(e) => setOp(e.target.value)}
    >
      <optgroup label="Equality">
        {COMPARE_OPS.map((o) => <option key={o} value={o}>{OP_LABELS[o]}</option>)}
      </optgroup>
      <optgroup label="String">
        {STRING_OPS.map((o) => <option key={o} value={o}>{OP_LABELS[o]}</option>)}
      </optgroup>
      <optgroup label="SemVer">
        {SEMVER_OPS.map((o) => <option key={o} value={o}>{OP_LABELS[o]}</option>)}
      </optgroup>
      {mode !== 'segment' && (
        <optgroup label="Segment">
          {SEGMENT_OPS.map((o) => <option key={o} value={o}>{OP_LABELS[o]}</option>)}
        </optgroup>
      )}
      {mode !== 'segment' && (
        <optgroup label="Flag">
          {FLAG_OPS.map((o) => <option key={o} value={o}>{OP_LABELS[o]}</option>)}
        </optgroup>
      )}
    </select>
  )

  function renderFields() {
    if (COMPARE_OPS.includes(op as (typeof COMPARE_OPS)[number])) {
      const payload = (condition as Record<string, { context_type: string; param: string; value: ParameterValue }>)[op]
      return (
        <>
          <input className="input" style={{ width: 90 }} placeholder="context" value={payload.context_type}
            onChange={(e) => onChange({ [op]: { ...payload, context_type: e.target.value } } as Condition)} />
          <input className="input" style={{ width: 110 }} placeholder="attribute" value={payload.param}
            onChange={(e) => onChange({ [op]: { ...payload, param: e.target.value } } as Condition)} />
          <input className="input" style={{ width: 120 }} placeholder="value" value={String(payload.value)}
            onChange={(e) => onChange({ [op]: { ...payload, value: e.target.value } } as Condition)} />
        </>
      )
    }

    if (op === 'Contains' || op === 'StartsWith' || op === 'EndsWith') {
      type StringPayload = { context_type: string; param: string; substr?: string; prefix?: string; suffix?: string }
      const payload = (condition as Record<string, StringPayload>)[op]
      const extraKey = op === 'Contains' ? 'substr' : op === 'StartsWith' ? 'prefix' : 'suffix'
      const extraVal = (payload[extraKey as keyof StringPayload] as string) ?? ''
      return (
        <>
          <input className="input" style={{ width: 90 }} placeholder="context" value={payload.context_type}
            onChange={(e) => onChange({ [op]: { ...payload, context_type: e.target.value } } as Condition)} />
          <input className="input" style={{ width: 110 }} placeholder="attribute" value={payload.param}
            onChange={(e) => onChange({ [op]: { ...payload, param: e.target.value } } as Condition)} />
          <input className="input" style={{ width: 120 }} placeholder={extraKey} value={extraVal}
            onChange={(e) => onChange({ [op]: { ...payload, [extraKey]: e.target.value } } as Condition)} />
        </>
      )
    }

    if (SEMVER_OPS.includes(op as (typeof SEMVER_OPS)[number])) {
      const payload = (condition as Record<string, { context_type: string; param: string; value: string }>)[op]
      return (
        <>
          <input className="input" style={{ width: 90 }} placeholder="context" value={payload.context_type}
            onChange={(e) => onChange({ [op]: { ...payload, context_type: e.target.value } } as Condition)} />
          <input className="input" style={{ width: 110 }} placeholder="attribute" value={payload.param}
            onChange={(e) => onChange({ [op]: { ...payload, param: e.target.value } } as Condition)} />
          <input className="input" style={{ width: 120 }} placeholder="1.2.3" value={payload.value}
            onChange={(e) => onChange({ [op]: { ...payload, value: e.target.value } } as Condition)} />
        </>
      )
    }

    if (op === 'InSegment' || op === 'NotInSegment') {
      const segId = (condition as Record<string, string>)[op]
      return (
        <SegmentPicker
          value={segId}
          envId={envId ?? null}
          orgId={orgId ?? ''}
          onChange={(newId) =>
            onChange({ [op]: newId } as Condition)
          }
        />
      )
    }

    if (op === 'FlagEvaluatedAs') {
      const payload = (condition as { FlagEvaluatedAs: { flag_id: string; variant_id: string } }).FlagEvaluatedAs
      return (
        <>
          <input className="input" style={{ width: 140 }} placeholder="flag ID" value={payload.flag_id}
            onChange={(e) => onChange({ FlagEvaluatedAs: { ...payload, flag_id: e.target.value } })} />
          <input className="input" style={{ width: 120 }} placeholder="variant ID" value={payload.variant_id}
            onChange={(e) => onChange({ FlagEvaluatedAs: { ...payload, variant_id: e.target.value } })} />
        </>
      )
    }

    return null
  }

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
      {iInput}
      {renderFields()}
      <button className="icon-btn" style={{ color: 'var(--danger)', flexShrink: 0 }} onClick={onDelete}>
        <I.x size={13} />
      </button>
    </div>
  )
}

// Export for convenience
export { ALL_OPS, OP_LABELS }
