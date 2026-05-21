import { createContext, useContext, useState, useEffect } from 'react'
import type { ReactNode } from 'react'

/**
 * ContextTypeContext — per-experiment provider that owns the active
 * context-type selection.
 *
 * Phase 8.3 lifts the active-context-type state out of `ExperimentDetail.tsx`
 * (where Task 8.2 parked it as a local `useState`) so that downstream
 * components — variant tables, exposures list, timeseries chart — can read
 * + write the same value without prop-drilling.
 *
 * Persistence: the selection is mirrored to `localStorage` under
 * `experiment_${experimentId}_ctx`. The provider:
 *   • Reads `localStorage` on mount and uses the stored value when it's in
 *     `contextTypes`. Otherwise falls back to `contextTypes[0]` (or `'user'`
 *     when no types are supplied — should never happen in practice but keeps
 *     the provider crash-safe).
 *   • Writes back on every change.
 *
 * The hook `useActiveContextType()` throws when used outside the provider —
 * matches the pattern in `OrgContext.tsx`.
 */
interface ContextTypeContextValue {
  /** All context types available for the bound experiment, in display order. */
  contextTypes: string[]
  /** Currently active context type (always a member of `contextTypes`). */
  activeContextType: string
  /** Update the active context type — also writes to localStorage. */
  setActiveContextType: (ct: string) => void
}

const Ctx = createContext<ContextTypeContextValue | null>(null)

interface ProviderProps {
  children: ReactNode
  contextTypes: string[]
  experimentId: string
}

/**
 * Computes the localStorage key the provider uses for a given experimentId.
 * Exported so callers can keep their persistence keys aligned (used by
 * `ContextTypeTabs.storageKey` for the dual-write path during the Task 8.3
 * transition).
 */
export function contextTypeStorageKey(experimentId: string): string {
  return `experiment_${experimentId}_ctx`
}

export function ContextTypeProvider({
  children,
  contextTypes,
  experimentId,
}: ProviderProps) {
  const storageKey = contextTypeStorageKey(experimentId)
  const initial = readInitial(contextTypes, storageKey)
  const [active, setActive] = useState<string>(initial)

  // Re-sync when contextTypes changes (e.g. the page initially renders before
  // results load, then re-renders with the real list). If the current active
  // value drops out of the list, bump to the first available.
  useEffect(() => {
    if (contextTypes.length === 0) return
    if (!contextTypes.includes(active)) {
      setActive(contextTypes[0])
    }
  }, [contextTypes, active])

  // Persist on every change.
  useEffect(() => {
    if (typeof window === 'undefined') return
    try {
      localStorage.setItem(storageKey, active)
    } catch {
      // Best-effort — quotas / disabled storage are non-fatal.
    }
  }, [storageKey, active])

  return (
    <Ctx.Provider
      value={{
        contextTypes,
        activeContextType: active,
        setActiveContextType: setActive,
      }}
    >
      {children}
    </Ctx.Provider>
  )
}

export function useActiveContextType(): ContextTypeContextValue {
  const v = useContext(Ctx)
  if (!v) {
    throw new Error(
      'useActiveContextType must be used inside ContextTypeProvider',
    )
  }
  return v
}

// ── internals ────────────────────────────────────────────────────────────────

function readInitial(contextTypes: string[], storageKey: string): string {
  const fallback = contextTypes[0] ?? 'user'
  if (typeof window === 'undefined') return fallback
  try {
    const saved = localStorage.getItem(storageKey)
    if (saved && contextTypes.includes(saved)) return saved
  } catch {
    // Storage disabled — fall through.
  }
  return fallback
}
