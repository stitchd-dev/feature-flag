/**
 * usePermissions — module-level cache source contract (feature-flag-e7v).
 *
 * The admin vitest env is `environment: 'node'` (no jsdom), so we cannot
 * drive the React hook directly. We pin the cache contract by inspecting
 * the source via Vite's `?raw` loader.
 *
 * Acceptance gate: across a Login → Dashboard → Flags → Flag detail nav,
 * `/me/permissions` must fire at most twice (once on auth-state change,
 * once after an org switch). Before the cache it fired once per consumer
 * hook (5+ call sites).
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './usePermissions.ts?raw'

describe('usePermissions source contract', () => {
  it('declares a module-level cache so consumer calls do not duplicate', () => {
    // The current impl uses three module-level slots (token + promise + result).
    expect(SOURCE).toMatch(/let\s+cachedToken/)
    expect(SOURCE).toMatch(/let\s+cachedPromise/)
  })

  it('reuses an in-flight promise to fold concurrent fetches into one', () => {
    expect(SOURCE).toMatch(/cachedToken === token && cachedPromise/)
    expect(SOURCE).toMatch(/return cachedPromise/)
  })

  it('keys the cache by bearer token so org switch invalidates it', () => {
    // A token mismatch should reset and refetch.
    expect(SOURCE).toMatch(/cachedToken = token/)
  })

  it('clears the in-flight promise on fetch failure so retry can occur', () => {
    expect(SOURCE).toMatch(/cachedPromise = null/)
  })
})
