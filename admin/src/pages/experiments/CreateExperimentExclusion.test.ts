/**
 * CreateExperimentModal exclusion-group picker tests (Phase 8 — xexp, P8.T2).
 *
 * The admin Vitest harness is node-env (no jsdom), so this mirrors the
 * existing CreateExperimentModal.test.ts seam pattern:
 *   1. Schema — the optional `exclusion_group_id` field accepts empty / valid
 *      UUID and rejects garbage.
 *   2. Capacity validation — `exclusionGroupFitsCapacity` drives the warn/block
 *      decision the picker + submit handler use.
 *   3. Module-shape grep — the modal renders the picker, derives requested_bp
 *      from traffic_allocation, and calls the assign endpoint.
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './CreateExperimentModal.tsx?raw'
import {
  experimentSchema,
  type ExperimentFormValues,
} from '../../lib/validation/experiment'
import { exclusionGroupFitsCapacity } from './exclusionGroups/capacity'

const validBase: ExperimentFormValues = {
  name: 'Checkout button colour',
  key: 'checkout-btn-colour',
  description: '',
  flag_id: '00000000-0000-0000-0000-000000000001',
  flag_rule_id: '11111111-1111-1111-1111-111111111111',
  targets_default_rule: false,
  metric_ids: ['22222222-2222-2222-2222-222222222222'],
  guardrail_metric_ids: [],
  unit_context_types: ['user'],
  pre_period_days: 0,
  traffic_allocation: 100,
  model: 'bayesian',
}

// ─── Schema: optional exclusion_group_id ─────────────────────────────────────

describe('experimentSchema.exclusion_group_id', () => {
  it('is valid when omitted (ungrouped default)', async () => {
    await expect(experimentSchema.validate(validBase)).resolves.toBeTruthy()
  })

  it('is valid when empty string', async () => {
    await expect(
      experimentSchema.validate({ ...validBase, exclusion_group_id: '' }),
    ).resolves.toBeTruthy()
  })

  it('is valid for a UUID', async () => {
    await expect(
      experimentSchema.validate({
        ...validBase,
        exclusion_group_id: '33333333-3333-3333-3333-333333333333',
      }),
    ).resolves.toBeTruthy()
  })

  it('rejects a non-UUID group id', async () => {
    await expect(
      experimentSchema.validate({
        ...validBase,
        exclusion_group_id: 'not-a-uuid',
      }),
    ).rejects.toThrow(/valid UUID/i)
  })
})

// ─── Capacity validation seam ────────────────────────────────────────────────

describe('exclusionGroupFitsCapacity (picker + submit guard)', () => {
  it('allows an allocation that fits the free budget', () => {
    // 25% traffic → 2500 bp, group has 5000 bp free → fits.
    const r = exclusionGroupFitsCapacity({ free_bp: 5000 }, 25)
    expect(r.requestedBp).toBe(2500)
    expect(r.fits).toBe(true)
  })

  it('blocks an allocation that exceeds the free budget', () => {
    // 75% traffic → 7500 bp, group has 5000 bp free → blocked.
    const r = exclusionGroupFitsCapacity({ free_bp: 5000 }, 75)
    expect(r.requestedBp).toBe(7500)
    expect(r.fits).toBe(false)
    expect(r.message).toMatch(/exceeds/i)
  })
})

// ─── Module shape ────────────────────────────────────────────────────────────

describe('CreateExperimentModal (exclusion-group wiring)', () => {
  it('renders the optional exclusion-group picker', () => {
    expect(SOURCE).toMatch(/ExclusionGroupPicker/)
    expect(SOURCE).toMatch(/Mutual-exclusion group/)
  })

  it('loads groups via listExclusionGroups', () => {
    expect(SOURCE).toMatch(/listExclusionGroups/)
  })

  it('derives requested_bp from traffic_allocation (× 100)', () => {
    expect(SOURCE).toMatch(/traffic_allocation \* 100|trafficPercentToBp|capacity\.requestedBp/)
  })

  it('calls the assign endpoint on create', () => {
    expect(SOURCE).toMatch(/assignExperimentToGroup/)
    expect(SOURCE).toMatch(/requested_bp/)
  })

  it('validates capacity before assigning', () => {
    expect(SOURCE).toMatch(/exclusionGroupFitsCapacity/)
  })
})
