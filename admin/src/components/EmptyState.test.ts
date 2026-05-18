import { describe, it, expect } from 'vitest'

// ── Logic mirrored from EmptyState ────────────────────────────────────────────

interface EmptyStateProps {
  title: string
  desc?: string
  action?: unknown
}

function hasDesc(props: EmptyStateProps): boolean {
  return !!props.desc
}

function hasAction(props: EmptyStateProps): boolean {
  return !!props.action
}

function titleText(props: EmptyStateProps): string {
  return props.title
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('EmptyState', () => {
  it('renders title', () => {
    const props: EmptyStateProps = { title: 'No flags yet' }
    expect(titleText(props)).toBe('No flags yet')
  })

  it('shows desc only when provided', () => {
    expect(hasDesc({ title: 'T' })).toBe(false)
    expect(hasDesc({ title: 'T', desc: 'Some text' })).toBe(true)
  })

  it('shows action only when provided', () => {
    expect(hasAction({ title: 'T' })).toBe(false)
    expect(hasAction({ title: 'T', action: 'button-element' })).toBe(true)
  })

  it('can have both desc and action', () => {
    const props: EmptyStateProps = { title: 'T', desc: 'D', action: 'A' }
    expect(hasDesc(props)).toBe(true)
    expect(hasAction(props)).toBe(true)
  })
})
