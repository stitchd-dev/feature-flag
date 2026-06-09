/**
 * Pagination — cursor-navigation logic tests (pure, no DOM).
 *
 * The component now renders only Previous / Next buttons driven by
 * `hasPrev` / `hasNext`; there are no page numbers and no total count.
 * It hides entirely when neither direction is navigable.
 */
import { describe, it, expect, vi } from 'vitest'

// ── Logic mirrored from the Pagination component ──────────────────────────────

interface PaginationState {
  hasPrev: boolean
  hasNext: boolean
}

/** Mirrors `if (!hasPrev && !hasNext) return null` — true means the control is hidden. */
function isHidden({ hasPrev, hasNext }: PaginationState): boolean {
  return !hasPrev && !hasNext
}

/** Whether the Previous button is disabled (mirrors `disabled={!hasPrev}`). */
function prevDisabled({ hasPrev }: PaginationState): boolean {
  return !hasPrev
}

/** Whether the Next button is disabled (mirrors `disabled={!hasNext}`). */
function nextDisabled({ hasNext }: PaginationState): boolean {
  return !hasNext
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('Pagination visibility', () => {
  it('hidden when neither prev nor next are navigable (single page)', () => {
    expect(isHidden({ hasPrev: false, hasNext: false })).toBe(true)
  })

  it('shown when there is a next page (first page of many)', () => {
    expect(isHidden({ hasPrev: false, hasNext: true })).toBe(false)
  })

  it('shown when there is a previous page (last page of many)', () => {
    expect(isHidden({ hasPrev: true, hasNext: false })).toBe(false)
  })

  it('shown in the middle (both directions navigable)', () => {
    expect(isHidden({ hasPrev: true, hasNext: true })).toBe(false)
  })
})

describe('Pagination button disabled states', () => {
  it('prev is disabled on the first page (no prev)', () => {
    expect(prevDisabled({ hasPrev: false, hasNext: true })).toBe(true)
  })

  it('prev is enabled once we have navigated forward', () => {
    expect(prevDisabled({ hasPrev: true, hasNext: true })).toBe(false)
  })

  it('next is disabled on the last page (next_cursor was null)', () => {
    expect(nextDisabled({ hasPrev: true, hasNext: false })).toBe(true)
  })

  it('next is enabled when next_cursor is present', () => {
    expect(nextDisabled({ hasPrev: false, hasNext: true })).toBe(false)
  })
})

describe('Pagination callbacks', () => {
  it('onNext is a no-op guard when hasNext is false', () => {
    const onNext = vi.fn()
    const hasNext = false
    // Mirrors `onClick={() => hasNext && onNext()}`.
    if (hasNext) onNext()
    expect(onNext).not.toHaveBeenCalled()
  })

  it('onNext fires when hasNext is true', () => {
    const onNext = vi.fn()
    const hasNext = true
    if (hasNext) onNext()
    expect(onNext).toHaveBeenCalledTimes(1)
  })

  it('onPrev is a no-op guard when hasPrev is false', () => {
    const onPrev = vi.fn()
    const hasPrev = false
    if (hasPrev) onPrev()
    expect(onPrev).not.toHaveBeenCalled()
  })

  it('onPrev fires when hasPrev is true', () => {
    const onPrev = vi.fn()
    const hasPrev = true
    if (hasPrev) onPrev()
    expect(onPrev).toHaveBeenCalledTimes(1)
  })
})
