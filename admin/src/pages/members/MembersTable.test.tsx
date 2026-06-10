/**
 * MembersTable — presentational render tests (members_roles_20260610, Phase 2/3).
 * Admin vitest env is `node`: render to string and assert on HTML shape.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import { MembersTable } from './MembersTable'
import type { OrgMemberSummary } from '../../lib/api'

const members: OrgMemberSummary[] = [
  { user_id: 'u1', email: 'priya@x.com', display_name: 'Priya Reddy', role: 'org_admin', created_at: '2026-06-01T00:00:00Z' },
  { user_id: 'u2', email: 'devon@x.com', display_name: 'Devon Hayes', role: 'org_member', created_at: '2026-06-02T00:00:00Z' },
]

describe('MembersTable', () => {
  it('renders one row per member with name, email and role badge', () => {
    const html = renderToString(<MembersTable members={members} canManage={false} onRemove={() => {}} />)
    expect(html).toMatch(/Priya Reddy/)
    expect(html).toMatch(/priya@x\.com/)
    expect(html).toMatch(/Devon Hayes/)
    expect(html).toMatch(/Admin/)
    expect(html).toMatch(/Member/)
  })

  it('renders deterministic avatar initials', () => {
    const html = renderToString(<MembersTable members={members} canManage={false} onRemove={() => {}} />)
    expect(html).toMatch(/>PR</)
    expect(html).toMatch(/>DH</)
  })

  it('hides the remove control when the viewer cannot manage members', () => {
    const html = renderToString(<MembersTable members={members} canManage={false} onRemove={() => {}} />)
    expect(html).not.toMatch(/Remove member/)
  })

  it('shows a remove control per row when the viewer can manage members', () => {
    const html = renderToString(<MembersTable members={members} canManage={true} onRemove={() => {}} />)
    expect(html).toMatch(/Remove member/)
  })
})
