import { I } from '../icons'
import type { AllocationBucket } from '../../lib/ruleTypes'
import { allocationSum } from '../../lib/ruleTypes'

interface Props {
  buckets: AllocationBucket[]
  variants: string[]
  onChange: (buckets: AllocationBucket[]) => void
}

export function PercentageRolloutEditor({ buckets, variants, onChange }: Props) {
  const sum = allocationSum(buckets)
  const remaining = 1000 - sum

  function setVariantKey(i: number, key: string) {
    const next = buckets.map((b, j) => j === i ? { ...b, variant_key: key } : b)
    onChange(next)
  }

  function setWeight(i: number, pct: number) {
    const milli = Math.round(pct * 10)
    const next = buckets.map((b, j) => j === i ? { ...b, weight_milli: milli } : b)
    onChange(next)
  }

  function addRow() {
    const key = variants.find((v) => !buckets.some((b) => b.variant_key === v)) ?? (variants[0] ?? '')
    onChange([...buckets, { variant_key: key, weight_milli: 0 }])
  }

  function removeRow(i: number) {
    onChange(buckets.filter((_, j) => j !== i))
  }

  function distributeEvenly() {
    if (buckets.length === 0) return
    const each = Math.floor(1000 / buckets.length)
    const leftover = 1000 - each * buckets.length
    onChange(buckets.map((b, i) => ({ ...b, weight_milli: each + (i === 0 ? leftover : 0) })))
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
      {buckets.map((b, i) => (
        <div key={i} style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <select className="input" style={{ width: 120, fontFamily: 'var(--font-mono)', fontSize: 12 }}
            value={b.variant_key} onChange={(e) => setVariantKey(i, e.target.value)}>
            {variants.map((v) => <option key={v} value={v}>{v}</option>)}
          </select>
          <input
            type="number" min={0} max={100} step={0.1}
            className="input" style={{ width: 80, fontFamily: 'var(--font-mono)', textAlign: 'right' }}
            value={(b.weight_milli / 10).toFixed(1)}
            onChange={(e) => setWeight(i, parseFloat(e.target.value) || 0)}
          />
          <span style={{ fontSize: 12, color: 'var(--fg-muted)', width: 14 }}>%</span>
          <button className="icon-btn" style={{ color: 'var(--danger)' }} onClick={() => removeRow(i)}>
            <I.x size={12} />
          </button>
        </div>
      ))}

      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 4 }}>
        <button className="btn sm" onClick={addRow}><I.plus size={11} /> Add variant</button>
        {buckets.length > 1 && (
          <button className="btn sm" onClick={distributeEvenly}>Distribute evenly</button>
        )}
        <span style={{
          marginLeft: 'auto', fontSize: 11, fontFamily: 'var(--font-mono)',
          color: sum === 1000 ? 'var(--success)' : 'var(--danger)',
          fontWeight: 600,
        }}>
          {(sum / 10).toFixed(1)}% {sum !== 1000 && `(${remaining > 0 ? '+' : ''}${(remaining / 10).toFixed(1)}%)`}
        </span>
      </div>
    </div>
  )
}
