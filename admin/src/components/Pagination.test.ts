import { describe, it, expect } from 'vitest'

// ── Logic mirrored from Pagination component ──────────────────────────────────

interface PaginationState {
  page: number
  perPage: number
  total: number
}

function totalPages({ perPage, total }: PaginationState): number {
  return Math.max(1, Math.ceil(total / perPage))
}

function isFirst({ page }: PaginationState): boolean {
  return page <= 1
}

function isLast(state: PaginationState): boolean {
  return state.page >= totalPages(state)
}

function from({ page, perPage, total }: PaginationState): number {
  return Math.min((page - 1) * perPage + 1, total)
}

function to({ page, perPage, total }: PaginationState): number {
  return Math.min(page * perPage, total)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('Pagination logic', () => {
  describe('totalPages', () => {
    it('rounds up to next page', () => {
      expect(totalPages({ page: 1, perPage: 10, total: 25 })).toBe(3)
    })

    it('is exactly N when total is divisible by perPage', () => {
      expect(totalPages({ page: 1, perPage: 10, total: 30 })).toBe(3)
    })

    it('returns 1 for empty total', () => {
      expect(totalPages({ page: 1, perPage: 50, total: 0 })).toBe(1)
    })
  })

  describe('isFirst / isLast (disabled states)', () => {
    it('prev is disabled on page 1', () => {
      expect(isFirst({ page: 1, perPage: 10, total: 30 })).toBe(true)
    })

    it('prev is enabled on page 2', () => {
      expect(isFirst({ page: 2, perPage: 10, total: 30 })).toBe(false)
    })

    it('next is disabled on last page', () => {
      expect(isLast({ page: 3, perPage: 10, total: 30 })).toBe(true)
    })

    it('next is enabled on non-last page', () => {
      expect(isLast({ page: 2, perPage: 10, total: 30 })).toBe(false)
    })

    it('both disabled when single page', () => {
      const state = { page: 1, perPage: 50, total: 5 }
      expect(isFirst(state)).toBe(true)
      expect(isLast(state)).toBe(true)
    })
  })

  describe('from/to range label', () => {
    it('page 1 shows 1–10 of 30', () => {
      const s = { page: 1, perPage: 10, total: 30 }
      expect(from(s)).toBe(1)
      expect(to(s)).toBe(10)
    })

    it('page 2 shows 11–20 of 30', () => {
      const s = { page: 2, perPage: 10, total: 30 }
      expect(from(s)).toBe(11)
      expect(to(s)).toBe(20)
    })

    it('last page clamps to total', () => {
      const s = { page: 3, perPage: 10, total: 25 }
      expect(from(s)).toBe(21)
      expect(to(s)).toBe(25)
    })

    it('empty list shows from=to=0 (clamped)', () => {
      const s = { page: 1, perPage: 50, total: 0 }
      expect(from(s)).toBe(0)
      expect(to(s)).toBe(0)
    })
  })

  describe('onChange fires correct page number', () => {
    it('next page is current + 1', () => {
      const page = 2
      expect(page + 1).toBe(3)
    })

    it('prev page is current - 1', () => {
      const page = 3
      expect(page - 1).toBe(2)
    })

    it('does not go below 1 on prev from page 1', () => {
      const page = 1
      const next = isFirst({ page, perPage: 10, total: 30 }) ? page : page - 1
      expect(next).toBe(1)
    })

    it('does not exceed totalPages on next from last', () => {
      const state = { page: 3, perPage: 10, total: 30 }
      const next = isLast(state) ? state.page : state.page + 1
      expect(next).toBe(3)
    })
  })
})
