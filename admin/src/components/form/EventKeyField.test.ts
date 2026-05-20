import { describe, expect, it } from 'vitest'
import { isKnownEventKey } from './EventKeyField'

// Pure-helper coverage for the picker's strict-mode predicate. The
// component itself is exercised end-to-end via Preview MCP; this file
// nails the trim+membership semantics in isolation so refactors can't
// silently regress them.

describe('isKnownEventKey', () => {
  const keys = ['checkout_started', 'checkout_completed', 'signup_completed']

  it('returns true for an exact-match key', () => {
    expect(isKnownEventKey('checkout_started', keys)).toBe(true)
  })

  it('returns false for a key not in the env', () => {
    expect(isKnownEventKey('purchase_completed', keys)).toBe(false)
  })

  it('trims surrounding whitespace before lookup', () => {
    expect(isKnownEventKey('  signup_completed  ', keys)).toBe(true)
  })

  it('treats an empty string as not-known', () => {
    // Empty isn't a *real* key — the picker handles "no value entered"
    // separately via the existing required-field validators.
    expect(isKnownEventKey('', keys)).toBe(false)
    expect(isKnownEventKey('   ', keys)).toBe(false)
  })

  it('is case-sensitive (matches server-side key semantics)', () => {
    // Event keys on the server are case-sensitive in the unique
    // constraint. Mirror that so the client doesn't accept a typo'd case.
    expect(isKnownEventKey('Checkout_Started', keys)).toBe(false)
  })

  it('returns false against an empty catalog', () => {
    expect(isKnownEventKey('checkout_started', [])).toBe(false)
  })
})
