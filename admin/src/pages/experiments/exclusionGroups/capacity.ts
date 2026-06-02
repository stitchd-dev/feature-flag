/**
 * Pure capacity helpers for the exclusion-group views (Phase 8 — xexp).
 *
 * Basis points ("bp") are integers in [0, 10000]; an exclusion group's
 * `allocated_bp + free_bp` sums to {@link TOTAL_BP} (10000 = 100%).
 *
 * Kept separate from the React components so the maths can be unit-tested in
 * isolation (see `ExclusionGroups.test.tsx`).
 */
import { TOTAL_BP, type ExclusionGroup } from '../../../lib/api/exclusionGroups'

/**
 * Allocated fraction of a group as a percentage (0–100), clamped.
 * Computed from `allocated_bp` so it stays correct even when a backend rounding
 * quirk makes `allocated + free` drift slightly off 10000.
 */
export function allocatedPercent(group: Pick<ExclusionGroup, 'allocated_bp'>): number {
  const pct = (group.allocated_bp / TOTAL_BP) * 100
  if (Number.isNaN(pct)) return 0
  return Math.min(100, Math.max(0, pct))
}

/** Free fraction of a group as a percentage (0–100), clamped. */
export function freePercent(group: Pick<ExclusionGroup, 'free_bp'>): number {
  const pct = (group.free_bp / TOTAL_BP) * 100
  if (Number.isNaN(pct)) return 0
  return Math.min(100, Math.max(0, pct))
}

/** Format a basis-point count as a human percentage string, e.g. `12.5%`. */
export function formatBpPercent(bp: number): string {
  const pct = (bp / TOTAL_BP) * 100
  // Drop a trailing ".0" so whole percentages read cleanly.
  return `${Number.isInteger(pct) ? pct : pct.toFixed(1)}%`
}

/**
 * Convert a UI traffic-allocation percentage (0–100) to basis points.
 * `traffic% × 100` per the assign-endpoint contract — e.g. 12.5% → 1250 bp.
 */
export function trafficPercentToBp(trafficPercent: number): number {
  return Math.round(trafficPercent * 100)
}

/**
 * True when a requested allocation (in bp) fits within a group's free budget.
 * Used by the create-experiment picker to warn/block over-allocation.
 */
export function fitsWithinFree(
  group: Pick<ExclusionGroup, 'free_bp'>,
  requestedBp: number,
): boolean {
  return requestedBp <= group.free_bp
}

/**
 * Validate a candidate experiment assignment against a group's free capacity.
 *
 * Returns `{ requestedBp, fits, message }` where:
 *   • `requestedBp` is the traffic% converted to basis points,
 *   • `fits` is true when the request fits within `free_bp`,
 *   • `message` is a human warning when it does not (else `null`).
 *
 * The create-experiment picker uses this to warn/block over-allocation before
 * calling the assign endpoint.
 */
export function exclusionGroupFitsCapacity(
  group: Pick<ExclusionGroup, 'free_bp'>,
  trafficPercent: number,
): { requestedBp: number; fits: boolean; message: string | null } {
  const requestedBp = trafficPercentToBp(trafficPercent)
  const fits = fitsWithinFree(group, requestedBp)
  return {
    requestedBp,
    fits,
    message: fits
      ? null
      : `Requested ${formatBpPercent(requestedBp)} exceeds the group's free capacity of ${formatBpPercent(group.free_bp)}.`,
  }
}

/**
 * Tally experiments per exclusion group → `{ [group_id]: count }`.
 * Experiments without an `exclusion_group_id` are ignored. Used to surface a
 * member count alongside each group's capacity view.
 */
export function countMembersByGroup(
  experiments: { exclusion_group_id?: string | null }[],
): Record<string, number> {
  const counts: Record<string, number> = {}
  for (const exp of experiments) {
    const gid = exp.exclusion_group_id
    if (!gid) continue
    counts[gid] = (counts[gid] ?? 0) + 1
  }
  return counts
}
