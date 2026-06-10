# Implementation Plan: Members & Roles — Real Data

Track: `members_roles_20260610` · Beads epic: TBD · Branch: `track/members_roles_20260610`

Methodology: TDD (Red→Green→Refactor) per `conductor/workflow.md`. Frontend gates:
`npm run` `tsc` (typecheck), `lint`, `test` (vitest, `CI=true`), `build`.

## Phase 1: Backend audit + management API client

- [x] Task 1.1: Audit backend member/role/invite surface. Confirm management
      routes (`GET/POST /v1/management/orgs/{org_id}/users`, `DELETE .../{user_id}`)
      and their JSON shapes; determine whether any role-change, pending-invite, or
      custom-role-definition RPC exists. Record findings in `learnings.md` (drives
      FR5/FR7 decisions). No code change — investigation task.
- [x] Task 1.2 (TDD): Write failing vitest tests for new client functions
      `listOrgMembers` / `createOrgMember` / `removeOrgMember` in
      `admin/src/lib/api.test.ts` (mock axios; assert correct method, URL, body,
      cursor handling). Confirm red.
      <!-- files: admin/src/lib/api.test.ts -->
- [x] Task 1.3 (Green): Implement the three functions + `OrgMemberSummary` type in
      `admin/src/lib/api.ts` against the management routes. Tests green.
      <!-- files: admin/src/lib/api.ts -->
- [x] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Members tab — real data

- [x] Task 2.1 (TDD): Write failing tests for the Members tab: loading state,
      error banner, empty state, and a rendered list of real members (mock the
      client). Place in `admin/src/pages/members/Members.test.tsx`.
      <!-- files: admin/src/pages/members/Members.test.tsx -->
- [x] Task 2.2 (Green): Extract `Members` out of `stubs.tsx` into
      `admin/src/pages/members/Members.tsx`; fetch via `listOrgMembers(orgId)`;
      render name/email/role-badge/joined with deterministic avatar initials;
      loading/empty/error states; real tab count. Update the route import in
      `admin/src/App.tsx`. Tests green.
      <!-- files: admin/src/pages/members/Members.tsx, admin/src/App.tsx, admin/src/pages/stubs.tsx -->
- [x] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Invite + remove member

- [x] Task 3.1 (TDD): Failing tests for the invite modal (validation, submit →
      createOrgMember → refresh + toast, error surfacing) and remove flow (confirm
      dialog → removeOrgMember → row removed, error surfacing).
      <!-- files: admin/src/pages/members/Members.test.tsx -->
- [x] Task 3.2 (Green): Implement invite modal (email/display_name/password/role,
      Yup validation) and row remove action with confirmation. Wire to client
      functions; refresh list on success; PermissionGate the write controls.
      Remove the non-functional "Bulk invite" button. Tests green.
      <!-- files: admin/src/pages/members/Members.tsx -->
- [x] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: SSO providers tab

- [x] Task 4.1 (TDD): Failing tests for the SSO tab: list providers, create
      (OIDC + SAML forms), edit, delete (confirm), SAML metadata download; states.
      <!-- files: admin/src/pages/members/SsoProviders.test.tsx -->
- [x] Task 4.2 (Green): Implement the SSO tab as `admin/src/pages/members/SsoProviders.tsx`
      using the existing `listAuthProviders`/`createAuthProvider`/`updateAuthProvider`/
      `deleteAuthProvider`/`getSamlSpMetadata` client functions; OIDC + SAML config
      forms; download-metadata action for SAML; mount it under the Members page SSO
      tab. Tests green.
      <!-- files: admin/src/pages/members/SsoProviders.tsx, admin/src/pages/members/Members.tsx -->
- [x] Task: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: Honest tabs, mock decommission, full verification

- [x] Task 5.1: Resolve Roles & Pending-invites tabs per the Phase-1 audit:
      remove tabs with no backend capability, or replace their content with a
      clear non-deceptive explanation. Remove the fake "custom role" card. No
      "Coming soon" placeholder remains.
      <!-- files: admin/src/pages/members/Members.tsx -->
- [x] Task 5.2: Remove `MEMBERS` (and any now-orphaned mock) from
      `admin/src/lib/mockData.ts`; confirm no remaining importers; delete the
      `Members` export from `stubs.tsx` if fully migrated.
      <!-- files: admin/src/lib/mockData.ts, admin/src/pages/stubs.tsx -->
- [x] Task 5.3: Full frontend gate — `tsc`, `lint`, vitest (`CI=true`), `build`
      all green. Fix any fallout. Update `learnings.md` with final notes + any
      backend-gap follow-ups filed in beads.
- [x] Task: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)
