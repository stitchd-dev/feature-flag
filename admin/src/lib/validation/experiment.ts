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
 *     sequential_testing_enabled: bool,     // opt-in; default false
 *     sequential_alpha: 0 < α < 1,          // default 0.05
 *     sequential_tau_squared?: > 0,         // omit = auto-derive
 *     sequential_min_sample_size: int >= 0, // default 100
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

// ── Bandit (FR7) ────────────────────────────────────────────────────────────

/** Experiment mode: a classic fixed-allocation A/B test, or an adaptive bandit. */
export const EXPERIMENT_MODES = ['fixed', 'bandit'] as const
export type ExperimentMode = (typeof EXPERIMENT_MODES)[number]

/** Bandit allocation algorithms (mirrors core `BanditAlgorithm`). */
export const BANDIT_ALGORITHMS = [
  'thompson',
  'epsilon_greedy',
  'ucb',
  'contextual',
] as const
export type BanditAlgorithm = (typeof BANDIT_ALGORITHMS)[number]

/** Propagation mode: static (per-tick rewrite) vs realtime (snapshot-resident). */
export const BANDIT_PROPAGATION_MODES = ['static', 'realtime'] as const
export type BanditPropagationMode = (typeof BANDIT_PROPAGATION_MODES)[number]

/** Lifecycle automation policy (mirrors core `LifecyclePolicy`). */
export const BANDIT_LIFECYCLE_POLICIES = [
  'advisory',
  'auto_commit',
  'auto_rollout',
] as const
export type BanditLifecyclePolicy = (typeof BANDIT_LIFECYCLE_POLICIES)[number]

/** Objective kind for the multi-objective builder. */
export const BANDIT_OBJECTIVE_KINDS = [
  'scalar',
  'scalarized',
  'constrained',
] as const
export type BanditObjectiveKind = (typeof BANDIT_OBJECTIVE_KINDS)[number]

/** Constraint direction for constrained objectives. */
export const BANDIT_CONSTRAINT_DIRECTIONS = ['gte', 'lte'] as const
export type BanditConstraintDirection =
  (typeof BANDIT_CONSTRAINT_DIRECTIONS)[number]

/** One metric+weight row for a scalarized objective. */
export interface BanditScalarizedWeight {
  metric_id: string
  weight: number
}

/** One guardrail constraint row for a constrained objective. */
export interface BanditConstraint {
  metric_id: string
  bound: number
  direction: BanditConstraintDirection
}

/**
 * Bandit configuration captured by the form. All fields are flat (Formik
 * doesn't love deeply-nested arrays of objects, but it copes with the shallow
 * rows here). The submit helper assembles these into the gateway's nested
 * `bandit_config` shape.
 */
export interface BanditConfigFormValues {
  algorithm: BanditAlgorithm
  propagation_mode: BanditPropagationMode
  /** Minimum exploration floor as a PERCENT (0–100); converted to bp on submit. */
  min_exploration_pct: number
  lifecycle_policy: BanditLifecyclePolicy
  /** Convergence probability threshold (0–1). */
  convergence_prob_threshold: number
  /** Contextual feature names (only meaningful when algorithm = contextual). */
  contextual_features: string[]
  // ── Objective builder ──
  objective_kind: BanditObjectiveKind
  /** Single metric for scalar; primary metric for constrained. */
  objective_metric_id: string
  /** Metric+weight rows for scalarized. */
  scalarized_weights: BanditScalarizedWeight[]
  /** Guardrail-constraint rows for constrained. */
  constraints: BanditConstraint[]
  // ── Optional autonomous campaign ──
  campaign_enabled: boolean
  campaign_max_iterations: number
  campaign_drift_threshold: number
}

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
  /**
   * Opt-in for sequential (always-valid) testing. Default false. The three
   * advanced knobs below are only required/validated meaningfully when this
   * is true (tau is always optional).
   */
  sequential_testing_enabled: boolean
  /** Target type-I error rate (α). Validate `> 0 && < 1`. Default 0.05. */
  sequential_alpha: number
  /**
   * Mixing variance (τ²) for the mSPRT mixture. Empty (undefined) = let the
   * service auto-derive. When provided, validate `> 0`.
   */
  sequential_tau_squared?: number | null
  /** Minimum per-variant sample size before a verdict. Integer `>= 0`. Default 100. */
  sequential_min_sample_size: number
  /** 0–100 with 0.1 step. */
  traffic_allocation: number
  model: ExperimentModel
  /**
   * Optional mutual-exclusion group UUID. Empty string = ungrouped (the
   * default). When set, the modal calls the assign endpoint after create with
   * `requested_bp` derived from `traffic_allocation`.
   */
  exclusion_group_id?: string
  /**
   * Experiment mode. `fixed` (default) = classic A/B; `bandit` = adaptive
   * allocation. When `bandit`, `bandit_config` is required + validated.
   */
  experiment_mode: ExperimentMode
  /** Bandit configuration; only meaningful + validated when mode = bandit. */
  bandit_config: BanditConfigFormValues
}

/** Sensible defaults for a fresh bandit config (mode flipped to bandit). */
export const DEFAULT_BANDIT_CONFIG: BanditConfigFormValues = {
  algorithm: 'thompson',
  propagation_mode: 'static',
  min_exploration_pct: 5,
  lifecycle_policy: 'advisory',
  convergence_prob_threshold: 0.95,
  contextual_features: [],
  objective_kind: 'scalar',
  objective_metric_id: '',
  scalarized_weights: [],
  constraints: [],
  campaign_enabled: false,
  campaign_max_iterations: 5,
  campaign_drift_threshold: 0.1,
}

/**
 * Bandit-config sub-schema, applied only when `experiment_mode === 'bandit'`.
 * Cross-field requireds reference sibling fields inside this object so they
 * resolve without external Yup context.
 */
export const banditConfigSchema = Yup.object({
  algorithm: Yup.string()
    .oneOf(BANDIT_ALGORITHMS as unknown as string[], 'Invalid algorithm')
    .required(),
  propagation_mode: Yup.string()
    .oneOf(
      BANDIT_PROPAGATION_MODES as unknown as string[],
      'Invalid propagation mode',
    )
    .required(),
  min_exploration_pct: Yup.number()
    .typeError('Exploration floor must be a number')
    .min(0, 'Exploration floor cannot be negative')
    // Floor is per-arm; cap at 50% so floor·n stays sane for small arm counts.
    .max(50, 'Exploration floor must be 50% or less')
    .required(),
  lifecycle_policy: Yup.string()
    .oneOf(
      BANDIT_LIFECYCLE_POLICIES as unknown as string[],
      'Invalid lifecycle policy',
    )
    .required(),
  convergence_prob_threshold: Yup.number()
    .typeError('Convergence threshold must be a number')
    .moreThan(0, 'Convergence threshold must be between 0 and 1')
    .lessThan(1, 'Convergence threshold must be between 0 and 1')
    .required(),
  contextual_features: Yup.array()
    .of(Yup.string().trim().min(1).required())
    // Contextual algorithm requires ≥1 feature; other algorithms ignore this.
    .when('algorithm', {
      is: 'contextual',
      then: (s) =>
        s.min(1, 'Contextual bandits need at least one feature').required(),
      otherwise: (s) => s.notRequired(),
    })
    .default([]),
  objective_kind: Yup.string()
    .oneOf(
      BANDIT_OBJECTIVE_KINDS as unknown as string[],
      'Invalid objective kind',
    )
    .required(),
  objective_metric_id: Yup.string()
    .when('objective_kind', {
      is: (kind: string) => kind === 'scalar' || kind === 'constrained',
      then: (s) =>
        s
          .matches(UUID_RE, 'Pick a metric for the objective')
          .required('Pick a metric for the objective'),
      otherwise: (s) => s.notRequired(),
    })
    .default(''),
  scalarized_weights: Yup.array()
    .of(
      Yup.object({
        metric_id: Yup.string()
          .matches(UUID_RE, 'Metric ID must be a UUID')
          .required(),
        weight: Yup.number()
          .typeError('Weight must be a number')
          .moreThan(0, 'Weight must be greater than 0')
          .required(),
      }),
    )
    .when('objective_kind', {
      is: 'scalarized',
      then: (s) => s.min(1, 'Add at least one weighted metric').required(),
      otherwise: (s) => s.notRequired(),
    })
    .default([]),
  constraints: Yup.array()
    .of(
      Yup.object({
        metric_id: Yup.string()
          .matches(UUID_RE, 'Metric ID must be a UUID')
          .required(),
        bound: Yup.number().typeError('Bound must be a number').required(),
        direction: Yup.string()
          .oneOf(BANDIT_CONSTRAINT_DIRECTIONS as unknown as string[])
          .required(),
      }),
    )
    .when('objective_kind', {
      is: 'constrained',
      then: (s) => s.min(1, 'Add at least one guardrail constraint').required(),
      otherwise: (s) => s.notRequired(),
    })
    .default([]),
  campaign_enabled: Yup.boolean().required().default(false),
  campaign_max_iterations: Yup.number()
    .typeError('Max iterations must be a number')
    .when('campaign_enabled', {
      is: true,
      then: (s) =>
        s
          .integer('Max iterations must be a whole number')
          .min(1, 'Max iterations must be at least 1')
          .required('Max iterations is required for a campaign'),
      otherwise: (s) => s.notRequired(),
    })
    .default(5),
  campaign_drift_threshold: Yup.number()
    .typeError('Drift threshold must be a number')
    .when('campaign_enabled', {
      is: true,
      then: (s) =>
        s
          .moreThan(0, 'Drift threshold must be between 0 and 1')
          .lessThan(1, 'Drift threshold must be between 0 and 1')
          .required('Drift threshold is required for a campaign'),
      otherwise: (s) => s.notRequired(),
    })
    .default(0.1),
})

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

  // ── Sequential (always-valid) testing ───────────────────────────────────
  // Opt-in toggle. The advanced knobs (alpha, min sample) are only validated
  // meaningfully when this is true; tau is always optional but, when present,
  // must be positive regardless of the toggle.
  sequential_testing_enabled: Yup.boolean().required().default(false),

  sequential_alpha: Yup.number()
    .typeError('α must be a number')
    // Blank input → fall back to the platform default so a disabled section
    // (or a cleared field) doesn't block submit.
    .transform((value, original) =>
      original === '' || original == null ? 0.05 : value,
    )
    .when('sequential_testing_enabled', {
      is: true,
      then: (s) =>
        s
          .moreThan(0, 'α must be between 0 and 1')
          .lessThan(1, 'α must be between 0 and 1')
          .required('α is required when sequential testing is enabled'),
      otherwise: (s) => s.notRequired(),
    })
    .default(0.05),

  sequential_tau_squared: Yup.number()
    .typeError('τ² must be a number')
    // Empty input means "auto-derive" → null (not NaN).
    .transform((value, original) =>
      original === '' || original == null ? null : value,
    )
    .nullable()
    // Always positive when a value is supplied, independent of the toggle.
    .moreThan(0, 'τ² must be greater than 0')
    .notRequired(),

  sequential_min_sample_size: Yup.number()
    .typeError('Minimum sample size must be a number')
    .transform((value, original) =>
      original === '' || original == null ? 100 : value,
    )
    .when('sequential_testing_enabled', {
      is: true,
      then: (s) =>
        s
          .integer('Minimum sample size must be a whole number')
          .min(0, 'Minimum sample size cannot be negative')
          .required('Minimum sample size is required when sequential testing is enabled'),
      otherwise: (s) => s.notRequired(),
    })
    .default(100),

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

  // ── Bandit mode + config ────────────────────────────────────────────────
  experiment_mode: Yup.string()
    .oneOf(EXPERIMENT_MODES as unknown as string[], 'Invalid experiment mode')
    .required()
    .default('fixed'),

  // The nested bandit config is validated only when mode = bandit; for a
  // `fixed` experiment the whole object is stripped (so an empty/partial config
  // never blocks submit). Intra-config conditionals (`algorithm`,
  // `objective_kind`, `campaign_enabled`) reference SIBLINGS within the object,
  // which Yup resolves natively without any external context.
  bandit_config: Yup.object()
    .when('experiment_mode', {
      is: 'bandit',
      then: () => banditConfigSchema,
      // Fixed mode: accept anything (the config is ignored on submit).
      otherwise: (s) => s.optional().nullable().strip(),
    })
    .default(DEFAULT_BANDIT_CONFIG),
}) as unknown as Yup.ObjectSchema<ExperimentFormValues>
