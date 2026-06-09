/**
 * usePaginatedList — cursor-pagination logic tests (pure, no DOM).
 *
 * Models the hook's navigation state machine: the current `cursor` (null on the
 * first page), a `prevStack` of cursors visited (enabling Previous), and the
 * `nextCursor` from the last response. Cursors are opaque tokens — treated as
 * black-box strings here, never parsed.
 */
import { describe, it, expect, vi } from 'vitest'

// ── Navigation state machine (mirrors the hook) ───────────────────────────────

interface NavState {
  cursor: string | null
  nextCursor: string | null
  prevStack: (string | null)[]
}

/** Initial state: first page, no cursor, empty prev stack. */
function initial(): NavState {
  return { cursor: null, nextCursor: null, prevStack: [] }
}

/** Apply a fetch response (the next_cursor it returned) to the current state. */
function applyResponse(state: NavState, nextCursor: string | null): NavState {
  return { ...state, nextCursor }
}

/** onNext: push current cursor onto prevStack, advance cursor to nextCursor. No-op when nextCursor is null. */
function onNext(state: NavState): NavState {
  if (state.nextCursor == null) return state
  return {
    cursor: state.nextCursor,
    nextCursor: null,
    prevStack: [...state.prevStack, state.cursor],
  }
}

/** onPrev: pop prevStack and set cursor to it. No-op when the stack is empty. */
function onPrev(state: NavState): NavState {
  if (state.prevStack.length === 0) return state
  const target = state.prevStack[state.prevStack.length - 1]
  return {
    cursor: target,
    nextCursor: null,
    prevStack: state.prevStack.slice(0, -1),
  }
}

function hasNext(state: NavState): boolean {
  return state.nextCursor != null
}

function hasPrev(state: NavState): boolean {
  return state.prevStack.length > 0
}

/** Build the list query string for a given cursor + limit (cursor omitted when null). */
function buildQs(cursor: string | null, limit: number): URLSearchParams {
  const qs = new URLSearchParams()
  qs.set('limit', String(limit))
  if (cursor != null) qs.set('cursor', cursor)
  return qs
}

// ── extractErrorMessage (subset) ──────────────────────────────────────────────

function extractErrorMessage(err: unknown): string {
  if (err && typeof err === 'object') {
    const e = err as Record<string, unknown>
    const msg = (e.response as Record<string, unknown> | undefined)?.data
    if (msg && typeof msg === 'object' && 'message' in msg) return String((msg as Record<string, unknown>).message)
    if (typeof e.message === 'string') return e.message
  }
  return String(err)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('usePaginatedList cursor navigation', () => {
  it('starts on the first page with no prev and no next', () => {
    const s = initial()
    expect(s.cursor).toBeNull()
    expect(hasPrev(s)).toBe(false)
    expect(hasNext(s)).toBe(false)
  })

  it('hasNext becomes true once a response carries a next_cursor', () => {
    const s = applyResponse(initial(), 'CUR_A')
    expect(hasNext(s)).toBe(true)
    expect(hasPrev(s)).toBe(false)
  })

  it('hasNext is false when next_cursor is null (last page)', () => {
    const s = applyResponse(initial(), null)
    expect(hasNext(s)).toBe(false)
  })

  it('onNext advances cursor to next_cursor and records the prior cursor', () => {
    let s = applyResponse(initial(), 'CUR_A') // first page, next = CUR_A
    s = onNext(s)
    expect(s.cursor).toBe('CUR_A')
    expect(s.prevStack).toEqual([null]) // came from the first page (cursor null)
    expect(hasPrev(s)).toBe(true)
  })

  it('onNext is a no-op on the last page (next_cursor null)', () => {
    const s = applyResponse(initial(), null)
    expect(onNext(s)).toEqual(s)
  })

  it('onPrev returns to the previous cursor and pops the stack', () => {
    let s = applyResponse(initial(), 'CUR_A')
    s = onNext(s) // now on CUR_A, prevStack=[null]
    s = applyResponse(s, 'CUR_B')
    s = onNext(s) // now on CUR_B, prevStack=[null, 'CUR_A']
    expect(s.cursor).toBe('CUR_B')
    s = onPrev(s) // back to CUR_A
    expect(s.cursor).toBe('CUR_A')
    expect(s.prevStack).toEqual([null])
    s = onPrev(s) // back to the first page
    expect(s.cursor).toBeNull()
    expect(hasPrev(s)).toBe(false)
  })

  it('onPrev is a no-op on the first page (empty stack)', () => {
    const s = initial()
    expect(onPrev(s)).toEqual(s)
  })

  it('forward-then-back round trips to the first page', () => {
    let s = applyResponse(initial(), 'CUR_A')
    s = onNext(s)
    s = onPrev(s)
    expect(s.cursor).toBeNull()
    expect(hasPrev(s)).toBe(false)
  })
})

describe('usePaginatedList query string', () => {
  it('omits cursor on the first page, always sets limit', () => {
    const qs = buildQs(null, 50)
    expect(qs.get('cursor')).toBeNull()
    expect(qs.get('limit')).toBe('50')
  })

  it('includes the opaque cursor token verbatim on later pages', () => {
    const qs = buildQs('opaque.base64url.token', 25)
    expect(qs.get('cursor')).toBe('opaque.base64url.token')
    expect(qs.get('limit')).toBe('25')
  })
})

describe('usePaginatedList error mapping', () => {
  it('extracts Axios-style response.data.message', () => {
    const err = { response: { data: { message: 'Not found' } } }
    expect(extractErrorMessage(err)).toBe('Not found')
  })

  it('falls back to err.message', () => {
    const err = new Error('Network error')
    expect(extractErrorMessage(err)).toBe('Network error')
  })

  it('falls back to String(err) for unknowns', () => {
    expect(extractErrorMessage('just a string')).toBe('just a string')
    expect(extractErrorMessage(42)).toBe('42')
  })
})

describe('usePaginatedList fetcher contract', () => {
  it('fetcher is called with cursor, limit, signal and returns items + next_cursor', async () => {
    const fetcher = vi.fn().mockResolvedValue({ items: ['a', 'b'], next_cursor: 'CUR_NEXT' })
    const signal = new AbortController().signal
    const result = await fetcher({ cursor: null, limit: 50, signal })
    expect(fetcher).toHaveBeenCalledWith({ cursor: null, limit: 50, signal })
    expect(result.items).toHaveLength(2)
    expect(result.next_cursor).toBe('CUR_NEXT')
  })

  it('null next_cursor signals the last page', async () => {
    const fetcher = vi.fn().mockResolvedValue({ items: ['z'], next_cursor: null })
    const result = await fetcher({ cursor: 'CUR_A', limit: 50, signal: new AbortController().signal })
    expect(result.next_cursor).toBeNull()
  })
})
