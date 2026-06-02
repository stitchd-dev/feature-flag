/**
 * Exclusion-group tests (Phase 8 — xexp, P8.T1 + shared capacity helpers).
 *
 * Coverage (node env + renderToString, per the admin Vitest convention):
 *   • Pure capacity helpers: allocatedPercent / freePercent / formatBpPercent /
 *     trafficPercentToBp / fitsWithinFree / exclusionGroupFitsCapacity /
 *     countMembersByGroup.
 *   • CapacityBar renders the allocated/free percentages + member count and a
 *     progressbar with the correct aria-valuenow.
 *   • Module shape: the list page reads listExclusionGroups + renders the
 *     CapacityBar + create modal; the modal posts via createExclusionGroup.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import LIST_SOURCE from './ExclusionGroups.tsx?raw'
import MODAL_SOURCE from './ExclusionGroupModal.tsx?raw'
import {
  allocatedPercent,
  freePercent,
  formatBpPercent,
  trafficPercentToBp,
  fitsWithinFree,
  exclusionGroupFitsCapacity,
  countMembersByGroup,
} from './capacity'
import { CapacityBar } from './CapacityBar'
import type { ExclusionGroup } from '../../../lib/api/exclusionGroups'

const GROUP: ExclusionGroup = {
  id: 'group-1',
  env_id: 'env-1',
  name: 'Checkout funnel',
  description: 'Shared checkout experiments',
  allocated_bp: 2500,
  free_bp: 7500,
  version: 1,
}

// ─── Capacity helpers ────────────────────────────────────────────────────────

describe('allocatedPercent / freePercent', () => {
  it('computes allocated percent from basis points', () => {
    expect(allocatedPercent({ allocated_bp: 2500 })).toBe(25)
    expect(allocatedPercent({ allocated_bp: 10000 })).toBe(100)
    expect(allocatedPercent({ allocated_bp: 0 })).toBe(0)
  })

  it('computes free percent from basis points', () => {
    expect(freePercent({ free_bp: 7500 })).toBe(75)
    expect(freePercent({ free_bp: 0 })).toBe(0)
  })

  it('clamps out-of-range values to [0, 100]', () => {
    expect(allocatedPercent({ allocated_bp: 12000 })).toBe(100)
    expect(allocatedPercent({ allocated_bp: -500 })).toBe(0)
  })
})

describe('formatBpPercent', () => {
  it('formats whole percentages without a decimal', () => {
    expect(formatBpPercent(2500)).toBe('25%')
    expect(formatBpPercent(10000)).toBe('100%')
  })

  it('formats fractional percentages to one decimal', () => {
    expect(formatBpPercent(1250)).toBe('12.5%')
  })
})

describe('trafficPercentToBp', () => {
  it('converts traffic% to basis points (× 100)', () => {
    expect(trafficPercentToBp(100)).toBe(10000)
    expect(trafficPercentToBp(12.5)).toBe(1250)
    expect(trafficPercentToBp(0)).toBe(0)
  })
})

describe('fitsWithinFree', () => {
  it('returns true when the request fits the free budget', () => {
    expect(fitsWithinFree({ free_bp: 7500 }, 5000)).toBe(true)
    expect(fitsWithinFree({ free_bp: 7500 }, 7500)).toBe(true) // exact fit
  })

  it('returns false when the request exceeds the free budget', () => {
    expect(fitsWithinFree({ free_bp: 7500 }, 8000)).toBe(false)
  })
})

describe('exclusionGroupFitsCapacity', () => {
  it('passes when the traffic allocation fits', () => {
    const r = exclusionGroupFitsCapacity({ free_bp: 7500 }, 50)
    expect(r.requestedBp).toBe(5000)
    expect(r.fits).toBe(true)
    expect(r.message).toBeNull()
  })

  it('fails with a message when the allocation exceeds free capacity', () => {
    const r = exclusionGroupFitsCapacity({ free_bp: 2500 }, 50)
    expect(r.requestedBp).toBe(5000)
    expect(r.fits).toBe(false)
    expect(r.message).toMatch(/exceeds/i)
  })
})

describe('countMembersByGroup', () => {
  it('tallies experiments per group, ignoring ungrouped ones', () => {
    const counts = countMembersByGroup([
      { exclusion_group_id: 'g1' },
      { exclusion_group_id: 'g1' },
      { exclusion_group_id: 'g2' },
      { exclusion_group_id: null },
      {},
    ])
    expect(counts).toEqual({ g1: 2, g2: 1 })
  })
})

// ─── CapacityBar render ──────────────────────────────────────────────────────

describe('CapacityBar', () => {
  it('renders allocated + free percentages and member count', () => {
    const html = renderToString(<CapacityBar group={GROUP} memberCount={3} />)
    expect(html).toMatch(/25%/)
    expect(html).toMatch(/75%/)
    expect(html).toMatch(/3 members/)
  })

  it('renders a singular "1 member" label', () => {
    const html = renderToString(<CapacityBar group={GROUP} memberCount={1} />)
    expect(html).toMatch(/1 member<\/span>|1 member/)
  })

  it('exposes the allocated percent via aria-valuenow', () => {
    const html = renderToString(<CapacityBar group={GROUP} memberCount={0} />)
    expect(html).toMatch(/aria-valuenow="25"/)
    expect(html).toMatch(/role="progressbar"/)
  })
})

// ─── Module shape ────────────────────────────────────────────────────────────

describe('ExclusionGroups (module shape)', () => {
  it('lists groups via listExclusionGroups', () => {
    expect(LIST_SOURCE).toMatch(/listExclusionGroups/)
  })

  it('renders a CapacityBar per group', () => {
    expect(LIST_SOURCE).toMatch(/CapacityBar/)
  })

  it('opens the create + edit modal', () => {
    expect(LIST_SOURCE).toMatch(/ExclusionGroupModal/)
  })

  it('supports delete via deleteExclusionGroup', () => {
    expect(LIST_SOURCE).toMatch(/deleteExclusionGroup/)
  })
})

describe('ExclusionGroupModal (module shape)', () => {
  it('creates via createExclusionGroup', () => {
    expect(MODAL_SOURCE).toMatch(/createExclusionGroup/)
  })

  it('edits via updateExclusionGroup (passing version)', () => {
    expect(MODAL_SOURCE).toMatch(/updateExclusionGroup/)
    expect(MODAL_SOURCE).toMatch(/version: group\.version/)
  })

  it('validates with the exclusionGroupSchema', () => {
    expect(MODAL_SOURCE).toMatch(/exclusionGroupSchema/)
  })
})
