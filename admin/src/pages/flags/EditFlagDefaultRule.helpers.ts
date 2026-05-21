/**
 * Pure helpers for `EditFlagDefaultRule.tsx`.
 *
 * Phase 10 introduces a new flag-detail subsection that lets users configure
 * either:
 *   1. A "Single variant" default (today's behaviour) — the flag falls
 *      through to one fixed variant when no custom rule matches.
 *   2. A "Percentage distribution" default — the flag hashes the context
 *      key against an allocation table when no custom rule matches.
 *
 * Mode #2 is required for "default-rule experiments" (Phase 1 spec §2): the
 * experiment binds to the flag's default-rule path, and the percentage
 * distribution provides the variant allocation.
 *
 * These helpers are pure so they can be exhaustively unit-tested.
 */

import type { AdminFlagResponse, RolloutDistribution } from '../../lib/types'

/** One row in the percentage-distribution editor. */
export interface AllocationRow {
  variant_key: string
  /** Percentage in (0, 100]. UI uses 0.1 step granularity. */
  percentage: number
}

/** Sum-to-100 tolerance: floats accumulate small drift. */
const SUM_TOLERANCE = 0.01

/**
 * Validates a percentage-distribution allocation list.
 *
 * Rules:
 *   - At least one allocation.
 *   - Each `variant_key` is non-empty.
 *   - Each `percentage` is in `(0, 100]`.
 *   - No duplicate `variant_key`s.
 *   - Sum of percentages = 100 ± 0.01.
 *
 * Returns `null` when the allocations are valid, or the user-facing error
 * string for the first violation found.
 */
export function validateDistributionAllocations(
  allocations: AllocationRow[],
): string | null {
  if (!allocations || allocations.length === 0) {
    return 'Add at least one allocation row.'
  }

  const seen = new Set<string>()
  for (const row of allocations) {
    const variantKey = row.variant_key.trim()
    if (!variantKey) {
      return 'Each allocation must reference a variant.'
    }
    if (!Number.isFinite(row.percentage)) {
      return `Allocation for "${variantKey}" must be a number.`
    }
    if (row.percentage <= 0) {
      return `Allocation for "${variantKey}" must be greater than 0.`
    }
    if (row.percentage > 100) {
      return `Allocation for "${variantKey}" must be 100 or less.`
    }
    if (seen.has(variantKey)) {
      return `Allocations must be unique by variant — "${variantKey}" appears twice.`
    }
    seen.add(variantKey)
  }

  const sum = allocations.reduce((acc, r) => acc + (r.percentage || 0), 0)
  if (Math.abs(sum - 100) > SUM_TOLERANCE) {
    return `Percentages must sum to 100 (currently ${sum.toFixed(2)}).`
  }
  return null
}

/**
 * Returns true when the flag has a non-empty default-rule distribution
 * configured. Used by the UI to decide whether to default the form into
 * "Percentage distribution" mode on first render.
 */
export function flagHasDefaultRuleDistribution(
  flag: { default_rule_distribution?: RolloutDistribution | null } | null | undefined,
): boolean {
  if (!flag) return false
  const d = flag.default_rule_distribution
  if (!d) return false
  return Array.isArray(d.allocations) && d.allocations.length > 0
}

/** Build the request body for `PUT /v1/projects/{project}/flags/{key}`
 *  (single-variant default branch). */
export function buildSingleVariantPayload(
  variantKey: string,
  version: number,
): Record<string, unknown> {
  return {
    default_variant_key: variantKey,
    /** Explicitly clearing the distribution so subsequent rules read the
     *  single-variant fallthrough. */
    default_rule_distribution: null,
    version,
  }
}

/** Build the request body for `POST /v1/projects/{project}/flags/{key}/default-rule-distribution`.
 *  The gateway derives `default_variant_id = null` server-side. */
export function buildDistributionPayload(
  allocations: AllocationRow[],
): RolloutDistribution {
  return {
    allocations: allocations.map((row) => ({
      variant_key: row.variant_key.trim(),
      percentage: row.percentage,
    })),
  }
}

/**
 * Picks the default mode for the editor given the current flag state.
 *
 *   - "percentage" when the flag already has a non-empty distribution.
 *   - "single_variant" otherwise.
 *
 * Lifted into a helper so the React component can call it deterministically
 * during `useState` initialisation (and so it's unit-testable).
 */
export function pickDefaultMode(
  flag: AdminFlagResponse & { default_rule_distribution?: RolloutDistribution | null },
): 'single_variant' | 'percentage' {
  return flagHasDefaultRuleDistribution(flag) ? 'percentage' : 'single_variant'
}

/**
 * Initial allocation rows for "Percentage distribution" mode given a flag.
 *
 *   - When the flag already has a distribution, seed from it.
 *   - Otherwise, seed an equal split across the flag's variants.
 */
export function seedAllocations(
  flag: AdminFlagResponse & { default_rule_distribution?: RolloutDistribution | null },
): AllocationRow[] {
  if (flagHasDefaultRuleDistribution(flag) && flag.default_rule_distribution) {
    return flag.default_rule_distribution.allocations.map((a) => ({
      variant_key: a.variant_key,
      percentage: a.percentage,
    }))
  }
  const n = flag.variants.length || 2
  const equal = Math.floor((100 / n) * 10) / 10
  const leftover = +(100 - equal * n).toFixed(2)
  return flag.variants.map((v, i) => ({
    variant_key: v.key,
    percentage: i === 0 ? equal + leftover : equal,
  }))
}
