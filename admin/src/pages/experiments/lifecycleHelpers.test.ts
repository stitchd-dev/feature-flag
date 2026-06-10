/**
 * Pure tests for the experiment lifecycle state machine + timeline helpers
 * (experiment_lifecycle_ui_20260610, Phase 1/3).
 */
import { describe, it, expect } from 'vitest'
import { allowedTransitions, lifecycleTimeline } from './lifecycleHelpers'

describe('allowedTransitions', () => {
  it('draft → Start (active)', () => {
    const a = allowedTransitions('draft')
    expect(a.map((x) => x.target)).toEqual(['active'])
    expect(a[0].label).toBe('Start')
  })

  it('running → Pause + Conclude', () => {
    const a = allowedTransitions('running')
    expect(a.map((x) => x.target).sort()).toEqual(['concluded', 'paused'])
  })

  it('paused → Resume (active) + Conclude', () => {
    const a = allowedTransitions('paused')
    expect(a.map((x) => x.target).sort()).toEqual(['active', 'concluded'])
    expect(a.find((x) => x.target === 'active')?.label).toBe('Resume')
  })

  it('concluded → no transitions (terminal)', () => {
    expect(allowedTransitions('concluded')).toEqual([])
  })

  it('Conclude is marked destructive', () => {
    const conclude = allowedTransitions('running').find((x) => x.target === 'concluded')
    expect(conclude?.danger).toBe(true)
  })

  it('unknown status yields no actions (fail safe)', () => {
    expect(allowedTransitions('weird')).toEqual([])
  })
})

describe('lifecycleTimeline', () => {
  it('includes only stages with a real timestamp, in order', () => {
    const tl = lifecycleTimeline({
      created_at: '2026-06-01T00:00:00Z',
      started_at: '2026-06-02T00:00:00Z',
      ended_at: null,
      status: 'running',
    })
    const labels = tl.map((s) => s.label)
    expect(labels).toContain('Created')
    expect(labels).toContain('Started')
    expect(labels).not.toContain('Ended')
  })

  it('adds an Ended stage when ended_at is present', () => {
    const tl = lifecycleTimeline({
      created_at: '2026-06-01T00:00:00Z',
      started_at: '2026-06-02T00:00:00Z',
      ended_at: '2026-06-10T00:00:00Z',
      status: 'concluded',
    })
    expect(tl.map((s) => s.label)).toContain('Ended')
  })

  it('never invents actors or fake dates', () => {
    const tl = lifecycleTimeline({ created_at: '2026-06-01T00:00:00Z', started_at: null, ended_at: null, status: 'draft' })
    // Only the Created stage; each stage carries a real ISO timestamp.
    expect(tl).toHaveLength(1)
    expect(tl[0].at).toBe('2026-06-01T00:00:00Z')
  })
})
