/**
 * Environments page — sidebar-refresh wiring contract (feature-flag-42f).
 *
 * Bug: after the user created the first environment on /environments,
 * the sidebar EnvSwitcher kept showing "● No environment MANAGE" until a
 * hard refresh because the page never invalidated the OrgContext-owned
 * env list the sidebar subscribed to.
 *
 * Fix: the page reads `refreshEnvironments` from `useOrgContext()` and
 * calls it after every create/rename/delete. The sidebar consumes the
 * same OrgContext, so the chip updates in the same render cycle.
 *
 * Admin vitest env is `environment: 'node'` (no jsdom). We audit the
 * Environments.tsx source via Vite's `?raw` loader.
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './Environments.tsx?raw'

describe('Environments page — sidebar-refresh wiring', () => {
  it('destructures refreshEnvironments from useOrgContext()', () => {
    expect(SOURCE).toMatch(/refreshEnvironments\b[\s\S]*?=\s*useOrgContext\(\)/)
  })

  it('invokes refreshEnvironments() after create / rename / delete', () => {
    // Spot-check each mutation handler calls the refresh.
    const createBlock = SOURCE.match(/handleCreate[\s\S]+?\n\s{2}(?:async\s+)?function\s+handleRename/) ?? []
    expect(createBlock.length).toBeGreaterThan(0)
    expect(createBlock[0]).toMatch(/refreshEnvironments\(\)/)

    const renameBlock = SOURCE.match(/handleRename[\s\S]+?\n\s{2}(?:async\s+)?function\s+handleDelete/) ?? []
    expect(renameBlock.length).toBeGreaterThan(0)
    expect(renameBlock[0]).toMatch(/refreshEnvironments\(\)/)

    const deleteBlock = SOURCE.match(/handleDelete[\s\S]+?\n\s{2}\}\n/) ?? []
    expect(deleteBlock.length).toBeGreaterThan(0)
    expect(deleteBlock[0]).toMatch(/refreshEnvironments\(\)/)
  })
})
