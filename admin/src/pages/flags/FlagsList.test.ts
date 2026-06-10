/**
 * FlagsList — source-contract test (flags_list_honest_20260610).
 *
 * Admin vitest env is `node`; the page is data-driven, so we assert the source
 * no longer renders fabricated/empty placeholder columns on the list.
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './FlagsList.tsx?raw'

describe('FlagsList — no fabricated placeholder columns', () => {
  it('drops the 30d evals / Segments / Owner table headers', () => {
    expect(SOURCE).not.toMatch(/30d evals/)
    expect(SOURCE).not.toMatch(/<th>Segments<\/th>/)
    expect(SOURCE).not.toMatch(/<th>Owner<\/th>/)
  })

  it('no longer renders an empty-data Sparkline on the list', () => {
    expect(SOURCE).not.toMatch(/data=\{\[\]\}/)
  })

  it('no longer imports Sparkline', () => {
    expect(SOURCE).not.toMatch(/\bSparkline\b/)
  })

  it('still renders the real columns (Key/Type/Status/Updated)', () => {
    expect(SOURCE).toMatch(/<th>Key<\/th>/)
    expect(SOURCE).toMatch(/<th>Type<\/th>/)
    expect(SOURCE).toMatch(/<th>Status<\/th>/)
    expect(SOURCE).toMatch(/<th>Updated<\/th>/)
  })
})
