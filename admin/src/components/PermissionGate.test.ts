/**
 * PermissionGate — logic tests (pure, no DOM).
 * Tests the allow/deny decision and loading-state behavior.
 */
import { describe, it, expect } from 'vitest'

// ── Access decision logic ─────────────────────────────────────────────────────

function checkPermission(permissions: string[], required: string): boolean {
  return permissions.includes(required)
}

type GateResult = 'loading' | 'allowed' | 'denied'

function gateResult(
  loading: boolean,
  permissions: string[],
  required: string,
): GateResult {
  if (loading) return 'loading'
  return checkPermission(permissions, required) ? 'allowed' : 'denied'
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('PermissionGate allow/deny', () => {
  const perms = ['flag:read', 'flag:write', 'project:create']

  it('allows when permission is present', () => {
    expect(gateResult(false, perms, 'flag:read')).toBe('allowed')
    expect(gateResult(false, perms, 'flag:write')).toBe('allowed')
  })

  it('denies when permission is absent', () => {
    expect(gateResult(false, perms, 'flag:delete')).toBe('denied')
    expect(gateResult(false, [], 'flag:read')).toBe('denied')
  })

  it('returns loading while permissions are loading', () => {
    expect(gateResult(true, perms, 'flag:read')).toBe('loading')
    expect(gateResult(true, [], 'flag:read')).toBe('loading')
  })
})

describe('PermissionGate exact string match', () => {
  it('is case-sensitive', () => {
    expect(checkPermission(['flag:read'], 'FLAG:READ')).toBe(false)
    expect(checkPermission(['flag:read'], 'flag:read')).toBe(true)
  })

  it('does not match partial strings', () => {
    expect(checkPermission(['flag:read'], 'flag')).toBe(false)
    expect(checkPermission(['flag:read'], 'read')).toBe(false)
  })
})
