/**
 * Pure helper tests for the Members page (members_roles_20260610, Phase 2/3).
 * Admin vitest env is `node`, so we test pure functions + the Yup schema here.
 */
import { describe, it, expect } from 'vitest'
import { memberInitials, roleBadge, ROLE_MODEL, addMemberSchema } from './membersHelpers'

describe('memberInitials', () => {
  it('uses the first letters of a two-word display name', () => {
    expect(memberInitials('Priya Reddy', 'p@x.com')).toBe('PR')
  })

  it('uses the first two letters of a single-word name', () => {
    expect(memberInitials('Madonna', 'm@x.com')).toBe('MA')
  })

  it('falls back to the email when the name is blank', () => {
    expect(memberInitials('', 'alice@example.com')).toBe('AL')
  })

  it('always returns uppercase', () => {
    expect(memberInitials('lin tan', 'l@x.com')).toBe('LT')
  })
})

describe('roleBadge', () => {
  it('maps org_admin to an Admin badge with an accent class', () => {
    const b = roleBadge('org_admin')
    expect(b.label).toBe('Admin')
    expect(b.className).toContain('accent')
  })

  it('maps org_member to a Member badge', () => {
    expect(roleBadge('org_member').label).toBe('Member')
  })

  it('shows an unknown role verbatim rather than hiding it', () => {
    expect(roleBadge('something_else').label).toBe('something_else')
  })
})

describe('ROLE_MODEL', () => {
  it('documents exactly the two real backend roles', () => {
    const keys = ROLE_MODEL.map((r) => r.role)
    expect(keys).toEqual(['org_admin', 'org_member'])
  })

  it('gives each role a human label and a non-empty capability summary', () => {
    for (const r of ROLE_MODEL) {
      expect(r.label.length).toBeGreaterThan(0)
      expect(r.summary.length).toBeGreaterThan(0)
    }
  })
})

describe('addMemberSchema', () => {
  const valid = {
    email: 'new@example.com',
    display_name: 'New Person',
    password: 'hunter2pass',
    org_role: 'org_member',
  }

  it('accepts a complete valid payload', async () => {
    await expect(addMemberSchema.validate(valid)).resolves.toBeTruthy()
  })

  it('rejects an invalid email', async () => {
    await expect(addMemberSchema.validate({ ...valid, email: 'nope' })).rejects.toThrow()
  })

  it('requires a display name', async () => {
    await expect(addMemberSchema.validate({ ...valid, display_name: '' })).rejects.toThrow()
  })

  it('requires a password of at least 8 characters', async () => {
    await expect(addMemberSchema.validate({ ...valid, password: 'short' })).rejects.toThrow()
  })

  it('only permits the two real org roles', async () => {
    await expect(addMemberSchema.validate({ ...valid, org_role: 'member' })).rejects.toThrow()
    await expect(addMemberSchema.validate({ ...valid, org_role: 'org_admin' })).resolves.toBeTruthy()
  })
})
