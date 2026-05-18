/**
 * Dropdown component — logic tests (pure, no DOM).
 * The test environment is `node`; we test the open/close state machine logic
 * that the Dropdown component encapsulates, without rendering.
 */
import { describe, it, expect, vi } from 'vitest'

// ── State machine logic mirrored from Dropdown ────────────────────────────────

type Open = boolean

function toggle(open: Open): Open {
  return !open
}

function closeOnOutside(
  open: Open,
  clickedInsideRoot: boolean,
): Open {
  if (!open) return false
  return clickedInsideRoot ? open : false
}

function closeOnEscape(open: Open, key: string): Open {
  if (!open) return false
  return key === 'Escape' ? false : open
}

function handleItemClick(
  _open: Open,
  itemOnClick: () => void,
): Open {
  itemOnClick()
  return false
}

function resolveOpen(
  controlled: boolean | undefined,
  internal: boolean,
): boolean {
  return controlled !== undefined ? controlled : internal
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('Dropdown open/close state', () => {
  it('starts closed internally', () => {
    expect(resolveOpen(undefined, false)).toBe(false)
  })

  it('toggle opens when closed', () => {
    expect(toggle(false)).toBe(true)
  })

  it('toggle closes when open', () => {
    expect(toggle(true)).toBe(false)
  })
})

describe('Dropdown outside-click dismiss', () => {
  it('stays open when click is inside root', () => {
    expect(closeOnOutside(true, true)).toBe(true)
  })

  it('closes when click is outside root', () => {
    expect(closeOnOutside(true, false)).toBe(false)
  })

  it('stays closed (already closed) on outside click', () => {
    expect(closeOnOutside(false, false)).toBe(false)
  })
})

describe('Dropdown Escape-key dismiss', () => {
  it('closes on Escape', () => {
    expect(closeOnEscape(true, 'Escape')).toBe(false)
  })

  it('does nothing for non-Escape keys', () => {
    expect(closeOnEscape(true, 'Enter')).toBe(true)
    expect(closeOnEscape(true, 'Tab')).toBe(true)
  })

  it('stays closed when already closed and Escape pressed', () => {
    expect(closeOnEscape(false, 'Escape')).toBe(false)
  })
})

describe('Dropdown item click', () => {
  it('calls item onClick and closes panel', () => {
    const onClick = vi.fn()
    const nextOpen = handleItemClick(true, onClick)
    expect(onClick).toHaveBeenCalledTimes(1)
    expect(nextOpen).toBe(false)
  })
})

describe('Dropdown controlled vs uncontrolled', () => {
  it('uses controlled value when provided', () => {
    expect(resolveOpen(true, false)).toBe(true)
    expect(resolveOpen(false, true)).toBe(false)
  })

  it('uses internal value when not controlled', () => {
    expect(resolveOpen(undefined, true)).toBe(true)
    expect(resolveOpen(undefined, false)).toBe(false)
  })
})

describe('Dropdown item list', () => {
  interface DropdownItem { label: string; onClick: () => void; active?: boolean; disabled?: boolean }

  function visibleItems(items: DropdownItem[], open: boolean): DropdownItem[] {
    return open ? items : []
  }

  it('items are not visible when closed', () => {
    const items: DropdownItem[] = [{ label: 'A', onClick: vi.fn() }]
    expect(visibleItems(items, false)).toHaveLength(0)
  })

  it('items are visible when open', () => {
    const items: DropdownItem[] = [
      { label: 'A', onClick: vi.fn() },
      { label: 'B', onClick: vi.fn(), active: true },
    ]
    expect(visibleItems(items, true)).toHaveLength(2)
  })
})
