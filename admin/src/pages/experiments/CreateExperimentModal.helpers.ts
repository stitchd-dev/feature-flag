/**
 * Pure helpers for CreateExperimentModal — kept separate from the React
 * component so they can be unit-tested in isolation (see
 * `CreateExperimentModal.test.ts`).
 *
 * Phase 10 rewrites the modal to bind an experiment to either a specific
 * percentage-rollout rule on a flag OR to the flag's default-rule
 * percentage distribution. The helpers below are the pure functions that
 * drive the UI primitives:
 *
 *   • `filterEligibleRules` — given a flag's rules + its default-rule
 *     distribution, returns the picker options (each tagged as eligible
 *     or ineligible with a reason).
 *   • `buildExperimentCreateBody` — assembles the JSON body posted to
 *     `POST /v1/environments/{env_id}/experiments`.
 *   • `filterMetricOptions` / `metricKindChipStyle` — the metric-picker
 *     search + chip colours, identical to the Phase 7 helpers.
 */

import type { RuleJson, RolloutDistribution } from '../../lib/types'
import type { ExperimentFormValues } from '../../lib/validation/experiment'

export type MetricKind = 'aggregation' | 'ratio' | 'funnel'

export interface MetricPickerOption {
  id: string
  key: string
  name: string
  kind: MetricKind
}

/**
 * One row in the rule picker dropdown. The picker is fed by
 * `filterEligibleRules`: it computes eligibility once based on the rule's
 * output shape so the picker can disable ineligible rows.
 */
export interface RulePickerOption {
  /** Stable identifier for the row.
   *
   *   - `rule:<rule_id>` for a flag-rule entry.
   *   - `default_rule` for the synthetic default-rule entry.
   */
  id: string
  /** The rule UUID when `kind === 'rule'`; null for the default-rule entry. */
  rule_id: string | null
  /** Display label (the rule's `name`, falling back to "Rule N"). */
  label: string
  /** When `false`, the picker disables the row and shows `disabled_reason`. */
  eligible: boolean
  /** Tooltip surfaced on disabled rows. */
  disabled_reason?: string
  kind: 'rule' | 'default_rule'
}

// ─── Picker styling (unchanged from Phase 7) ────────────────────────────────

/**
 * Kind-chip colour map — mirrors `MetricsList.tsx::kindChipStyle` so the
 * picker looks consistent with the metrics list page.
 */
export function metricKindChipStyle(kind: MetricKind): {
  background: string
  color: string
  border: string
} {
  switch (kind) {
    case 'aggregation':
      return {
        background: 'rgba(59,130,246,0.1)',
        color: '#2563eb',
        border: '1px solid rgba(59,130,246,0.25)',
      }
    case 'ratio':
      return {
        background: 'rgba(168,85,247,0.1)',
        color: '#9333ea',
        border: '1px solid rgba(168,85,247,0.25)',
      }
    case 'funnel':
      return {
        background: 'rgba(34,197,94,0.1)',
        color: '#16a34a',
        border: '1px solid rgba(34,197,94,0.25)',
      }
  }
}

// ─── Metric picker search ───────────────────────────────────────────────────

/**
 * Filter the metric options for the picker dropdown.
 *
 * - Hides options whose `id` is in `selected` (already picked metrics
 *   shouldn't appear as picks).
 * - Performs a case-insensitive substring match on `name` or `key` when
 *   `query` is non-empty.
 * - Empty query returns all non-selected options.
 */
export function filterMetricOptions(
  options: MetricPickerOption[],
  query: string,
  selected: Set<string> = new Set(),
): MetricPickerOption[] {
  const q = query.trim().toLowerCase()
  return options.filter((opt) => {
    if (selected.has(opt.id)) return false
    if (!q) return true
    return (
      opt.name.toLowerCase().includes(q) || opt.key.toLowerCase().includes(q)
    )
  })
}

// ─── Rule picker eligibility ─────────────────────────────────────────────────

/**
 * Returns true when the rule's output is a percentage-distribution allocation
 * (and therefore eligible to host an experiment).
 *
 * The flag-engine output shapes:
 *   - `{ variant_key: "..." }`      → specific variant; INELIGIBLE
 *   - `{ allocation: { ... } }`     → percentage distribution; ELIGIBLE
 */
export function isPercentageRolloutRule(output: unknown): boolean {
  if (!output || typeof output !== 'object') return false
  const o = output as Record<string, unknown>
  if (!('allocation' in o)) return false
  const alloc = o.allocation as { buckets?: unknown }
  return Array.isArray(alloc?.buckets) && alloc.buckets.length > 0
}

/**
 * Builds the rule-picker option list for a flag.
 *
 * - All percentage-rollout rules surface as eligible rows.
 * - Specific-variant + segment-only rules surface as INELIGIBLE rows with
 *   a "requires a percentage rollout rule" tooltip — the picker disables them.
 * - When `defaultRuleDistribution` is non-null (i.e. the flag has been
 *   configured for default-rule experiments), a "Default rule (fallthrough)"
 *   entry is appended to the picker.
 *
 * Rules are surfaced in their array order. The default-rule entry is always
 * last so the picker presents rules first, then the catch-all.
 */
export function filterEligibleRules(
  rules: { rule_id: string; name?: string | null; output: unknown }[],
  defaultRuleDistribution: RolloutDistribution | null,
): RulePickerOption[] {
  const ruleOptions: RulePickerOption[] = rules.map((r, idx) => {
    const eligible = isPercentageRolloutRule(r.output)
    const label = r.name && r.name.trim() ? r.name : `Rule ${idx + 1}`
    return {
      id: `rule:${r.rule_id}`,
      rule_id: r.rule_id,
      label,
      eligible,
      disabled_reason: eligible
        ? undefined
        : 'Experiments require a percentage rollout rule.',
      kind: 'rule',
    }
  })

  if (defaultRuleDistribution !== null) {
    ruleOptions.push({
      id: 'default_rule',
      rule_id: null,
      label: 'Default rule (fallthrough)',
      eligible: true,
      kind: 'default_rule',
    })
  }
  return ruleOptions
}

// ─── Default-rule presence ─────────────────────────────────────────────────

/**
 * Returns true when the flag has a non-empty default-rule distribution
 * configured. The picker uses this to decide whether to surface the
 * "Default rule (fallthrough)" entry.
 *
 * Accepts the shape served by the gateway (`{ allocations: [...] }`) or
 * `null` / `undefined` (no distribution).
 */
export function hasDefaultRuleDistribution(
  d: RolloutDistribution | null | undefined,
): boolean {
  if (!d) return false
  return Array.isArray(d.allocations) && d.allocations.length > 0
}

// ─── Submit body ────────────────────────────────────────────────────────────

/**
 * Build the JSON body for `POST /v1/environments/{env}/experiments`.
 *
 * The Phase 10 wire shape:
 *   {
 *     environment_id, key, name, description?,
 *     flag_id,
 *     flag_rule_id?  XOR  targets_default_rule,
 *     metric_ids[], guardrail_metric_ids[],
 *     unit_context_types[],
 *     pre_period_days, traffic_allocation,
 *     model
 *   }
 *
 * `traffic_allocation` is converted from the UI's 0–100 percentage to the
 * gateway's 0–1 float. `flag_rule_id` is omitted when `targets_default_rule`
 * is true (XOR).
 */
export function buildExperimentCreateBody(
  values: ExperimentFormValues,
  environmentId: string,
): Record<string, unknown> {
  const desc = values.description?.trim()
  const body: Record<string, unknown> = {
    environment_id: environmentId,
    key: values.key.trim(),
    name: values.name.trim(),
    description: desc ? desc : undefined,
    flag_id: values.flag_id,
    metric_ids: values.metric_ids,
    guardrail_metric_ids: values.guardrail_metric_ids ?? [],
    unit_context_types: values.unit_context_types
      .map((s) => s.trim())
      .filter(Boolean),
    pre_period_days: Math.max(0, Math.floor(values.pre_period_days)),
    traffic_allocation: values.traffic_allocation / 100,
    model: values.model,
  }
  if (values.targets_default_rule) {
    body.targets_default_rule = true
  } else {
    body.flag_rule_id = values.flag_rule_id
  }
  return body
}

// ─── Edit mode body ─────────────────────────────────────────────────────────

/**
 * Build the JSON body for `PATCH /v1/environments/{env}/experiments/{key}`.
 *
 * Edit mode allows the same fields as create except for `key` and `flag_id`
 * (those are immutable post-create). `flag_rule_id` / `targets_default_rule`
 * remain swappable until the experiment leaves draft.
 */
export function buildExperimentPatchBody(
  values: ExperimentFormValues,
): Record<string, unknown> {
  const desc = values.description?.trim()
  const body: Record<string, unknown> = {
    name: values.name.trim(),
    description: desc ? desc : undefined,
    metric_ids: values.metric_ids,
    guardrail_metric_ids: values.guardrail_metric_ids ?? [],
    unit_context_types: values.unit_context_types
      .map((s) => s.trim())
      .filter(Boolean),
    pre_period_days: Math.max(0, Math.floor(values.pre_period_days)),
    traffic_allocation: values.traffic_allocation / 100,
    model: values.model,
  }
  if (values.targets_default_rule) {
    body.targets_default_rule = true
    body.flag_rule_id = null
  } else {
    body.targets_default_rule = false
    body.flag_rule_id = values.flag_rule_id
  }
  return body
}

// ─── Re-exports kept for back-compat ────────────────────────────────────────
//
// `ExperimentCreateFormValues` was the Phase 7 form-values type. The legacy
// helpers (back-compat tests + a couple of mock-data fixtures) still import
// it from this module. Re-exporting the Phase 10 shape under the old name
// keeps that import surface stable.

/** @deprecated Use `ExperimentFormValues` from `lib/validation/experiment`. */
export type ExperimentCreateFormValues = ExperimentFormValues

// Force inclusion of `RuleJson` re-export below so we keep the public surface
// of this module discoverable from the test file (which does not import
// `RuleJson` directly but uses its shape via the rule-picker helpers).
export type { RuleJson }
