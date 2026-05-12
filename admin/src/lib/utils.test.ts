import { describe, it, expect } from 'vitest'
import { slugify } from './utils'

describe('slugify', () => {
  it('lowercases input', () => {
    expect(slugify('Hello World')).toBe('hello-world')
  })

  it('replaces spaces with dashes', () => {
    expect(slugify('my flag name')).toBe('my-flag-name')
  })

  it('replaces special characters with dashes', () => {
    expect(slugify('flag@2.0!')).toBe('flag-2-0')
  })

  it('collapses multiple separators into one dash', () => {
    expect(slugify('foo  --  bar')).toBe('foo-bar')
  })

  it('strips leading and trailing dashes', () => {
    expect(slugify('  hello  ')).toBe('hello')
    expect(slugify('---flag---')).toBe('flag')
  })

  it('handles already-clean keys', () => {
    expect(slugify('my-feature-flag')).toBe('my-feature-flag')
  })

  it('returns empty string for blank input', () => {
    expect(slugify('')).toBe('')
    expect(slugify('   ')).toBe('')
  })
})
