/**
 * ArchiveEventModal — logic tests (pure, no DOM).
 *
 * Mirrors `EventsList.test.ts` / `EditEventModal.test.ts`: tests the pure
 * URL builder and confirmation-state derivations; DOM-level rendering is
 * intentionally covered by the underlying Modal primitive tests.
 */
import { describe, it, expect } from 'vitest'

// ── Pure builders mirrored from ArchiveEventModal.tsx ──────────────────────────

/**
 * The DELETE URL for an event key, with proper URL encoding for keys that
 * contain dots / colons / hyphens.
 */
export function buildArchiveUrl(eventKey: string): string {
  return `/v1/events/${encodeURIComponent(eventKey)}`
}

/**
 * Confirm-button label derivation — switches to "Archiving…" while the
 * DELETE is in flight, matching the existing FormSubmit pattern.
 */
export function confirmLabel(submitting: boolean): string {
  return submitting ? 'Archiving…' : 'Archive'
}

/**
 * Whether the confirm button is disabled. Pure derivation so the
 * component test can assert the truth-table without rendering.
 */
export function confirmDisabled(submitting: boolean): boolean {
  return submitting
}

/**
 * Whether the cancel button is disabled. Same rule: don't let users back
 * out mid-DELETE because the click would race the response.
 */
export function cancelDisabled(submitting: boolean): boolean {
  return submitting
}

// ── Tests ──────────────────────────────────────────────────────────────────────

describe('ArchiveEventModal — buildArchiveUrl', () => {
  it('encodes simple keys', () => {
    expect(buildArchiveUrl('checkout_completed')).toBe('/v1/events/checkout_completed')
  })

  it('encodes keys with reserved URL characters', () => {
    expect(buildArchiveUrl('user:signup')).toBe('/v1/events/user%3Asignup')
    expect(buildArchiveUrl('a/b')).toBe('/v1/events/a%2Fb')
  })

  it('preserves dots and hyphens (both allowed by the schema, not encoded)', () => {
    expect(buildArchiveUrl('checkout.completed-v2')).toBe('/v1/events/checkout.completed-v2')
  })
})

describe('ArchiveEventModal — label & disabled-state derivations', () => {
  it('shows "Archive" when idle', () => {
    expect(confirmLabel(false)).toBe('Archive')
  })

  it('shows "Archiving…" when in flight', () => {
    expect(confirmLabel(true)).toBe('Archiving…')
  })

  it('confirm button disables while submitting', () => {
    expect(confirmDisabled(false)).toBe(false)
    expect(confirmDisabled(true)).toBe(true)
  })

  it('cancel button disables while submitting — prevents click-race with the DELETE response', () => {
    expect(cancelDisabled(false)).toBe(false)
    expect(cancelDisabled(true)).toBe(true)
  })
})
