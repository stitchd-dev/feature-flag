/**
 * Members page — source-contract tests (members_roles_20260610).
 *
 * Admin vitest env is `node` and the page is data-driven via useEffect, so we
 * pin the contract with `?raw` source assertions plus a render of the static
 * RolesInfo tab. Interactive list/add/remove flows are covered by the API
 * wrapper tests (api.members.test.ts) and presentational tests (MembersTable).
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import MEMBERS_SRC from './Members.tsx?raw'
import { RolesInfo } from './RolesInfo'

describe('Members page — source contract', () => {
  it('uses the real management member API, not mock data', () => {
    expect(MEMBERS_SRC).toMatch(/listOrgMembers/)
    expect(MEMBERS_SRC).toMatch(/removeOrgMember/)
    expect(MEMBERS_SRC).not.toMatch(/mockData/)
    expect(MEMBERS_SRC).not.toMatch(/\bMEMBERS\b/)
  })

  it('contains no "coming soon" placeholder', () => {
    expect(MEMBERS_SRC.toLowerCase()).not.toContain('coming soon')
  })

  it('does not offer a fake bulk-invite control', () => {
    expect(MEMBERS_SRC.toLowerCase()).not.toContain('bulk invite')
  })

  it('labels the create action "Add member" (not "Invite") — no email-invite backend', () => {
    expect(MEMBERS_SRC).toMatch(/Add member/)
  })

  it('mounts the real SSO providers tab', () => {
    expect(MEMBERS_SRC).toMatch(/SsoProviders/)
  })
})

describe('RolesInfo tab', () => {
  it('documents the two real roles honestly', () => {
    const html = renderToString(<RolesInfo />)
    expect(html).toMatch(/org_admin/)
    expect(html).toMatch(/org_member/)
    expect(html).toMatch(/fixed two-role model/)
    expect(html.toLowerCase()).not.toContain('coming soon')
  })
})
