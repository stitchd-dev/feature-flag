import { I } from '../icons'
import type { AllocationOutput, HashTarget } from '../../lib/ruleTypes'
import { allocationSum } from '../../lib/ruleTypes'

interface Props {
  value: AllocationOutput
  variants: string[]
  onChange: (value: AllocationOutput) => void
}

// ─── HashTargetsEditor ────────────────────────────────────────────────────────

function HashTargetsEditor({
  targets, onChange,
}: {
  targets: HashTarget[]
  onChange: (targets: HashTarget[]) => void
}) {
  function addTarget() {
    onChange([...targets, { context_type: 'user', field: 'key' }])
  }

  function removeTarget(i: number) {
    onChange(targets.filter((_, j) => j !== i))
  }

  function setContextType(i: number, val: string) {
    onChange(targets.map((t, j) => j === i ? { ...t, context_type: val } : t))
  }

  function setField(i: number, val: string) {
    onChange(targets.map((t, j) => j === i ? { ...t, field: val } : t))
  }

  return (
    <div style={{ marginBottom: 12 }}>
      <div style={{ fontSize: 11, fontWeight: 600, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.06em', marginBottom: 6 }}>
        Hash by (determines bucket stickiness)
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
        {targets.map((t, i) => (
          <div key={i} style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            {/* context type */}
            <input
              className="input"
              list="ctx-types"
              placeholder="context type"
              value={t.context_type}
              onChange={(e) => setContextType(i, e.target.value)}
              style={{ width: 90, fontFamily: 'var(--font-mono)', fontSize: 12 }}
            />
            <datalist id="ctx-types">
              <option value="user" />
              <option value="org" />
              <option value="device" />
              <option value="session" />
            </datalist>

            <span style={{ fontSize: 11, color: 'var(--fg-subtle)' }}>.</span>

            {/* field: "key" or param name */}
            <div style={{ display: 'flex', alignItems: 'center', gap: 4, flex: 1 }}>
              <label style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: 12, cursor: 'pointer', whiteSpace: 'nowrap' }}>
                <input
                  type="radio"
                  checked={t.field === 'key'}
                  onChange={() => setField(i, 'key')}
                />
                key
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: 12, cursor: 'pointer', whiteSpace: 'nowrap' }}>
                <input
                  type="radio"
                  checked={t.field !== 'key'}
                  onChange={() => setField(i, '')}
                />
                param:
              </label>
              {t.field !== 'key' && (
                <input
                  className="input"
                  placeholder="param name"
                  value={t.field}
                  onChange={(e) => setField(i, e.target.value)}
                  style={{ width: 110, fontFamily: 'var(--font-mono)', fontSize: 12 }}
                  autoFocus
                />
              )}
            </div>

            <button
              className="icon-btn"
              style={{ color: targets.length <= 1 ? 'var(--fg-faint)' : 'var(--danger)' }}
              disabled={targets.length <= 1}
              onClick={() => removeTarget(i)}
              title="Remove hash target"
            >
              <I.x size={12} />
            </button>
          </div>
        ))}
      </div>
      <button className="btn sm" style={{ marginTop: 6 }} onClick={addTarget}>
        <I.plus size={11} /> Add hash input
      </button>
    </div>
  )
}

// ─── PercentageRolloutEditor ──────────────────────────────────────────────────

export function PercentageRolloutEditor({ value, variants, onChange }: Props) {
  const { hash_targets, buckets } = value
  const sum = allocationSum(buckets)
  const remaining = 1000 - sum

  function setVariantKey(i: number, key: string) {
    const next = buckets.map((b, j) => j === i ? { ...b, variant_key: key } : b)
    onChange({ ...value, buckets: next })
  }

  function setWeight(i: number, pct: number) {
    const milli = Math.round(pct * 10)
    const next = buckets.map((b, j) => j === i ? { ...b, weight_milli: milli } : b)
    onChange({ ...value, buckets: next })
  }

  function addRow() {
    const key = variants.find((v) => !buckets.some((b) => b.variant_key === v)) ?? (variants[0] ?? '')
    onChange({ ...value, buckets: [...buckets, { variant_key: key, weight_milli: 0 }] })
  }

  function removeRow(i: number) {
    onChange({ ...value, buckets: buckets.filter((_, j) => j !== i) })
  }

  function distributeEvenly() {
    if (buckets.length === 0) return
    const each = Math.floor(1000 / buckets.length)
    const leftover = 1000 - each * buckets.length
    onChange({ ...value, buckets: buckets.map((b, i) => ({ ...b, weight_milli: each + (i === 0 ? leftover : 0) })) })
  }

  return (
    <div>
      <HashTargetsEditor
        targets={hash_targets}
        onChange={(t) => onChange({ ...value, hash_targets: t })}
      />

      <div style={{ fontSize: 11, fontWeight: 600, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.06em', marginBottom: 6 }}>
        Variant weights
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        {buckets.map((b, i) => (
          <div key={i} style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <select
              className="input"
              style={{ width: 120, fontFamily: 'var(--font-mono)', fontSize: 12 }}
              value={b.variant_key}
              onChange={(e) => setVariantKey(i, e.target.value)}
            >
              {variants.map((v) => <option key={v} value={v}>{v}</option>)}
            </select>
            <input
              type="number" min={0} max={100} step={0.1}
              className="input"
              style={{ width: 80, fontFamily: 'var(--font-mono)', textAlign: 'right' }}
              value={(b.weight_milli / 10).toFixed(1)}
              onChange={(e) => setWeight(i, parseFloat(e.target.value) || 0)}
            />
            <span style={{ fontSize: 12, color: 'var(--fg-muted)', width: 14 }}>%</span>
            <button className="icon-btn" style={{ color: 'var(--danger)' }} onClick={() => removeRow(i)}>
              <I.x size={12} />
            </button>
          </div>
        ))}
      </div>

      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 8 }}>
        <button className="btn sm" onClick={addRow}><I.plus size={11} /> Add variant</button>
        {buckets.length > 1 && (
          <button className="btn sm" onClick={distributeEvenly}>Distribute evenly</button>
        )}
        <span style={{
          marginLeft: 'auto', fontSize: 11, fontFamily: 'var(--font-mono)',
          color: sum === 1000 ? 'var(--success)' : 'var(--danger)',
          fontWeight: 600,
        }}>
          {(sum / 10).toFixed(1)}%{sum !== 1000 && ` (${remaining > 0 ? '+' : ''}${(remaining / 10).toFixed(1)}%)`}
        </span>
      </div>
    </div>
  )
}
