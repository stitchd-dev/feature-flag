/**
 * Unit tests for CreateExperimentModal — pure helpers + module-shape checks.
 *
 * The admin Vitest harness runs in node env (no jsdom / Testing Library), so
 * this file uses the same testable-seam pattern as `ExperimentDetail.test.ts`:
 *
 *   1. Yup schema (`experimentSchema`) — drives validate() to assert each
 *      field's rules.
 *   2. Pure helpers — `filterEligibleRules`, `filterMetricOptions`,
 *      `metricKindChipStyle`, `buildExperimentCreateBody`,
 *      `buildExperimentPatchBody`, `hasDefaultRuleDistribution`.
 *   3. Module-shape grep — confirms the modal component imports the new
 *      schema + helpers, surfaces the required form fields, and uses
 *      `validateOnChange={false}` (track learning).
 */
import { describe, it, expect } from 'vitest'
import { ValidationError } from 'yup'
import SOURCE from './CreateExperimentModal.tsx?raw'
import {
  experimentSchema,
  DEFAULT_BANDIT_CONFIG,
  type ExperimentFormValues,
  MAX_METRIC_IDS,
  MAX_GUARDRAIL_METRIC_IDS,
} from '../../lib/validation/experiment'
import {
  buildExperimentCreateBody,
  buildExperimentPatchBody,
  filterEligibleRules,
  filterMetricOptions,
  hasDefaultRuleDistribution,
  isPercentageRolloutRule,
  metricKindChipStyle,
  type MetricPickerOption,
} from './CreateExperimentModal.helpers'

// ─── Schema helpers ──────────────────────────────────────────────────────────

const validBase: ExperimentFormValues = {
  name: 'Checkout button colour',
  key: 'checkout-btn-colour',
  description: 'Test the button colour',
  flag_id: '00000000-0000-0000-0000-000000000001',
  flag_rule_id: '11111111-1111-1111-1111-111111111111',
  targets_default_rule: false,
  metric_ids: ['22222222-2222-2222-2222-222222222222'],
  guardrail_metric_ids: [],
  unit_context_types: ['user'],
  pre_period_days: 0,
  sequential_testing_enabled: false,
  sequential_alpha: 0.05,
  sequential_tau_squared: undefined,
  sequential_min_sample_size: 100,
  traffic_allocation: 100,
  model: 'bayesian',
  experiment_mode: 'fixed',
  bandit_config: DEFAULT_BANDIT_CONFIG,
}

async function validate(
  patch: Partial<ExperimentFormValues>,
): Promise<{ ok: true } | { ok: false; errors: Record<string, string> }> {
  try {
    await experimentSchema.validate({ ...validBase, ...patch }, { abortEarly: false })
    return { ok: true }
  } catch (err) {
    if (err instanceof ValidationError) {
      const errors: Record<string, string> = {}
      for (const inner of err.inner) {
        if (inner.path && !(inner.path in errors)) errors[inner.path] = inner.message
      }
      return { ok: false, errors }
    }
    throw err
  }
}

// ─── Schema: basic required + length validations ────────────────────────────

describe('experimentSchema — name + key + flag_id', () => {
  it('accepts a fully-valid base value', async () => {
    const r = await validate({})
    expect(r.ok).toBe(true)
  })

  it('rejects empty name', async () => {
    const r = await validate({ name: '' })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.name).toMatch(/required/i)
  })

  it('rejects name > 255 chars', async () => {
    const r = await validate({ name: 'x'.repeat(256) })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.name).toMatch(/255/i)
  })

  it('rejects invalid key format', async () => {
    const r = await validate({ key: 'Bad Key!' })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.key).toMatch(/lowercase/i)
  })

  it('rejects flag_id when missing', async () => {
    const r = await validate({ flag_id: '' })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.flag_id).toBeDefined()
  })
})

// ─── Schema: XOR rule binding ───────────────────────────────────────────────

describe('experimentSchema — flag_rule_id XOR targets_default_rule', () => {
  it('accepts a specific flag_rule_id with targets_default_rule=false', async () => {
    const r = await validate({
      flag_rule_id: '11111111-1111-1111-1111-111111111111',
      targets_default_rule: false,
    })
    expect(r.ok).toBe(true)
  })

  it('accepts targets_default_rule=true with empty flag_rule_id', async () => {
    const r = await validate({
      flag_rule_id: '',
      targets_default_rule: true,
    })
    expect(r.ok).toBe(true)
  })

  it('rejects when both flag_rule_id AND targets_default_rule are set', async () => {
    const r = await validate({
      flag_rule_id: '11111111-1111-1111-1111-111111111111',
      targets_default_rule: true,
    })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.flag_rule_id).toMatch(/cannot bind to both/i)
  })

  it('rejects when neither is set', async () => {
    const r = await validate({
      flag_rule_id: '',
      targets_default_rule: false,
    })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.flag_rule_id).toMatch(/pick a percentage/i)
  })

  it('rejects when flag_rule_id is set but not a UUID', async () => {
    const r = await validate({
      flag_rule_id: 'not-a-uuid',
      targets_default_rule: false,
    })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.flag_rule_id).toMatch(/uuid/i)
  })
})

// ─── Schema: metric_ids + guardrails ────────────────────────────────────────

describe('experimentSchema — metric_ids', () => {
  it('rejects empty metric_ids array', async () => {
    const r = await validate({ metric_ids: [] })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.metric_ids).toMatch(/at least 1|required/i)
  })

  it(`rejects more than ${MAX_METRIC_IDS} metric_ids`, async () => {
    const ids = Array.from({ length: MAX_METRIC_IDS + 1 }, (_, i) =>
      `${String(i + 1).repeat(8)}-1111-1111-1111-111111111111`.slice(0, 36),
    )
    const r = await validate({ metric_ids: ids })
    expect(r.ok).toBe(false)
  })

  it('accepts up to MAX guardrail_metric_ids', async () => {
    const ids = Array.from({ length: MAX_GUARDRAIL_METRIC_IDS }, (_, i) =>
      `${String(i + 1).repeat(8)}-1111-1111-1111-111111111111`.slice(0, 36),
    )
    const r = await validate({ guardrail_metric_ids: ids })
    expect(r.ok).toBe(true)
  })

  it('accepts an empty guardrail_metric_ids array (optional)', async () => {
    const r = await validate({ guardrail_metric_ids: [] })
    expect(r.ok).toBe(true)
  })
})

// ─── Schema: unit_context_types ─────────────────────────────────────────────

describe('experimentSchema — unit_context_types', () => {
  it('defaults to ["user"] in the valid base', async () => {
    expect(validBase.unit_context_types).toEqual(['user'])
  })

  it('rejects empty array', async () => {
    const r = await validate({ unit_context_types: [] })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.unit_context_types).toMatch(/at least 1/i)
  })

  it('accepts multiple distinct context types', async () => {
    const r = await validate({ unit_context_types: ['user', 'account'] })
    expect(r.ok).toBe(true)
  })

  it('rejects duplicate context types', async () => {
    const r = await validate({ unit_context_types: ['user', 'user'] })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.unit_context_types).toMatch(/unique/i)
  })
})

// ─── Schema: pre_period_days + traffic_allocation ───────────────────────────

describe('experimentSchema — pre_period_days', () => {
  it('accepts 0 (CUPED off)', async () => {
    const r = await validate({ pre_period_days: 0 })
    expect(r.ok).toBe(true)
  })

  it('rejects negative values', async () => {
    const r = await validate({ pre_period_days: -1 })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.pre_period_days).toMatch(/negative/i)
  })

  it('rejects non-integer values', async () => {
    const r = await validate({ pre_period_days: 1.5 })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.pre_period_days).toMatch(/whole/i)
  })
})

// ─── Schema: sequential testing ─────────────────────────────────────────────

describe('experimentSchema — sequential testing', () => {
  it('accepts the disabled default (enabled=false)', async () => {
    const r = await validate({ sequential_testing_enabled: false })
    expect(r.ok).toBe(true)
  })

  it('accepts a valid enabled config', async () => {
    const r = await validate({
      sequential_testing_enabled: true,
      sequential_alpha: 0.01,
      sequential_tau_squared: 0.25,
      sequential_min_sample_size: 200,
    })
    expect(r.ok).toBe(true)
  })

  it('rejects alpha = 0 when enabled', async () => {
    const r = await validate({ sequential_testing_enabled: true, sequential_alpha: 0 })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.sequential_alpha).toMatch(/between 0 and 1|greater than 0/i)
  })

  it('rejects alpha = 1 when enabled', async () => {
    const r = await validate({ sequential_testing_enabled: true, sequential_alpha: 1 })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.sequential_alpha).toMatch(/between 0 and 1|less than 1/i)
  })

  it('rejects negative tau² when provided', async () => {
    const r = await validate({
      sequential_testing_enabled: true,
      sequential_tau_squared: -0.5,
    })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.sequential_tau_squared).toMatch(/greater than 0|positive/i)
  })

  it('accepts a blank (auto-derive) tau² when enabled', async () => {
    const r = await validate({
      sequential_testing_enabled: true,
      sequential_tau_squared: undefined,
    })
    expect(r.ok).toBe(true)
  })

  it('rejects a negative min sample size when enabled', async () => {
    const r = await validate({
      sequential_testing_enabled: true,
      sequential_min_sample_size: -1,
    })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.sequential_min_sample_size).toMatch(/negative/i)
  })

  it('rejects a non-integer min sample size when enabled', async () => {
    const r = await validate({
      sequential_testing_enabled: true,
      sequential_min_sample_size: 10.5,
    })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.sequential_min_sample_size).toMatch(/whole/i)
  })

  it('does not require valid advanced knobs when disabled', async () => {
    // With the toggle off, an out-of-range alpha / negative min sample should
    // not block the form — the gateway ignores them when disabled.
    const r = await validate({
      sequential_testing_enabled: false,
      sequential_alpha: 0,
      sequential_min_sample_size: -5,
    })
    expect(r.ok).toBe(true)
  })

  it('always rejects a negative tau², even when disabled', async () => {
    // tau is the one knob that is validated whenever a value is present,
    // independent of the toggle (an explicit negative is never meaningful).
    const r = await validate({
      sequential_testing_enabled: false,
      sequential_tau_squared: -1,
    })
    expect(r.ok).toBe(false)
  })
})

describe('experimentSchema — traffic_allocation', () => {
  it('accepts 50 (mid-range)', async () => {
    const r = await validate({ traffic_allocation: 50 })
    expect(r.ok).toBe(true)
  })

  it('accepts 0.1 increments', async () => {
    const r = await validate({ traffic_allocation: 12.3 })
    expect(r.ok).toBe(true)
  })

  it('rejects values outside [0, 100]', async () => {
    const r = await validate({ traffic_allocation: 101 })
    expect(r.ok).toBe(false)
  })

  it('rejects sub-0.1 precision', async () => {
    const r = await validate({ traffic_allocation: 12.345 })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.errors.traffic_allocation).toMatch(/0\.1/i)
  })
})

// ─── Picker helpers ─────────────────────────────────────────────────────────

describe('metricKindChipStyle', () => {
  it('returns a distinct colour per metric kind', () => {
    expect(metricKindChipStyle('aggregation').color).toBe('#2563eb')
    expect(metricKindChipStyle('ratio').color).toBe('#9333ea')
    expect(metricKindChipStyle('funnel').color).toBe('#16a34a')
  })
})

describe('filterMetricOptions', () => {
  const opts: MetricPickerOption[] = [
    { id: 'a', key: 'checkout_completed', name: 'Checkout completed', kind: 'aggregation' },
    { id: 'b', key: 'cart_value', name: 'Cart value (USD)', kind: 'aggregation' },
    { id: 'c', key: 'signup_funnel', name: 'Signup funnel', kind: 'funnel' },
  ]

  it('returns all options when query is empty', () => {
    expect(filterMetricOptions(opts, '')).toHaveLength(3)
  })

  it('filters by name (case-insensitive substring)', () => {
    expect(filterMetricOptions(opts, 'cart').map((o) => o.id)).toEqual(['b'])
  })

  it('filters by key (case-insensitive substring)', () => {
    expect(filterMetricOptions(opts, 'SIGNUP').map((o) => o.id)).toEqual(['c'])
  })

  it('hides selected options', () => {
    expect(filterMetricOptions(opts, '', new Set(['a'])).map((o) => o.id)).toEqual([
      'b',
      'c',
    ])
  })
})

// ─── Rule picker eligibility ────────────────────────────────────────────────

describe('isPercentageRolloutRule', () => {
  it('returns true for an allocation output', () => {
    expect(
      isPercentageRolloutRule({
        allocation: {
          buckets: [{ variant_key: 'a', weight_bp: 5000 }],
        },
      }),
    ).toBe(true)
  })

  it('returns false for a specific variant output', () => {
    expect(isPercentageRolloutRule({ variant_key: 'control' })).toBe(false)
  })

  it('returns false for null / undefined', () => {
    expect(isPercentageRolloutRule(null)).toBe(false)
    expect(isPercentageRolloutRule(undefined)).toBe(false)
  })

  it('returns false for an allocation with empty buckets', () => {
    expect(isPercentageRolloutRule({ allocation: { buckets: [] } })).toBe(false)
  })
})

describe('filterEligibleRules', () => {
  const rules = [
    {
      rule_id: 'r1',
      name: 'percentage A',
      output: { allocation: { buckets: [{ variant_key: 'a', weight_bp: 5000 }] } },
    },
    { rule_id: 'r2', name: 'specific variant', output: { variant_key: 'control' } },
    {
      rule_id: 'r3',
      name: null,
      output: { allocation: { buckets: [{ variant_key: 'b', weight_bp: 10000 }] } },
    },
  ]

  it('marks percentage-rollout rules eligible', () => {
    const opts = filterEligibleRules(rules, null)
    expect(opts[0].eligible).toBe(true)
    expect(opts[2].eligible).toBe(true)
  })

  it('marks specific-variant rules ineligible with a tooltip', () => {
    const opts = filterEligibleRules(rules, null)
    expect(opts[1].eligible).toBe(false)
    expect(opts[1].disabled_reason).toMatch(/percentage rollout/i)
  })

  it('falls back to "Rule N" when the rule has no name', () => {
    const opts = filterEligibleRules(rules, null)
    expect(opts[2].label).toBe('Rule 3')
  })

  it('omits the default-rule entry when distribution is null', () => {
    const opts = filterEligibleRules(rules, null)
    expect(opts.find((o) => o.kind === 'default_rule')).toBeUndefined()
  })

  it('appends a default-rule entry when distribution is set', () => {
    const opts = filterEligibleRules(rules, {
      allocations: [{ variant_key: 'a', percentage: 100 }],
    })
    const defaultEntry = opts.find((o) => o.kind === 'default_rule')
    expect(defaultEntry).toBeDefined()
    expect(defaultEntry?.eligible).toBe(true)
    expect(defaultEntry?.label).toMatch(/default rule/i)
    expect(defaultEntry?.rule_id).toBeNull()
  })
})

describe('hasDefaultRuleDistribution', () => {
  it('returns false for null / undefined / empty', () => {
    expect(hasDefaultRuleDistribution(null)).toBe(false)
    expect(hasDefaultRuleDistribution(undefined)).toBe(false)
    expect(hasDefaultRuleDistribution({ allocations: [] })).toBe(false)
  })

  it('returns true for a populated distribution', () => {
    expect(
      hasDefaultRuleDistribution({
        allocations: [{ variant_key: 'a', percentage: 100 }],
      }),
    ).toBe(true)
  })
})

// ─── Submit body shape ──────────────────────────────────────────────────────

describe('buildExperimentCreateBody', () => {
  it('includes flag_rule_id when targets_default_rule=false', () => {
    const body = buildExperimentCreateBody(validBase, 'env-42')
    expect(body.flag_rule_id).toBe(validBase.flag_rule_id)
    expect(body.targets_default_rule).toBeUndefined()
  })

  it('omits flag_rule_id and sets targets_default_rule when default-bound', () => {
    const body = buildExperimentCreateBody(
      { ...validBase, flag_rule_id: '', targets_default_rule: true },
      'env-1',
    )
    expect(body.flag_rule_id).toBeUndefined()
    expect(body.targets_default_rule).toBe(true)
  })

  it('emits primary + guardrail metric_ids arrays', () => {
    const body = buildExperimentCreateBody(
      {
        ...validBase,
        metric_ids: ['11111111-1111-1111-1111-111111111111'],
        guardrail_metric_ids: ['22222222-2222-2222-2222-222222222222'],
      },
      'env-1',
    )
    expect(body.metric_ids).toEqual(['11111111-1111-1111-1111-111111111111'])
    expect(body.guardrail_metric_ids).toEqual([
      '22222222-2222-2222-2222-222222222222',
    ])
  })

  it('emits unit_context_types and pre_period_days', () => {
    const body = buildExperimentCreateBody(
      { ...validBase, unit_context_types: ['user', 'account'], pre_period_days: 14 },
      'env-1',
    )
    expect(body.unit_context_types).toEqual(['user', 'account'])
    expect(body.pre_period_days).toBe(14)
  })

  it('converts traffic_allocation from 0–100 percent to 0–1 float', () => {
    const body = buildExperimentCreateBody(
      { ...validBase, traffic_allocation: 25 },
      'env-1',
    )
    expect(body.traffic_allocation).toBeCloseTo(0.25, 6)
  })

  it('emits the sequential testing config keys with defaults', () => {
    const body = buildExperimentCreateBody(validBase, 'env-1')
    expect(body.sequential_testing_enabled).toBe(false)
    expect(body.sequential_alpha).toBe(0.05)
    expect(body.sequential_min_sample_size).toBe(100)
  })

  it('omits sequential_tau_squared when blank (auto-derive)', () => {
    const body = buildExperimentCreateBody(
      { ...validBase, sequential_tau_squared: undefined },
      'env-1',
    )
    expect('sequential_tau_squared' in body).toBe(false)
  })

  it('sends sequential_tau_squared when provided', () => {
    const body = buildExperimentCreateBody(
      { ...validBase, sequential_tau_squared: 0.42 },
      'env-1',
    )
    expect(body.sequential_tau_squared).toBeCloseTo(0.42, 6)
  })

  it('passes through an enabled sequential config', () => {
    const body = buildExperimentCreateBody(
      {
        ...validBase,
        sequential_testing_enabled: true,
        sequential_alpha: 0.01,
        sequential_tau_squared: 0.1,
        sequential_min_sample_size: 250,
      },
      'env-1',
    )
    expect(body.sequential_testing_enabled).toBe(true)
    expect(body.sequential_alpha).toBeCloseTo(0.01, 6)
    expect(body.sequential_tau_squared).toBeCloseTo(0.1, 6)
    expect(body.sequential_min_sample_size).toBe(250)
  })

  it('trims and drops empty description', () => {
    const body = buildExperimentCreateBody(
      { ...validBase, description: '   ' },
      'env-1',
    )
    expect(body.description).toBeUndefined()
  })

  it('sets environment_id', () => {
    const body = buildExperimentCreateBody(validBase, 'env-99')
    expect(body.environment_id).toBe('env-99')
  })
})

describe('buildExperimentPatchBody', () => {
  it('omits key + flag_id (immutable on edit)', () => {
    const body = buildExperimentPatchBody(validBase)
    expect(body.key).toBeUndefined()
    expect(body.flag_id).toBeUndefined()
    expect(body.environment_id).toBeUndefined()
  })

  it('emits explicit flag_rule_id null when switching to default-rule', () => {
    const body = buildExperimentPatchBody({
      ...validBase,
      flag_rule_id: '',
      targets_default_rule: true,
    })
    expect(body.flag_rule_id).toBeNull()
    expect(body.targets_default_rule).toBe(true)
  })

  it('emits explicit targets_default_rule=false when switching to a rule', () => {
    const body = buildExperimentPatchBody({
      ...validBase,
      targets_default_rule: false,
    })
    expect(body.targets_default_rule).toBe(false)
    expect(body.flag_rule_id).toBe(validBase.flag_rule_id)
  })

  it('carries the sequential testing config through on edit', () => {
    const body = buildExperimentPatchBody({
      ...validBase,
      sequential_testing_enabled: true,
      sequential_alpha: 0.02,
      sequential_tau_squared: 0.3,
      sequential_min_sample_size: 150,
    })
    expect(body.sequential_testing_enabled).toBe(true)
    expect(body.sequential_alpha).toBeCloseTo(0.02, 6)
    expect(body.sequential_tau_squared).toBeCloseTo(0.3, 6)
    expect(body.sequential_min_sample_size).toBe(150)
  })

  it('omits sequential_tau_squared on edit when blank', () => {
    const body = buildExperimentPatchBody({
      ...validBase,
      sequential_tau_squared: undefined,
    })
    expect('sequential_tau_squared' in body).toBe(false)
  })
})

// ─── Module-shape grep tests ────────────────────────────────────────────────

describe('CreateExperimentModal (module shape)', () => {
  it('imports the Phase 10 schema from lib/validation/experiment', () => {
    expect(SOURCE).toMatch(/from\s+['"][^'"]*lib\/validation\/experiment['"]/)
  })

  it('imports the helpers module', () => {
    expect(SOURCE).toMatch(/from\s+['"]\.\/CreateExperimentModal\.helpers['"]/)
  })

  it('uses Formik with validateOnChange={false} for async-tolerant validation', () => {
    expect(SOURCE).toMatch(/validateOnChange=\{false\}/)
  })

  it('uses enableReinitialize for edit mode', () => {
    expect(SOURCE).toMatch(/enableReinitialize/)
  })

  it('renders the FormErrorBanner primitive for inline server errors', () => {
    expect(SOURCE).toMatch(/FormErrorBanner/)
  })

  it('renders the rule picker, unit_context_types, guardrail, and pre_period inputs', () => {
    // We don't depend on the exact JSX shape — just that each labeled
    // field is present in the file. (Tests assert behaviour via helpers
    // + schema; the grep confirms wiring is in place.)
    expect(SOURCE).toMatch(/flag_rule_id|targets_default_rule|RulePicker/)
    expect(SOURCE).toMatch(/unit_context_types/)
    expect(SOURCE).toMatch(/guardrail_metric_ids/)
    expect(SOURCE).toMatch(/pre_period_days/)
  })

  it('renders the sequential testing section (toggle + advanced knobs)', () => {
    // Opt-in toggle is always present…
    expect(SOURCE).toMatch(/sequential_testing_enabled/)
    // …and the three advanced knobs are wired as form fields.
    expect(SOURCE).toMatch(/sequential_alpha/)
    expect(SOURCE).toMatch(/sequential_tau_squared/)
    expect(SOURCE).toMatch(/sequential_min_sample_size/)
  })

  it('gates the sequential advanced knobs behind the enabled toggle', () => {
    // The advanced inputs must be conditional on the toggle value so they
    // only render when sequential testing is enabled.
    expect(SOURCE).toMatch(/values\.sequential_testing_enabled\s*&&/)
  })

  it('does not synthesise a placeholder rule UUID from the array index (feature-flag-1p6)', () => {
    // The gateway now surfaces `feature_flag_rules.id` as `rule_id`. The
    // earlier workaround that built a UUID from the rule's array index
    // (`00000000-0000-0000-0000-${idx}`) must be gone — otherwise the
    // experiment-create body posts a placeholder the backend rejects.
    expect(SOURCE).not.toMatch(/00000000-0000-0000-0000-\$\{/)
    expect(SOURCE).not.toMatch(/padStart\(12,\s*['"]0['"]\)/)
    // The narrower acceptance grep the spec uses must also stay clean.
    expect(SOURCE).not.toMatch(/synthetic|index-derived|index.*UUID/)
  })
})
