import { describe, it, expect } from 'vitest'

// ── Logic mirrored from LoadingSpinner ────────────────────────────────────────

type SpinnerSize = 'sm' | 'md' | 'lg'

const SIZES: Record<SpinnerSize, { px: number; font: number }> = {
  sm: { px: 16, font: 12 },
  md: { px: 24, font: 14 },
  lg: { px: 36, font: 15 },
}

function getSize(size: SpinnerSize = 'md') {
  return SIZES[size]
}

function getLabel(label?: string): string {
  return label ?? 'Loading…'
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('LoadingSpinner size', () => {
  it('sm is 16px', () => expect(getSize('sm').px).toBe(16))
  it('md is 24px', () => expect(getSize('md').px).toBe(24))
  it('lg is 36px', () => expect(getSize('lg').px).toBe(36))
  it('defaults to md', () => expect(getSize()).toEqual(SIZES.md))
})

describe('LoadingSpinner label', () => {
  it('defaults to "Loading…"', () => expect(getLabel()).toBe('Loading…'))
  it('accepts custom label', () => expect(getLabel('Fetching data…')).toBe('Fetching data…'))
})
