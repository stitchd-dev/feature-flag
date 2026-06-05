/**
 * Pure helpers for the prerequisites editor (flag_lifecycle_20260604, Phase 8.3).
 *
 * The authoritative cycle check lives in the flag-service (a write returns
 * HTTP 400 naming the cycle path); these client-side helpers give a *live*
 * pre-save warning for the cycles we can detect locally (a self-edge and a
 * direct 2-cycle when the prerequisite flag, in turn, lists the edited flag).
 */
import type { Prerequisite, SetPrerequisitesBody } from '../../lib/types'

/** A prerequisite row as edited in the form. */
export interface PrereqRow {
  prerequisite_flag_key: string
  required_variant_key: string
}

/**
 * Detect cycles that are visible from the local data: a self-edge (the flag
 * naming itself) and a direct back-edge (a chosen prerequisite flag that itself
 * lists the edited flag as a prerequisite). `reverseDeps` maps a flag key → the
 * set of flag keys it already lists as prerequisites (sourced from `listFlags`).
 * Returns a human-readable cycle path, or null when no *locally visible* cycle
 * exists. (Deeper transitive cycles are caught server-side on save.)
 */
export function detectLocalCycle(
  flagKey: string,
  rows: PrereqRow[],
  reverseDeps: Record<string, string[]>,
): string | null {
  for (const row of rows) {
    const p = row.prerequisite_flag_key
    if (!p) continue
    if (p === flagKey) {
      return `${flagKey} → ${flagKey}`
    }
    // Direct 2-cycle: the chosen prerequisite already depends on this flag.
    const theirDeps = reverseDeps[p] ?? []
    if (theirDeps.includes(flagKey)) {
      return `${flagKey} → ${p} → ${flagKey}`
    }
  }
  return null
}

/** Build the PUT body from the form rows + fallback + the flag's version. */
export function toSetBody(
  rows: PrereqRow[],
  fallbackVariantKey: string,
  version: number,
): SetPrerequisitesBody {
  return {
    prerequisites: rows
      .filter((r) => r.prerequisite_flag_key && r.required_variant_key)
      .map<Prerequisite>((r) => ({
        prerequisite_flag_key: r.prerequisite_flag_key,
        required_variant_key: r.required_variant_key,
      })),
    fallback_variant_key: fallbackVariantKey,
    version,
  }
}

/**
 * Whether a 400 error message looks like a prerequisite-cycle rejection. The
 * flag-service stamps "prerequisite cycle detected: a -> b -> a".
 */
export function isCycleMessage(message: string): boolean {
  return /prerequisite cycle/i.test(message)
}
