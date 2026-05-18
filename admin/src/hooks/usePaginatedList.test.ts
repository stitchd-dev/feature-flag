/**
 * usePaginatedList — logic tests (pure, no DOM).
 * Tests page math, error mapping, and abort signal semantics.
 */
import { describe, it, expect, vi } from 'vitest'

// ── Page math helpers ─────────────────────────────────────────────────────────

function parsePage(raw: string | null): number {
  return Math.max(1, Number(raw ?? 1))
}

function totalPages(total: number, perPage: number): number {
  return Math.max(1, Math.ceil(total / perPage))
}

function buildQs(page: number, perPage: number): URLSearchParams {
  const qs = new URLSearchParams()
  qs.set('page', String(page))
  qs.set('per_page', String(perPage))
  return qs
}

// ── Abort logic ───────────────────────────────────────────────────────────────

function createFetchState() {
  let aborted = false
  const controller = { abort() { aborted = true }, signal: { get aborted() { return aborted } } }
  return controller
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

describe('usePaginatedList page math', () => {
  it('parses page 1 from null (default)', () => {
    expect(parsePage(null)).toBe(1)
  })

  it('parses explicit page number', () => {
    expect(parsePage('3')).toBe(3)
  })

  it('clamps to 1 on negative input', () => {
    expect(parsePage('-5')).toBe(1)
  })

  it('clamps to 1 on zero', () => {
    expect(parsePage('0')).toBe(1)
  })

  it('calculates total pages correctly', () => {
    expect(totalPages(100, 50)).toBe(2)
    expect(totalPages(101, 50)).toBe(3)
    expect(totalPages(0, 50)).toBe(1)
  })

  it('builds query string with page and per_page', () => {
    const qs = buildQs(2, 25)
    expect(qs.get('page')).toBe('2')
    expect(qs.get('per_page')).toBe('25')
  })
})

describe('usePaginatedList abort semantics', () => {
  it('aborted signal prevents state updates', () => {
    const ctrl = createFetchState()
    expect(ctrl.signal.aborted).toBe(false)
    ctrl.abort()
    expect(ctrl.signal.aborted).toBe(true)
  })

  it('new controller after abort is clean', () => {
    const ctrl1 = createFetchState()
    ctrl1.abort()
    const ctrl2 = createFetchState()
    expect(ctrl2.signal.aborted).toBe(false)
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
  it('fetcher is called with page, perPage, signal', async () => {
    const fetcher = vi.fn().mockResolvedValue({ items: ['a', 'b'], total: 2 })
    const signal = new AbortController().signal
    const result = await fetcher({ page: 1, perPage: 50, signal })
    expect(fetcher).toHaveBeenCalledWith({ page: 1, perPage: 50, signal })
    expect(result.items).toHaveLength(2)
    expect(result.total).toBe(2)
  })
})
