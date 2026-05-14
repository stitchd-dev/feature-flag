import { describe, it, expect } from 'vitest'

// ─── Types (mirrored from PreviewTab.tsx) ────────────────────────────────────

export interface ContextParam {
  key: string
  value: string
}

export interface ContextCard {
  _type: string
  key: string
  params: ContextParam[]
}

// ─── Helpers under test ──────────────────────────────────────────────────────

/**
 * Parses a JSON string into an array of ContextCards.
 * Returns `{ contexts, error }` where error is non-null on invalid input.
 */
function parseContextsJson(json: string): { contexts: ContextCard[]; error: string | null } {
  let parsed: unknown
  try {
    parsed = JSON.parse(json)
  } catch {
    return { contexts: [], error: 'Invalid JSON: ' + (json.length > 40 ? json.slice(0, 40) + '…' : json) }
  }
  if (!Array.isArray(parsed)) {
    return { contexts: [], error: 'Expected a JSON array of context objects' }
  }
  const contexts: ContextCard[] = parsed.map((item: unknown) => {
    if (typeof item !== 'object' || item === null) {
      return { _type: '', key: '', params: [] }
    }
    const obj = item as Record<string, unknown>
    const _type = typeof obj['_type'] === 'string' ? obj['_type'] : ''
    const key = typeof obj['key'] === 'string' ? obj['key'] : ''
    const parameters = obj['parameters']
    const params: ContextParam[] =
      parameters && typeof parameters === 'object' && !Array.isArray(parameters)
        ? Object.entries(parameters as Record<string, unknown>).map(([k, v]) => ({
            key: k,
            value: String(v),
          }))
        : []
    return { _type, key, params }
  })
  return { contexts, error: null }
}

/**
 * Serialises an array of ContextCards back to a JSON string (pretty-printed).
 */
function contextsToJson(contexts: ContextCard[]): string {
  const arr = contexts.map((c) => {
    const parameters: Record<string, string> = {}
    for (const p of c.params) {
      if (p.key !== '') parameters[p.key] = p.value
    }
    return { _type: c._type, key: c.key, parameters }
  })
  return JSON.stringify(arr, null, 2)
}

/**
 * Validates a raw JSON string for use as the contexts input.
 * Returns an error message, or null if valid.
 */
function validateContextsJson(json: string): string | null {
  return parseContextsJson(json).error
}

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('parseContextsJson', () => {
  it('parses a single full context object', () => {
    const input = JSON.stringify([{ _type: 'user', key: 'alice', parameters: { plan: 'pro', region: 'us' } }])
    const { contexts, error } = parseContextsJson(input)
    expect(error).toBeNull()
    expect(contexts).toHaveLength(1)
    expect(contexts[0]._type).toBe('user')
    expect(contexts[0].key).toBe('alice')
    expect(contexts[0].params).toEqual([
      { key: 'plan', value: 'pro' },
      { key: 'region', value: 'us' },
    ])
  })

  it('parses multiple context objects', () => {
    const input = JSON.stringify([
      { _type: 'user', key: 'alice', parameters: {} },
      { _type: 'org', key: 'acme', parameters: { tier: 'enterprise' } },
    ])
    const { contexts, error } = parseContextsJson(input)
    expect(error).toBeNull()
    expect(contexts).toHaveLength(2)
    expect(contexts[1].params).toEqual([{ key: 'tier', value: 'enterprise' }])
  })

  it('parses object with empty parameters as empty params array', () => {
    const input = JSON.stringify([{ _type: 'device', key: 'iphone', parameters: {} }])
    const { contexts, error } = parseContextsJson(input)
    expect(error).toBeNull()
    expect(contexts[0].params).toEqual([])
  })

  it('handles missing parameters field gracefully', () => {
    const input = JSON.stringify([{ _type: 'user', key: 'bob' }])
    const { contexts, error } = parseContextsJson(input)
    expect(error).toBeNull()
    expect(contexts[0].params).toEqual([])
  })

  it('handles missing _type and key fields as empty strings', () => {
    const input = JSON.stringify([{ parameters: { x: '1' } }])
    const { contexts, error } = parseContextsJson(input)
    expect(error).toBeNull()
    expect(contexts[0]._type).toBe('')
    expect(contexts[0].key).toBe('')
  })

  it('returns error for invalid JSON', () => {
    const { contexts, error } = parseContextsJson('{not valid json')
    expect(contexts).toEqual([])
    expect(error).not.toBeNull()
    expect(error).toMatch(/Invalid JSON/)
  })

  it('returns error for non-array JSON value', () => {
    const { contexts, error } = parseContextsJson(JSON.stringify({ _type: 'user' }))
    expect(contexts).toEqual([])
    expect(error).toMatch(/array/)
  })

  it('handles array of non-object items by producing empty cards', () => {
    const { contexts, error } = parseContextsJson('[null, 42]')
    expect(error).toBeNull()
    expect(contexts).toHaveLength(2)
    expect(contexts[0]).toEqual({ _type: '', key: '', params: [] })
  })

  it('coerces non-string parameter values to strings', () => {
    const input = JSON.stringify([{ _type: 'org', key: 'acme', parameters: { count: 5, active: true } }])
    const { contexts, error } = parseContextsJson(input)
    expect(error).toBeNull()
    expect(contexts[0].params).toEqual([
      { key: 'count', value: '5' },
      { key: 'active', value: 'true' },
    ])
  })
})

describe('contextsToJson', () => {
  it('round-trips a single context back to identical JSON', () => {
    const original = [{ _type: 'user', key: 'alice', parameters: { plan: 'pro', region: 'us' } }]
    const json = JSON.stringify(original)
    const { contexts } = parseContextsJson(json)
    const roundTripped = JSON.parse(contextsToJson(contexts))
    expect(roundTripped).toEqual(original)
  })

  it('round-trips multiple contexts back to identical JSON', () => {
    const original = [
      { _type: 'user', key: 'alice', parameters: {} },
      { _type: 'org', key: 'acme', parameters: { tier: 'enterprise' } },
    ]
    const json = JSON.stringify(original)
    const { contexts } = parseContextsJson(json)
    const roundTripped = JSON.parse(contextsToJson(contexts))
    expect(roundTripped).toEqual(original)
  })

  it('omits params with empty key', () => {
    const contexts: ContextCard[] = [{ _type: 'user', key: 'alice', params: [{ key: '', value: 'ignored' }, { key: 'role', value: 'admin' }] }]
    const result = JSON.parse(contextsToJson(contexts))
    expect(result[0].parameters).toEqual({ role: 'admin' })
  })

  it('produces pretty-printed JSON', () => {
    const contexts: ContextCard[] = [{ _type: 'user', key: 'x', params: [] }]
    const json = contextsToJson(contexts)
    expect(json).toContain('\n')
    expect(json).toContain('  ')
  })

  it('adding a context card increases the JSON array length', () => {
    const before: ContextCard[] = [{ _type: 'user', key: 'alice', params: [] }]
    const after: ContextCard[] = [...before, { _type: 'org', key: 'acme', params: [] }]
    expect(JSON.parse(contextsToJson(before))).toHaveLength(1)
    expect(JSON.parse(contextsToJson(after))).toHaveLength(2)
  })

  it('removing a context card decreases the JSON array length', () => {
    const before: ContextCard[] = [
      { _type: 'user', key: 'alice', params: [] },
      { _type: 'org', key: 'acme', params: [] },
    ]
    const after = before.slice(0, 1)
    expect(JSON.parse(contextsToJson(after))).toHaveLength(1)
  })
})

describe('validateContextsJson', () => {
  it('returns null for valid JSON array', () => {
    expect(validateContextsJson('[{"_type":"user","key":"x","parameters":{}}]')).toBeNull()
  })

  it('returns error for invalid JSON', () => {
    expect(validateContextsJson('oops')).not.toBeNull()
  })

  it('returns error for non-array', () => {
    expect(validateContextsJson('{}')).not.toBeNull()
  })

  it('returns null for empty array', () => {
    expect(validateContextsJson('[]')).toBeNull()
  })
})
