/**
 * CreateExperimentModal bandit-config tests (Phase 12, Task 12.1).
 *
 * Node-env harness (no jsdom), so this mirrors the existing
 * CreateExperimentModal.test.ts seam pattern:
 *   1. Schema — `experiment_mode` + the conditionally-validated
 *      `bandit_config` sub-schema (algorithm / objective / contextual /
 *      campaign rules) and the mode=fixed bypass.
 *   2. Payload builders — `buildBanditConfigPayload`, `buildBanditObjective`,
 *      `parseBanditConfig`, and that `buildExperimentCreateBody` /
 *      `buildExperimentPatchBody` emit `bandit_config` only for mode=bandit.
 *   3. Module-shape grep — the modal renders the mode picker + bandit fields.
 */
import { describe, it, expect } from 'vitest'
import { ValidationError } from 'yup'
import SOURCE from './CreateExperimentModal.tsx?raw'
import {
  experimentSchema,
  DEFAULT_BANDIT_CONFIG,
  type ExperimentFormValues,
  type BanditConfigFormValues,
} from '../../lib/validation/experiment'
import {
  buildExperimentCreateBody,
  buildExperimentPatchBody,
  buildBanditConfigPayload,
  buildBanditObjective,
  parseBanditConfig,
} from './CreateExperimentModal.helpers'

const M1 = '22222222-2222-2222-2222-222222222222'
const M2 = '33333333-3333-3333-3333-333333333333'
const M3 = '44444444-4444-4444-4444-444444444444'

const validBase: ExperimentFormValues = {
  name: 'Checkout button colour',
  key: 'checkout-btn-colour',
  description: '',
  flag_id: '00000000-0000-0000-0000-000000000001',
  flag_rule_id: '11111111-1111-1111-1111-111111111111',
  targets_default_rule: false,
  metric_ids: [M1],
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

function banditCfg(
  patch: Partial<BanditConfigFormValues> = {},
): BanditConfigFormValues {
  return { ...DEFAULT_BANDIT_CONFIG, objective_metric_id: M1, ...patch }
}

async function validate(
  patch: Partial<ExperimentFormValues>,
): Promise<{ ok: true } | { ok: false; errors: Record<string, string> }> {
  try {
    await experimentSchema.validate(
      { ...validBase, ...patch },
      { abortEarly: false },
    )
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

// ─── Schema: mode + conditional config ───────────────────────────────────────

describe('experimentSchema.experiment_mode', () => {
  it('defaults to fixed and validates without a bandit config', async () => {
    const res = await validate({ experiment_mode: 'fixed' })
    expect(res.ok).toBe(true)
  })

  it('rejects an unknown mode', async () => {
    // @ts-expect-error — deliberately invalid
    const res = await validate({ experiment_mode: 'multi-armed' })
    expect(res.ok).toBe(false)
  })

  it('a fixed experiment ignores a half-filled bandit config', async () => {
    const res = await validate({
      experiment_mode: 'fixed',
      bandit_config: banditCfg({ objective_metric_id: '' }),
    })
    expect(res.ok).toBe(true)
  })
})

describe('experimentSchema bandit_config (mode=bandit)', () => {
  it('accepts a valid scalar-objective Thompson config', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg(),
    })
    expect(res.ok).toBe(true)
  })

  it('requires an objective metric for a scalar objective', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({ objective_kind: 'scalar', objective_metric_id: '' }),
    })
    expect(res.ok).toBe(false)
  })

  it('rejects an exploration floor above 50%', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({ min_exploration_pct: 75 }),
    })
    expect(res.ok).toBe(false)
  })

  it('rejects a convergence threshold outside (0,1)', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({ convergence_prob_threshold: 1.5 }),
    })
    expect(res.ok).toBe(false)
  })

  it('contextual algorithm needs at least one feature', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({ algorithm: 'contextual', contextual_features: [] }),
    })
    expect(res.ok).toBe(false)
  })

  it('contextual algorithm passes with a feature', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({
        algorithm: 'contextual',
        contextual_features: ['user.country'],
      }),
    })
    expect(res.ok).toBe(true)
  })

  it('scalarized objective needs at least one weighted metric', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({ objective_kind: 'scalarized', scalarized_weights: [] }),
    })
    expect(res.ok).toBe(false)
  })

  it('scalarized objective passes with weighted metrics', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({
        objective_kind: 'scalarized',
        scalarized_weights: [
          { metric_id: M1, weight: 0.7 },
          { metric_id: M2, weight: 0.3 },
        ],
      }),
    })
    expect(res.ok).toBe(true)
  })

  it('constrained objective needs a primary metric + at least one constraint', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({ objective_kind: 'constrained', constraints: [] }),
    })
    expect(res.ok).toBe(false)
  })

  it('constrained objective passes with a primary + constraint', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({
        objective_kind: 'constrained',
        objective_metric_id: M1,
        constraints: [{ metric_id: M2, bound: 0.5, direction: 'gte' }],
      }),
    })
    expect(res.ok).toBe(true)
  })

  it('campaign requires a positive max-iterations when enabled', async () => {
    const res = await validate({
      experiment_mode: 'bandit',
      bandit_config: banditCfg({ campaign_enabled: true, campaign_max_iterations: 0 }),
    })
    expect(res.ok).toBe(false)
  })
})

// ─── buildBanditObjective ────────────────────────────────────────────────────

describe('buildBanditObjective', () => {
  it('builds a scalar objective', () => {
    expect(buildBanditObjective(banditCfg({ objective_kind: 'scalar', objective_metric_id: M1 }))).toEqual({
      type: 'scalar',
      metric_id: M1,
    })
  })

  it('builds a scalarized objective with weights', () => {
    const obj = buildBanditObjective(
      banditCfg({
        objective_kind: 'scalarized',
        scalarized_weights: [
          { metric_id: M1, weight: 0.7 },
          { metric_id: M2, weight: 0.3 },
        ],
      }),
    )
    expect(obj).toEqual({
      type: 'scalarized',
      weights: [
        { metric_id: M1, weight: 0.7 },
        { metric_id: M2, weight: 0.3 },
      ],
    })
  })

  it('builds a constrained objective with primary + constraints', () => {
    const obj = buildBanditObjective(
      banditCfg({
        objective_kind: 'constrained',
        objective_metric_id: M1,
        constraints: [{ metric_id: M2, bound: 0.5, direction: 'lte' }],
      }),
    )
    expect(obj).toEqual({
      type: 'constrained',
      primary_metric_id: M1,
      constraints: [{ metric_id: M2, bound: 0.5, direction: 'lte' }],
    })
  })
})

// ─── buildBanditConfigPayload ────────────────────────────────────────────────

describe('buildBanditConfigPayload', () => {
  it('converts the exploration floor percent to basis points', () => {
    const payload = buildBanditConfigPayload(banditCfg({ min_exploration_pct: 5 }))
    expect(payload.min_exploration_bp).toBe(500)
  })

  it('carries algorithm, propagation, lifecycle, threshold', () => {
    const payload = buildBanditConfigPayload(
      banditCfg({
        algorithm: 'ucb',
        propagation_mode: 'realtime',
        lifecycle_policy: 'auto_rollout',
        convergence_prob_threshold: 0.9,
      }),
    )
    expect(payload).toMatchObject({
      algorithm: 'ucb',
      propagation_mode: 'realtime',
      lifecycle_policy: 'auto_rollout',
      convergence_prob_threshold: 0.9,
    })
  })

  it('omits contextual_features for non-contextual algorithms', () => {
    const payload = buildBanditConfigPayload(banditCfg({ algorithm: 'thompson' }))
    expect(payload).not.toHaveProperty('contextual_features')
  })

  it('includes contextual_features for contextual algorithm', () => {
    const payload = buildBanditConfigPayload(
      banditCfg({ algorithm: 'contextual', contextual_features: ['user.country', ''] }),
    )
    expect(payload.contextual_features).toEqual(['user.country'])
  })

  it('omits campaign block when not enabled', () => {
    const payload = buildBanditConfigPayload(banditCfg({ campaign_enabled: false }))
    expect(payload).not.toHaveProperty('campaign')
  })

  it('includes campaign block when enabled', () => {
    const payload = buildBanditConfigPayload(
      banditCfg({
        campaign_enabled: true,
        campaign_max_iterations: 7,
        campaign_drift_threshold: 0.2,
      }),
    )
    expect(payload.campaign).toEqual({ max_iterations: 7, drift_threshold: 0.2 })
  })
})

// ─── parseBanditConfig (edit-mode round-trip) ────────────────────────────────

describe('parseBanditConfig', () => {
  it('round-trips a built payload back into form values', () => {
    const cfg = banditCfg({
      algorithm: 'epsilon_greedy',
      min_exploration_pct: 3,
      lifecycle_policy: 'auto_commit',
      objective_kind: 'scalarized',
      scalarized_weights: [{ metric_id: M1, weight: 0.6 }],
      campaign_enabled: true,
      campaign_max_iterations: 4,
      campaign_drift_threshold: 0.15,
    })
    const payload = buildBanditConfigPayload(cfg)
    const parsed = parseBanditConfig(payload, DEFAULT_BANDIT_CONFIG)
    expect(parsed.algorithm).toBe('epsilon_greedy')
    expect(parsed.min_exploration_pct).toBe(3)
    expect(parsed.lifecycle_policy).toBe('auto_commit')
    expect(parsed.objective_kind).toBe('scalarized')
    expect(parsed.scalarized_weights).toEqual([{ metric_id: M1, weight: 0.6 }])
    expect(parsed.campaign_enabled).toBe(true)
    expect(parsed.campaign_max_iterations).toBe(4)
  })

  it('parses a constrained objective payload', () => {
    const payload = {
      algorithm: 'thompson',
      objective: {
        type: 'constrained',
        primary_metric_id: M1,
        constraints: [{ metric_id: M3, bound: 0.4, direction: 'lte' }],
      },
    }
    const parsed = parseBanditConfig(payload, DEFAULT_BANDIT_CONFIG)
    expect(parsed.objective_kind).toBe('constrained')
    expect(parsed.objective_metric_id).toBe(M1)
    expect(parsed.constraints).toEqual([{ metric_id: M3, bound: 0.4, direction: 'lte' }])
  })

  it('falls back to defaults for empty/garbage input', () => {
    expect(parseBanditConfig(null, DEFAULT_BANDIT_CONFIG)).toEqual(DEFAULT_BANDIT_CONFIG)
    expect(parseBanditConfig('nope', DEFAULT_BANDIT_CONFIG)).toEqual(DEFAULT_BANDIT_CONFIG)
  })
})

// ─── Submit body integration ─────────────────────────────────────────────────

describe('buildExperimentCreateBody bandit', () => {
  it('omits bandit_config for a fixed experiment', () => {
    const body = buildExperimentCreateBody(validBase, 'env-1')
    expect(body.experiment_mode).toBe('fixed')
    expect(body).not.toHaveProperty('bandit_config')
  })

  it('includes a bandit_config for a bandit experiment', () => {
    const body = buildExperimentCreateBody(
      { ...validBase, experiment_mode: 'bandit', bandit_config: banditCfg() },
      'env-1',
    )
    expect(body.experiment_mode).toBe('bandit')
    expect(body.bandit_config).toMatchObject({
      algorithm: 'thompson',
      min_exploration_bp: 500,
      objective: { type: 'scalar', metric_id: M1 },
    })
  })
})

describe('buildExperimentPatchBody bandit', () => {
  it('switches an experiment to bandit mode on edit', () => {
    const body = buildExperimentPatchBody({
      ...validBase,
      experiment_mode: 'bandit',
      bandit_config: banditCfg({ algorithm: 'ucb' }),
    })
    expect(body.experiment_mode).toBe('bandit')
    expect((body.bandit_config as Record<string, unknown>).algorithm).toBe('ucb')
  })

  it('omits bandit_config when patching back to fixed', () => {
    const body = buildExperimentPatchBody({ ...validBase, experiment_mode: 'fixed' })
    expect(body.experiment_mode).toBe('fixed')
    expect(body).not.toHaveProperty('bandit_config')
  })
})

// ─── Module shape ────────────────────────────────────────────────────────────

describe('CreateExperimentModal bandit (module shape)', () => {
  it('renders an experiment-mode picker', () => {
    expect(SOURCE).toMatch(/experiment_mode/)
    expect(SOURCE).toMatch(/EXPERIMENT_MODE_OPTIONS/)
  })

  it('gates the bandit config section behind mode=bandit', () => {
    expect(SOURCE).toMatch(/experiment_mode\s*!==\s*['"]bandit['"]/)
    expect(SOURCE).toMatch(/BanditConfigSection/)
  })

  it('surfaces the bandit knobs (algorithm, propagation, lifecycle, objective)', () => {
    expect(SOURCE).toMatch(/bandit_config\.algorithm/)
    expect(SOURCE).toMatch(/bandit_config\.propagation_mode/)
    expect(SOURCE).toMatch(/bandit_config\.lifecycle_policy/)
    expect(SOURCE).toMatch(/bandit_config\.objective_kind/)
    expect(SOURCE).toMatch(/bandit_config\.min_exploration_pct/)
  })

  it('surfaces the optional campaign config', () => {
    expect(SOURCE).toMatch(/bandit_config\.campaign_enabled/)
    expect(SOURCE).toMatch(/bandit_config\.campaign_max_iterations/)
  })
})
