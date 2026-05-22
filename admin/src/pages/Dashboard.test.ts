/**
 * Dashboard — module-level invariants after feature-flag-tcy + zh9.
 *
 * No DOM / runtime rendering tests: the admin vitest env is `node` so we
 * assert on the source text and the typed module surface.
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './Dashboard.tsx?raw'

describe('Dashboard (page module)', () => {
  it('does not import the FLAGS mock array', () => {
    // feature-flag-tcy: the dashboard previously rendered hard-coded flags
    // from mockData. After the fix, real data must come from the API.
    expect(SOURCE).not.toMatch(/\bFLAGS\b/)
    expect(SOURCE).not.toMatch(/from\s+['"][^'"]*mockData['"]/)
  })

  it('does not render hard-coded service-health port table', () => {
    // feature-flag-zh9: the previous code shipped a static SERVICES array
    // with wrong port numbers. The card was removed entirely — verify no
    // residual port string survives in source.
    expect(SOURCE).not.toMatch(/:50051\b/)
    expect(SOURCE).not.toMatch(/:50052\b/)
    expect(SOURCE).not.toMatch(/:50053\b/)
    expect(SOURCE).not.toMatch(/:50054\b/)
    expect(SOURCE).not.toMatch(/:50055\b/)
    expect(SOURCE).not.toMatch(/:50056\b/)
  })

  it('does not render the "Acme Cloud" mock breadcrumb', () => {
    expect(SOURCE).not.toMatch(/Acme Cloud/)
    expect(SOURCE).not.toMatch(/stitchd-app/)
  })

  it('imports the real API + org context for data', () => {
    expect(SOURCE).toMatch(/listFlags\b/)
    expect(SOURCE).toMatch(/useOrgContext\b/)
  })
})
