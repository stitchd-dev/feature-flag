/**
 * Yup schema tests for the cross-context percentage-hash selector list.
 *
 * The schema enforces the same three rules as the gateway's
 * `validate_hash_inputs` in `crates/stitchd-gateway/src/routes/flags.rs`:
 *
 *   1. The selector array must be non-empty.
 *   2. `ContextParameter` selectors require a non-empty `parameter`.
 *   3. No two selectors share the same `(context_type, field)` identity.
 *
 * The shape is the camel-case TypeScript mirror of the wire DTO
 * (`HashSelectorJson`). The wire conversion happens at the API boundary.
 */
import { describe, it, expect } from 'vitest'
import { hashInputsSchema } from './hashInputSchema'
import type { HashSelector } from '../hashInputTypes'

describe('hashInputsSchema — non-empty', () => {
  it('rejects an empty array', () => {
    expect(hashInputsSchema.isValidSync([])).toBe(false)
  })

  it('accepts a single ContextKey selector', () => {
    const sel: HashSelector[] = [{ kind: 'context_key', context_type: 'user' }]
    expect(hashInputsSchema.isValidSync(sel)).toBe(true)
  })

  it('accepts a single ContextParameter selector with non-empty parameter', () => {
    const sel: HashSelector[] = [
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ]
    expect(hashInputsSchema.isValidSync(sel)).toBe(true)
  })
})

describe('hashInputsSchema — ContextParameter requires non-empty parameter', () => {
  it('rejects ContextParameter with empty parameter', () => {
    const sel: HashSelector[] = [
      { kind: 'context_parameter', context_type: 'user', parameter: '' },
    ]
    expect(hashInputsSchema.isValidSync(sel)).toBe(false)
  })

  it('rejects ContextParameter with whitespace-only parameter', () => {
    const sel: HashSelector[] = [
      { kind: 'context_parameter', context_type: 'user', parameter: '   ' },
    ]
    expect(hashInputsSchema.isValidSync(sel)).toBe(false)
  })

  it('rejects ContextKey with empty context_type', () => {
    const sel: HashSelector[] = [{ kind: 'context_key', context_type: '' }]
    expect(hashInputsSchema.isValidSync(sel)).toBe(false)
  })

  it('rejects ContextParameter with empty context_type', () => {
    const sel: HashSelector[] = [
      { kind: 'context_parameter', context_type: '', parameter: 'os' },
    ]
    expect(hashInputsSchema.isValidSync(sel)).toBe(false)
  })
})

describe('hashInputsSchema — uniqueness', () => {
  it('rejects two ContextKey selectors with the same context_type', () => {
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_key', context_type: 'user' },
    ]
    expect(hashInputsSchema.isValidSync(sel)).toBe(false)
  })

  it('rejects two ContextParameter selectors with the same (context_type, parameter)', () => {
    const sel: HashSelector[] = [
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ]
    expect(hashInputsSchema.isValidSync(sel)).toBe(false)
  })

  it('accepts ContextKey + ContextParameter on the same context_type', () => {
    // Different `field` slot — `__key__` sentinel vs parameter name —
    // so these are distinct selectors per the gateway's `identity()`.
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'user', parameter: 'name' },
    ]
    expect(hashInputsSchema.isValidSync(sel)).toBe(true)
  })

  it('accepts the worked-example selector list verbatim', () => {
    // From spec.md FR-9: the live worked-example string in the helper banner.
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'user', parameter: 'name' },
      { kind: 'context_key', context_type: 'device' },
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
      { kind: 'context_key', context_type: 'application' },
    ]
    expect(hashInputsSchema.isValidSync(sel)).toBe(true)
  })
})

describe('hashInputsSchema — error messages', () => {
  it('reports a duplicate selector error path', () => {
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_key', context_type: 'user' },
    ]
    try {
      hashInputsSchema.validateSync(sel, { abortEarly: false })
      throw new Error('expected validation to fail')
    } catch (e) {
      const err = e as { errors: string[] }
      expect(err.errors.some((m) => /duplicate/i.test(m))).toBe(true)
    }
  })

  it('reports a missing-parameter error', () => {
    const sel: HashSelector[] = [
      { kind: 'context_parameter', context_type: 'user', parameter: '' },
    ]
    try {
      hashInputsSchema.validateSync(sel, { abortEarly: false })
      throw new Error('expected validation to fail')
    } catch (e) {
      const err = e as { errors: string[] }
      expect(err.errors.some((m) => /parameter/i.test(m))).toBe(true)
    }
  })

  it('reports a non-empty error for an empty array', () => {
    try {
      hashInputsSchema.validateSync([], { abortEarly: false })
      throw new Error('expected validation to fail')
    } catch (e) {
      const err = e as { errors: string[] }
      expect(err.errors.some((m) => /at least one/i.test(m))).toBe(true)
    }
  })
})
