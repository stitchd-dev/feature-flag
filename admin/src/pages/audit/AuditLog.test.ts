/**
 * AuditLog — source-contract test (audit_log_20260611).
 *
 * The page is data-driven (useEffect via usePaginatedList), so we pin the
 * contract with a `?raw` source assertion: it consumes the real listAuditLog
 * API and renders no fabricated audit data.
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './AuditLog.tsx?raw'

describe('AuditLog page — real data, no mock', () => {
  it('uses the real listAuditLog API', () => {
    expect(SOURCE).toMatch(/listAuditLog/)
  })

  it('does not import mock data', () => {
    expect(SOURCE).not.toMatch(/from\s+['"][^'"]*mockData['"]/)
  })

  it('does not contain the fabricated audit fixtures', () => {
    expect(SOURCE).not.toMatch(/Priya Reddy/)
    expect(SOURCE).not.toMatch(/rollout 20% → 30%/)
  })

  it('paginates and filters via real controls', () => {
    expect(SOURCE).toMatch(/usePaginatedList/)
    expect(SOURCE).toMatch(/resource_type/)
  })
})
