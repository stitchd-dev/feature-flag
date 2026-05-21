import { useEffect } from 'react'

/**
 * ContextTypeTabs — switcher primitive for the experiment results page.
 *
 * Renders as a tab strip when there are 3 or fewer context types (compact,
 * eyeball-scannable). Switches to a dropdown when there are 4+ types so the
 * page header doesn't overflow on experiments configured with many
 * `unit_context_types`.
 *
 * Persists the active context type to `localStorage` when `storageKey` is
 * supplied — used by `ExperimentDetail.tsx` to preserve the selection
 * across page revisits with key shape `experiment_${experimentId}_ctx`.
 *
 * Phase 8 scope: the primitive is intentionally dumb — it has no opinion on
 * what counts as a valid context type. `ContextTypeContext` (the provider)
 * is the place to enforce the "active type must be in `contextTypes`"
 * invariant.
 */
interface Props {
  /** All context types available for the experiment. Order is preserved. */
  contextTypes: string[]
  /** The currently active context type. Should be a member of `contextTypes`. */
  activeContextType: string
  /** Called when the user picks a different context type. */
  onChange: (ct: string) => void
  /** Optional localStorage key for persistence. */
  storageKey?: string
}

export function ContextTypeTabs({
  contextTypes,
  activeContextType,
  onChange,
  storageKey,
}: Props) {
  // Persist the active type whenever it changes (when a storageKey is given).
  useEffect(() => {
    if (storageKey) {
      localStorage.setItem(storageKey, activeContextType)
    }
  }, [storageKey, activeContextType])

  if (contextTypes.length === 0) return null

  // Threshold: tab strip for ≤ 3 context types, dropdown for 4+.
  if (contextTypes.length >= 4) {
    return (
      <select
        value={activeContextType}
        onChange={(e) => onChange(e.target.value)}
        aria-label="Context type"
        className="ctx-tabs-select"
        style={{
          fontSize: 13,
          padding: '6px 10px',
          border: '1px solid var(--border-strong)',
          borderRadius: 6,
          background: 'var(--surface)',
          color: 'var(--fg)',
        }}
      >
        {contextTypes.map((ct) => (
          <option key={ct} value={ct}>
            {ct}
          </option>
        ))}
      </select>
    )
  }

  return (
    <div
      role="tablist"
      aria-label="Context type"
      className="ctx-tabs"
      style={{ display: 'inline-flex', gap: 4 }}
    >
      {contextTypes.map((ct) => {
        const active = ct === activeContextType
        return (
          <button
            key={ct}
            role="tab"
            type="button"
            aria-selected={active}
            className={active ? 'ctx-tab active' : 'ctx-tab'}
            onClick={() => onChange(ct)}
            style={{
              fontSize: 12,
              padding: '6px 12px',
              borderRadius: 6,
              border: active
                ? '1px solid var(--accent)'
                : '1px solid var(--border-strong)',
              background: active ? 'var(--surface)' : 'transparent',
              color: active ? 'var(--accent)' : 'var(--fg-muted)',
              fontWeight: active ? 600 : 500,
              cursor: 'pointer',
            }}
          >
            {ct}
          </button>
        )
      })}
    </div>
  )
}
