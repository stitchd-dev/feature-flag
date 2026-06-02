/**
 * Results-tab interaction-warning tests (Phase 8 — xexp, P8.T3).
 *
 * `ExperimentDetail` wires React contexts (OrgContext / ContextTypeContext) and
 * cannot be cleanly rendered in the node-env harness, so this covers the
 * warning-banner logic two ways:
 *   1. The pure gate (`hasSignificantInteraction`) that drives the banner.
 *   2. Module-shape grep confirming the banner renders ONLY behind that gate
 *      and that interactions are fetched on mount (so the banner can appear
 *      without opening the Interactions tab).
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './ExperimentDetail.tsx?raw'
import { hasSignificantInteraction } from './tabs/Interactions'
import type { ExperimentInteraction } from '../../lib/api/exclusionGroups'

function interaction(
  significant: boolean,
  insufficient_data = false,
): ExperimentInteraction {
  return {
    experiment_id_a: 'a',
    experiment_id_b: 'b',
    other_experiment_name: 'Other',
    context_type: 'user',
    metric_key: 'm',
    shared_count: 10,
    interaction_estimate: insufficient_data ? 0.0 : 0.1,
    p_value: insufficient_data ? 0.0 : significant ? 0.001 : 0.5,
    significant,
    insufficient_data,
  }
}

describe('interaction-warning gate', () => {
  it('shows the warning only when a significant interaction exists', () => {
    expect(hasSignificantInteraction([interaction(true)])).toBe(true)
    expect(hasSignificantInteraction([interaction(false)])).toBe(false)
    expect(hasSignificantInteraction([])).toBe(false)
  })

  it('does not count insufficient-data rows toward the warning', () => {
    // significant=true on the wire but insufficient_data=true → not a real hit.
    expect(hasSignificantInteraction([interaction(true, true)])).toBe(false)
  })
})

describe('ExperimentDetail (interaction wiring)', () => {
  it('fetches interactions via listExperimentInteractions', () => {
    expect(SOURCE).toMatch(/listExperimentInteractions/)
  })

  it('gates the Results warning banner on showInteractionWarning', () => {
    expect(SOURCE).toMatch(/showInteractionWarning && \(/)
    expect(SOURCE).toMatch(/data-testid="interaction-warning"/)
  })

  it('derives the gate from hasSignificantInteraction', () => {
    expect(SOURCE).toMatch(/hasSignificantInteraction\(interactions\)/)
  })

  it('renders the Interactions tab sibling to Results/Iterations/Exposures', () => {
    expect(SOURCE).toMatch(/tab === 'interactions'/)
    expect(SOURCE).toMatch(/InteractionsTab/)
  })
})
