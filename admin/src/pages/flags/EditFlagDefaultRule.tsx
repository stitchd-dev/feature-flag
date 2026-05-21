/**
 * Phase 10 EditFlagDefaultRule — flag editor "Default rule" subsection.
 *
 * Renders inside `FlagDetail.tsx` (Variants tab or alongside Targeting). Lets
 * the user configure the flag's default-rule fallthrough as either:
 *
 *   1. "Single variant" — today's default (sets `default_variant_key`).
 *   2. "Percentage distribution" — new in Phase 1; required for default-rule
 *      experiments. Sets `default_rule_distribution`.
 *
 * Locked when an active experiment is bound to the flag — surfaced via the
 * gateway's 409 `FLAG_LOCKED_BY_EXPERIMENT` response. We surface the lock
 * either from the flag-load response (when the gateway eventually adds
 * `locked_by_experiment_id`) or by attempting the save and catching the
 * 409 — whichever fires first.
 */
import { useState } from 'react'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { api, setDefaultRuleDistribution } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import type {
  AdminFlagResponse,
  RolloutDistribution,
} from '../../lib/types'
import {
  validateDistributionAllocations,
  buildSingleVariantPayload,
  pickDefaultMode,
  seedAllocations,
  type AllocationRow,
} from './EditFlagDefaultRule.helpers'

/** Flag shape extended with the Phase 1 default-rule fields. */
interface FlagWithDefaultRule extends AdminFlagResponse {
  default_rule_distribution?: RolloutDistribution | null
  /** Phase 11 cleanup: the gateway will surface this on flag GET. Until then
   *  we derive lock state from 409 attempts. */
  locked_by_experiment_id?: string | null
}

interface Props {
  flag: FlagWithDefaultRule
  canWrite: boolean
  onSaved: (updated: AdminFlagResponse) => void
  onConflict: () => void
}

type Mode = 'single_variant' | 'percentage'

export function EditFlagDefaultRule({ flag, canWrite, onSaved, onConflict }: Props) {
  const { projectId } = useOrgContext()
  const [mode, setMode] = useState<Mode>(() => pickDefaultMode(flag))
  const [singleVariantKey, setSingleVariantKey] = useState<string>(
    flag.default_variant_key ?? flag.variants[0]?.key ?? '',
  )
  const [allocations, setAllocations] = useState<AllocationRow[]>(() =>
    seedAllocations(flag),
  )
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [lockedByExperiment, setLockedByExperiment] = useState<string | null>(
    flag.locked_by_experiment_id ?? null,
  )

  const locked = Boolean(lockedByExperiment)
  const interactive = canWrite && !locked

  // ── Allocation editing helpers ──────────────────────────────────────────

  function setRow(index: number, patch: Partial<AllocationRow>) {
    setAllocations((rows) =>
      rows.map((r, i) => (i === index ? { ...r, ...patch } : r)),
    )
  }

  function addRow() {
    const used = new Set(allocations.map((r) => r.variant_key))
    const next = flag.variants.find((v) => !used.has(v.key))
    setAllocations((rows) => [
      ...rows,
      { variant_key: next?.key ?? '', percentage: 0 },
    ])
  }

  function removeRow(index: number) {
    setAllocations((rows) => rows.filter((_, i) => i !== index))
  }

  // ── Save ────────────────────────────────────────────────────────────────

  async function save() {
    if (!projectId || !canWrite) return
    setSaving(true)
    setError(null)
    try {
      if (mode === 'single_variant') {
        const body = buildSingleVariantPayload(singleVariantKey, flag.version)
        const { data } = await api.put<AdminFlagResponse>(
          `/v1/projects/${projectId}/flags/${flag.key}`,
          body,
        )
        // Reseed allocations with the saved variant for symmetry on mode-switch.
        onSaved(data)
      } else {
        const err = validateDistributionAllocations(allocations)
        if (err) {
          setError(err)
          setSaving(false)
          return
        }
        await setDefaultRuleDistribution(projectId, flag.key, {
          allocations: allocations.map((r) => ({
            variant_key: r.variant_key.trim(),
            percentage: r.percentage,
          })),
        })
        // The endpoint returns 204; refetch the flag to surface the updated
        // shape upstream.
        const { data } = await api.get<AdminFlagResponse>(
          `/v1/projects/${projectId}/flags/${flag.key}`,
        )
        onSaved(data)
      }
    } catch (e: unknown) {
      const status = (e as { response?: { status?: number; data?: { experiment_id?: string } } })
        ?.response
      if (status?.status === 409) {
        const expId = status.data?.experiment_id ?? 'unknown'
        setLockedByExperiment(expId)
        setError(`Locked by experiment: ${expId}`)
        onConflict()
        return
      }
      setError(extractErrorMessage(e))
    } finally {
      setSaving(false)
    }
  }

  // ── Render ─────────────────────────────────────────────────────────────

  const total = allocations.reduce((s, r) => s + (r.percentage || 0), 0)
  const totalOk = Math.abs(total - 100) < 0.01

  return (
    <div className="card" data-section="default-rule-editor">
      <div className="card-header">
        <div className="card-title">
          <I.toggle size={14} /> Default rule
        </div>
        {interactive && (
          <div style={{ display: 'flex', gap: 6 }}>
            <button
              className="btn primary sm"
              disabled={saving}
              onClick={save}
              type="button"
            >
              {saving ? 'Saving…' : 'Save'}
            </button>
          </div>
        )}
      </div>
      <div style={{ padding: 14 }}>
        {locked && (
          <div
            role="alert"
            data-locked-by-experiment={lockedByExperiment ?? ''}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              padding: '8px 12px',
              background: 'var(--bg-sunken)',
              border: '1px solid var(--border)',
              borderRadius: 6,
              fontSize: 12,
              color: 'var(--fg-muted)',
              marginBottom: 12,
            }}
          >
            <I.lock size={12} /> Locked by experiment:{' '}
            <code style={{ fontFamily: 'var(--font-mono)', fontSize: 11 }}>
              {lockedByExperiment}
            </code>
            <span style={{ marginLeft: 'auto' }}>
              Stop the experiment to edit.
            </span>
          </div>
        )}

        {error && !locked && (
          <div
            role="alert"
            style={{
              padding: '8px 12px',
              background: 'var(--danger-bg)',
              border: '1px solid rgba(196,43,28,0.3)',
              borderRadius: 6,
              color: 'var(--danger)',
              fontSize: 12,
              marginBottom: 12,
            }}
          >
            {error}
          </div>
        )}

        {/* Mode toggle ─ radio group for keyboard a11y. */}
        <div
          role="radiogroup"
          aria-label="Default rule mode"
          style={{
            display: 'flex',
            gap: 6,
            padding: 3,
            background: 'var(--bg-sunken)',
            border: '1px solid var(--border)',
            borderRadius: 6,
            marginBottom: 12,
            width: 'fit-content',
            opacity: locked ? 0.35 : 1,
          }}
        >
          <button
            type="button"
            role="radio"
            aria-checked={mode === 'single_variant'}
            disabled={!interactive}
            onClick={() => setMode('single_variant')}
            style={{
              padding: '4px 10px',
              border: 'none',
              background:
                mode === 'single_variant' ? 'var(--surface)' : 'transparent',
              color: mode === 'single_variant' ? 'var(--fg)' : 'var(--fg-muted)',
              fontWeight: mode === 'single_variant' ? 600 : 500,
              fontSize: 12,
              borderRadius: 4,
              cursor: interactive ? 'pointer' : 'not-allowed',
            }}
          >
            Single variant
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={mode === 'percentage'}
            disabled={!interactive}
            onClick={() => setMode('percentage')}
            style={{
              padding: '4px 10px',
              border: 'none',
              background:
                mode === 'percentage' ? 'var(--surface)' : 'transparent',
              color: mode === 'percentage' ? 'var(--fg)' : 'var(--fg-muted)',
              fontWeight: mode === 'percentage' ? 600 : 500,
              fontSize: 12,
              borderRadius: 4,
              cursor: interactive ? 'pointer' : 'not-allowed',
            }}
          >
            Percentage distribution
          </button>
        </div>

        {mode === 'single_variant' && (
          <div style={{ opacity: locked ? 0.35 : 1 }}>
            <label
              className="label"
              htmlFor="default-variant-key"
              style={{ display: 'block', marginBottom: 4 }}
            >
              Default variant
            </label>
            <select
              id="default-variant-key"
              className="input"
              disabled={!interactive}
              value={singleVariantKey}
              onChange={(e) => setSingleVariantKey(e.target.value)}
              style={{ minWidth: 200 }}
            >
              {flag.variants.map((v) => (
                <option key={v.key} value={v.key}>
                  {v.key}
                </option>
              ))}
            </select>
            <div
              style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 6 }}
            >
              The flag falls through to this variant when no custom rule matches.
            </div>
          </div>
        )}

        {mode === 'percentage' && (
          <div style={{ opacity: locked ? 0.35 : 1 }}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {allocations.map((row, i) => (
                <div
                  key={i}
                  style={{ display: 'flex', gap: 8, alignItems: 'center' }}
                >
                  <select
                    aria-label={`Variant for allocation ${i + 1}`}
                    className="input"
                    style={{
                      flex: 1,
                      fontFamily: 'var(--font-mono)',
                      fontSize: 12,
                    }}
                    disabled={!interactive}
                    value={row.variant_key}
                    onChange={(e) => setRow(i, { variant_key: e.target.value })}
                  >
                    <option value="">— pick variant —</option>
                    {flag.variants.map((v) => (
                      <option key={v.key} value={v.key}>
                        {v.key}
                      </option>
                    ))}
                  </select>
                  <input
                    aria-label={`Percentage for allocation ${i + 1}`}
                    className="input"
                    type="number"
                    min={0}
                    max={100}
                    step={0.1}
                    style={{ width: 90, fontFamily: 'var(--font-mono)' }}
                    disabled={!interactive}
                    value={row.percentage}
                    onChange={(e) =>
                      setRow(i, { percentage: parseFloat(e.target.value) || 0 })
                    }
                  />
                  <span style={{ fontSize: 12, color: 'var(--fg-muted)' }}>%</span>
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label={`Remove allocation ${i + 1}`}
                    disabled={!interactive || allocations.length <= 1}
                    onClick={() => removeRow(i)}
                    style={{
                      color:
                        allocations.length <= 1
                          ? 'var(--fg-faint)'
                          : 'var(--danger)',
                    }}
                  >
                    <I.x size={12} />
                  </button>
                </div>
              ))}
            </div>
            <div
              style={{
                display: 'flex',
                justifyContent: 'space-between',
                alignItems: 'center',
                marginTop: 8,
              }}
            >
              <button
                type="button"
                className="btn sm"
                disabled={!interactive}
                onClick={addRow}
              >
                <I.plus size={11} /> Add allocation
              </button>
              <div
                style={{
                  fontSize: 11,
                  color: totalOk ? 'var(--success)' : 'var(--danger)',
                  fontFamily: 'var(--font-mono)',
                }}
              >
                Total: {total.toFixed(1)}%{' '}
                {totalOk ? '✓' : '— must equal 100%'}
              </div>
            </div>
            <div
              style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 6 }}
            >
              Required for default-rule experiments. The hash of the context
              key buckets users into a variant.
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
