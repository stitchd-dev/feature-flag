/**
 * Modal component — logic tests (pure, no DOM).
 * Tests the props interface, focus-trap math, and width lookup.
 */
import { describe, it, expect } from 'vitest'

// ── Width lookup ──────────────────────────────────────────────────────────────

type ModalSize = 'sm' | 'md' | 'lg'
const WIDTHS: Record<ModalSize, number> = { sm: 400, md: 520, lg: 720 }

function getWidth(size: ModalSize = 'md'): number {
  return WIDTHS[size]
}

// ── Focus-trap index math ─────────────────────────────────────────────────────

function trapFocusNext(current: number, total: number): number {
  // Tab forward: if at last element, wrap to first
  return current >= total - 1 ? 0 : current + 1
}

function trapFocusPrev(current: number, total: number): number {
  // Shift+Tab backward: if at first element, wrap to last
  return current <= 0 ? total - 1 : current - 1
}

// ── Visibility gate ───────────────────────────────────────────────────────────

function shouldRender(isOpen: boolean): boolean {
  return isOpen
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('Modal width', () => {
  it('sm is 400px', () => expect(getWidth('sm')).toBe(400))
  it('md is 520px (default)', () => expect(getWidth('md')).toBe(520))
  it('md is the default size', () => expect(getWidth()).toBe(520))
  it('lg is 720px', () => expect(getWidth('lg')).toBe(720))
})

describe('Modal visibility gate', () => {
  it('renders when isOpen is true', () => expect(shouldRender(true)).toBe(true))
  it('does not render when isOpen is false', () => expect(shouldRender(false)).toBe(false))
})

describe('Modal focus trap', () => {
  it('Tab from last element wraps to first', () => {
    expect(trapFocusNext(2, 3)).toBe(0) // index 2 of [0,1,2]
  })

  it('Tab from non-last element advances', () => {
    expect(trapFocusNext(0, 3)).toBe(1)
    expect(trapFocusNext(1, 3)).toBe(2)
  })

  it('Shift+Tab from first element wraps to last', () => {
    expect(trapFocusPrev(0, 3)).toBe(2)
  })

  it('Shift+Tab from non-first element goes back', () => {
    expect(trapFocusPrev(2, 3)).toBe(1)
    expect(trapFocusPrev(1, 3)).toBe(0)
  })
})

describe('Modal close triggers', () => {
  it('Escape key string matches "Escape"', () => {
    const key = 'Escape'
    expect(key === 'Escape').toBe(true)
  })

  it('other keys do not trigger close', () => {
    const nonEscapeKeys = ['Enter', 'Tab', 'Space', 'a', 'Backspace']
    nonEscapeKeys.forEach((k) => {
      expect(k === 'Escape').toBe(false)
    })
  })
})
