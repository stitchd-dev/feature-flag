/**
 * Yup schema for the Phase 10 Create/Edit experiment modal.
 *
 * Mirrors the gateway's experiment-create body shape introduced by the
 * `experimentation_full_20260521` track:
 *
 *   {
 *     name, key, description?,
 *     flag_id,
 *     flag_rule_id?  XOR  targets_default_rule,
 *     metric_ids: UUID[],            // primary; min 1
 *     guardrail_metric_ids: UUID[],  // optional; 0+
 *     unit_context_types: string[],  // min 1
 *     pre_period_days: int >= 0,     // 0 = CUPED off
 *     traffic_allocation: 0..100 (0.1 step),
 *     model
 *   }
 *
 * This is distinct from the legacy `experimentSchema.ts` (which only covered
 * the Phase 7 cutover from `primary_metric` → `metric_ids`). The legacy schema
 * stays in place for back-compat — the modal swaps to this richer one.
 *
 * Per the track learnings (`validateOnChange={false}` for async validators):
 * this schema is intentionally synchronous-only. No async UUID resolution or
 * RPC lookups here.
 */
import * as Yup from 'yup'

export const EXPERIMENT_MODELS = ['bayesian', 'frequentist'] as const
export type ExperimentModel = (typeof EXPERIMENT_MODELS)[number]

/** Maximum number of primary metrics permitted per experiment. */
export const MAX_METRIC_IDS = 5
/** Maximum number of guardrail metrics permitted per experiment. */
export const MAX_GUARDRAIL_METRIC_IDS = 5

/**
 * UUID-ish regex (any version). The gateway revalidates against the live
 * `metric_definitions` / `feature_flags` / `flag_rules` tables; the UI just
 * checks shape so we surface bad submits inline.
 */
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/**
 * Shape of the values the experiment form holds. Exported so `CreateExperimentModal`
 * + tests can share the type with the schema. `Yup.InferType` is intentionally
 * NOT used here because Yup's inference produces a partial / Maybe-of-T shape
 * for required-defaulted fields that doesn't align cleanly with the form's
 * runtime expectations.
 */
export interface ExperimentFormValues {
  name: string
  key: string
  description?: string
  flag_id: string
  /** Empty string when `targets_default_rule` is true (XOR). */
  flag_rule_id: string
  /** Mutually exclusive with `flag_rule_id`. */
  targets_default_rule: boolean
  metric_ids: string[]
  guardrail_metric_ids: string[]
  unit_context_types: string[]
  /** 0 = CUPED disabled. */
  pre_period_days: number
  /** 0–100 with 0.1 step. */
  traffic_allocation: number
  model: ExperimentModel
  /**
   * Optional mutual-exclusion group UUID. Empty string = ungrouped (the
   * default). When set, the modal calls the assign endpoint after create with
   * `requested_bp` derived from `traffic_allocation`.
   */
  exclusion_group_id?: string
}

/**
 * The Phase 10 schema.
 *
 * XOR validation lives on the `flag_rule_id` field rather than the parent
 * object — this lets Yup surface the error against a specific field so the
 * UI's per-field error banner targets the rule picker.
 */
export const experimentSchema: Yup.ObjectSchema<ExperimentFormValues> = Yup.object({
  name: Yup.string()
    .trim()
    .min(1, 'Name is required')
    .max(255, 'Name must be 255 characters or fewer')
    .required('Name is required'),

  key: Yup.string()
    .trim()
    .min(1, 'Key is required')
    .max(120, 'Key must be 120 characters or fewer')
    .matches(
      /^[a-z0-9][a-z0-9_-]*$/,
      'Key must be lowercase with letters, digits, hyphens, underscores',
    )
    .required('Key is required'),

  description: Yup.string().trim().max(500, 'Description must be 500 characters or fewer'),

  flag_id: Yup.string()
    .matches(UUID_RE, 'Flag is required')
    .required('Flag is required'),

  // XOR enforcement: exactly one of `flag_rule_id` (non-empty UUID) or
  // `targets_default_rule` (true) must be set. We hang the error on
  // `flag_rule_id` so the rule picker surfaces it inline.
  flag_rule_id: Yup.string()
    .defined()
    .test(
      'rule-or-default-xor',
      'Pick a percentage-rollout rule or the default rule',
      function (value) {
        const { targets_default_rule } = this.parent as ExperimentFormValues
        const hasRule = Boolean(value && value.trim())
        if (hasRule && targets_default_rule) {
          return this.createError({
            message: 'Cannot bind to both a rule and the default rule',
          })
        }
        if (!hasRule && !targets_default_rule) {
          return false // surfaces default message above
        }
        if (hasRule && !UUID_RE.test(value!)) {
          return this.createError({ message: 'Rule must be a valid UUID' })
        }
        return true
      },
    ),

  targets_default_rule: Yup.boolean().required().default(false),

  metric_ids: Yup.array()
    .of(
      Yup.string()
        .matches(UUID_RE, 'Metric ID must be a UUID')
        .required('Metric ID is required'),
    )
    .min(1, 'Pick at least 1 primary metric')
    .max(MAX_METRIC_IDS, `Pick at most ${MAX_METRIC_IDS} primary metrics`)
    .required('Pick at least 1 primary metric'),

  guardrail_metric_ids: Yup.array()
    .of(
      Yup.string()
        .matches(UUID_RE, 'Guardrail metric ID must be a UUID')
        .required(),
    )
    .max(MAX_GUARDRAIL_METRIC_IDS, `Pick at most ${MAX_GUARDRAIL_METRIC_IDS} guardrail metrics`)
    .default([])
    .required(),

  unit_context_types: Yup.array()
    .of(
      Yup.string()
        .trim()
        .min(1, 'Context type cannot be empty')
        .required(),
    )
    .min(1, 'Pick at least 1 context type')
    .test(
      'unique-context-types',
      'Context types must be unique',
      (arr) => {
        if (!arr) return true
        return new Set(arr.map((s) => s.trim())).size === arr.length
      },
    )
    .required(),

  pre_period_days: Yup.number()
    .integer('Pre-period days must be a whole number')
    .min(0, 'Pre-period days cannot be negative')
    .max(365, 'Pre-period days cannot exceed 365')
    .required()
    .default(0),

  traffic_allocation: Yup.number()
    .min(0, 'Traffic allocation must be >= 0')
    .max(100, 'Traffic allocation must be <= 100')
    .test(
      'tenth-precision',
      'Traffic allocation must be in 0.1 increments',
      (n) => {
        if (n == null) return true
        // Tolerate float drift — accept anything within 1e-6 of a 0.1 multiple.
        return Math.abs(n * 10 - Math.round(n * 10)) < 1e-6
      },
    )
    .required('Traffic allocation is required')
    .default(100),

  model: Yup.string()
    .oneOf(EXPERIMENT_MODELS as unknown as string[], 'Invalid model')
    .required('Model is required'),

  // Optional mutual-exclusion group. Empty string = ungrouped (default).
  // The "fits within free capacity" check is enforced at the field level by
  // the picker (`exclusionGroupFitsCapacity`) since it needs the live group's
  // `free_bp`, which the schema does not carry.
  exclusion_group_id: Yup.string()
    .trim()
    .test(
      'group-uuid-or-empty',
      'Exclusion group must be a valid UUID',
      (value) => !value || UUID_RE.test(value),
    ),
}) as unknown as Yup.ObjectSchema<ExperimentFormValues>
