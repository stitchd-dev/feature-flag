# Track Revisions: domain_20260411

## Revision 1 — 2026-04-11

**Type:** Spec/Plan  
**Triggered by:** Implementing PgUserRepository (Phase 3, Task 5)  
**Phase/Task:** Phase 3, Task 5

**What triggered the revision:**  
When implementing `PgUserRepository`, the `User` and `Role` domain structs in
`stitchd-core` were found to be missing fields present in the database schema:

- `User` missing: `password_hash: String`, `deleted_at: Option<DateTime<Utc>>`,
  `version: i64`
- `Role` missing: `created_at: DateTime<Utc>`, `updated_at: DateTime<Utc>`,
  `deleted_at: Option<DateTime<Utc>>`, `version: i64`

The `create(user: &User)` trait method cannot insert a user without `password_hash`
since the column is `NOT NULL`.

**Changes made:**  
- Added `password_hash`, `deleted_at`, `version` to `User` in
  `crates/stitchd-core/src/user.rs`
- Added `created_at`, `updated_at`, `deleted_at`, `version` to `Role` in the same
  file
- Updated spec to reflect these fields

**Rationale:**  
`User` and `Role` were under-specified relative to the migration. All other
aggregates (Organisation, Project, Environment) already carry `deleted_at` and
`version`. Consistency requires the same for User and Role.
