import * as Yup from 'yup'

// ─── Constants ───────────────────────────────────────────────────────────────

export const METRIC_KINDS = ['aggregation', 'ratio', 'funnel'] as const
export type MetricKind = typeof METRIC_KINDS[number]

export const AGGREGATORS = ['count', 'sum', 'avg', 'p50', 'p90', 'p99', 'uniq'] as const
export type Aggregator = typeof AGGREGATORS[number]

export const GOAL_DIRECTIONS = ['increase', 'decrease', 'neutral'] as const
export type GoalDirection = typeof GOAL_DIRECTIONS[number]

/**
 * `count` ignores `on_field`; everything else needs a field reference.
 * Mirrors `AggregationOperator::requires_field()` in `stitchd-core`.
 */
export function aggregatorRequiresField(agg: string): boolean {
  return agg !== 'count' && agg !== ''
}

/**
 * Parse a where-clause textarea (raw JSON). Returns `{ ok, value, error }`.
 * An empty / whitespace-only string maps to `value: undefined` (cleared).
 * Mirrors how the gateway treats `where_clause: null`.
 */
export function parseWhereClause(
  raw: string,
): { ok: true; value: unknown | undefined } | { ok: false; error: string } {
  const trimmed = raw.trim()
  if (trimmed === '') return { ok: true, value: undefined }
  try {
    return { ok: true, value: JSON.parse(trimmed) as unknown }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) }
  }
}

// ─── Form values ─────────────────────────────────────────────────────────────

/** One funnel step row in the form (where_clause stored as raw JSON string). */
export interface MetricStepValues {
  event_key: string
  where_clause: string
}

/**
 * Form values for the create / edit metric form.
 *
 * All fields are present regardless of kind so Formik state survives kind
 * switches — the discriminated Yup schema below ignores fields that aren't
 * relevant for the active kind via `Yup.when('kind', ...)`.
 */
export interface MetricFormValues {
  kind: MetricKind | ''
  /** Required on create, ignored on edit (server-immutable). */
  key: string
  name: string
  description: string
  goal_direction: GoalDirection

  // Aggregation
  event_key: string
  aggregator: Aggregator | ''
  on_field: string
  where_clause: string

  // Ratio
  numerator_metric_id: string
  denominator_metric_id: string
  /** Stored as string in the form for clean numeric input handling. */
  min_denominator: string

  // Funnel
  steps: MetricStepValues[]
  window_seconds: string
  count_repeats: boolean
}

export const initialMetricValues: MetricFormValues = {
  kind: '',
  key: '',
  name: '',
  description: '',
  goal_direction: 'increase',
  event_key: '',
  aggregator: 'count',
  on_field: '',
  where_clause: '',
  numerator_metric_id: '',
  denominator_metric_id: '',
  min_denominator: '50',
  steps: [
    { event_key: '', where_clause: '' },
    { event_key: '', where_clause: '' },
  ],
  window_seconds: '86400',
  count_repeats: false,
}

// ─── Schema ──────────────────────────────────────────────────────────────────

/**
 * Yup schema for the metric form. Implements the discriminated-union
 * pattern from `conductor/patterns.md` — kind-specific fields are
 * conditionally required via `Yup.when('kind', ...)`.
 *
 * `where_clause` strings are validated via a custom `.test` that accepts
 * empty or valid JSON (consumers should parse with `parseWhereClause`
 * before sending to the API).
 *
 * Note: this is the EDIT-mode schema — `key` is optional (the field is
 * read-only on edit). Use [`metricCreateSchema`] when creating new metrics.
 */
export const metricSchema = Yup.object({
  kind: Yup.string()
    .oneOf(METRIC_KINDS as unknown as string[], 'Choose a metric kind')
    .required('Metric kind is required'),

  // Optional in the base schema — `metricCreateSchema` makes it required.
  key: Yup.string()
    .trim()
    .max(120, 'Key must be 120 characters or fewer')
    .test(
      'key-format',
      'Key must start with a letter/digit and contain only lowercase letters, digits, hyphens, underscores',
      (val) => val == null || val.trim() === '' || /^[a-z0-9][a-z0-9_-]*$/.test(val),
    ),

  name: Yup.string()
    .trim()
    .min(1, 'Name is required')
    .max(120, 'Name must be 120 characters or fewer')
    .required('Name is required'),

  description: Yup.string().max(500, 'Description must be 500 characters or fewer'),

  goal_direction: Yup.string()
    .oneOf(GOAL_DIRECTIONS as unknown as string[], 'Invalid goal direction')
    .required('Goal direction is required'),

  // ── Aggregation fields ────────────────────────────────────────────────
  event_key: Yup.string().when('kind', {
    is: 'aggregation',
    then: (s) =>
      s
        .trim()
        .min(1, 'Event key is required for aggregation metrics')
        .required('Event key is required for aggregation metrics'),
    otherwise: (s) => s,
  }),

  aggregator: Yup.string().when('kind', {
    is: 'aggregation',
    then: (s) =>
      s
        .oneOf(AGGREGATORS as unknown as string[], 'Invalid aggregator')
        .required('Aggregator is required'),
    otherwise: (s) => s,
  }),

  on_field: Yup.string().when(['kind', 'aggregator'], ([kind, agg], schema) => {
    if (kind === 'aggregation' && aggregatorRequiresField(String(agg))) {
      return schema
        .trim()
        .min(1, 'Field name is required for this aggregator')
        .required('Field name is required for this aggregator')
    }
    return schema
  }),

  where_clause: Yup.string().test(
    'valid-json',
    'Where clause must be valid JSON',
    (val) => {
      if (val == null || val.trim() === '') return true
      return parseWhereClause(val).ok
    },
  ),

  // ── Ratio fields ──────────────────────────────────────────────────────
  numerator_metric_id: Yup.string().when('kind', {
    is: 'ratio',
    then: (s) =>
      s
        .trim()
        .min(1, 'Numerator metric is required')
        .required('Numerator metric is required'),
    otherwise: (s) => s,
  }),

  denominator_metric_id: Yup.string().when('kind', {
    is: 'ratio',
    then: (s) =>
      s
        .trim()
        .min(1, 'Denominator metric is required')
        .required('Denominator metric is required')
        .test(
          'distinct-from-numerator',
          'Numerator and denominator must reference different metrics',
          function (value) {
            const num = (this.parent as { numerator_metric_id?: string })
              .numerator_metric_id
            if (!num || !value) return true
            return num !== value
          },
        ),
    otherwise: (s) => s,
  }),

  min_denominator: Yup.string().when('kind', {
    is: 'ratio',
    then: (s) =>
      s
        .matches(/^\d+$/, 'min_denominator must be a non-negative integer')
        .required('min_denominator is required'),
    otherwise: (s) => s,
  }),

  // ── Funnel fields ─────────────────────────────────────────────────────
  steps: Yup.array()
    .of(
      Yup.object({
        event_key: Yup.string().trim(),
        where_clause: Yup.string().test(
          'valid-step-json',
          'Step where clause must be valid JSON',
          (val) => {
            if (val == null || val.trim() === '') return true
            return parseWhereClause(val).ok
          },
        ),
      }),
    )
    .when('kind', {
      is: 'funnel',
      then: (s) =>
        s
          .min(2, 'Funnel metrics must have at least 2 steps')
          .test(
            'every-step-has-event-key',
            'Every funnel step needs an event key',
            (steps) =>
              steps == null ||
              steps.every(
                (st) => ((st as MetricStepValues).event_key ?? '').trim().length > 0,
              ),
          ),
      otherwise: (s) => s,
    }),

  window_seconds: Yup.string().when('kind', {
    is: 'funnel',
    then: (s) =>
      s
        .matches(/^\d+$/, 'window_seconds must be a positive integer')
        .test(
          'positive',
          'window_seconds must be strictly positive',
          (val) => val != null && Number.parseInt(val, 10) > 0,
        )
        .required('window_seconds is required'),
    otherwise: (s) => s,
  }),

  count_repeats: Yup.boolean(),
})

/**
 * Create-mode variant of `metricSchema`: requires `key` (which is
 * read-only on edit because the server treats it as immutable).
 */
export const metricCreateSchema = metricSchema.shape({
  key: Yup.string()
    .trim()
    .min(1, 'Key is required')
    .max(120, 'Key must be 120 characters or fewer')
    .matches(
      /^[a-z0-9][a-z0-9_-]*$/,
      'Key must start with a letter/digit and contain only lowercase letters, digits, hyphens, underscores',
    )
    .required('Key is required'),
})
