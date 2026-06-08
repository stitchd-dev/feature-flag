# Revisions: platform_hardening_20260608

Record of spec/plan revisions made during implementation.

---

## Revision #1 — 2026-06-08 — Type: Spec + Plan (FR-4 Cursor Pagination)

**Phase/task when raised:** Phase 4 (Cursor-Based Pagination Migration), during the
route + repo + UI sweep.

**Trigger:** The originally-specified FR-4.3 was a per-repo keyset rewrite across
**6 service repos + 11 proto RPCs** (`WHERE (sort_key, id) > cursor … LIMIT n+1`,
dropping `OFFSET` + `COUNT(*) OVER()`). Executing that as one coordinated breaking
change — proto + every service repo + every route + all Admin UI together — is
large and risky to land CI-green in a single pass, and it reverses the deliberate
`domain_boundaries_20260530` page-based canonical. Separately, the experiment-detail
sub-lists surfaced a conflict: the Admin UI's exposure-count stat reads the `total`
that the cursor `{items, next_cursor}` envelope omits.

**Changes made (spec):**

1. **FR-4.3 — keyset internals → opaque encoded-offset (deferred keyset).** The
   cursor **contract** (`?cursor=&limit=` → `{items, next_cursor}`) is now delivered
   via an opaque **encoded-offset** at the gateway over each service's existing
   `(offset, limit) → (items, total)` RPC (`CursorParams::offset()` +
   `CursorPage::from_offset`). **Zero proto/repo churn.** The true-keyset internal
   (drops `OFFSET`, swaps only the token payload from `{offset}` to `{sort_key, id}`
   — contract-preserving) is deferred to follow-up bead **`feature-flag-cj5`**. The
   `CursorPage::from_overfetch` primitive is already in place for it.

2. **FR-4 scope — top-level collections only.** Cursor applies to the 8 top-level
   resource lists (flags, experiments, segments, events, metrics, sdk-keys,
   org-users mgmt+admin, exclusion-groups). **Experiment-detail sub-lists**
   (`iterations`, `exposures`) intentionally stay **page-based** — they back
   numbered detail views and the exposure-count stat needs the `total` the cursor
   envelope drops. AC-7 + NFR-6 reworded accordingly.

**Rationale & impact:**
- Delivers the `product-guidelines.md`-mandated cursor **contract** end-to-end
  (gateway + Admin UI), CI-green, with **bounded risk** — no 6-repo/11-proto
  breaking rewrite in one pass.
- True-keyset perf (O(1) deep-page scans, concurrent-insert stability) is a pure
  internal optimization tracked as `feature-flag-cj5`; the REST contract does not
  change when it lands.
- Keeping detail sub-lists page-based preserves the exposure-count UI feature.
- Already reflected in `tech-stack.md` and `product.md` during implementation.

**Changes made (plan):**
- Added a "Last Revised" marker + a Phase 4 `SCOPE` note (top-level collections;
  sub-lists page-based; encoded-offset approach).
- **Task 3 reframed** from "migrate repo queries to keyset" → "cursor transport via
  opaque encoded-offset at the gateway over existing offset RPCs (no proto/repo
  change)". Tasks 1/2/4/5 descriptions tightened to the as-built; the manual
  verification meta-task marked complete (verified CI-green).
- Added a **`### Phase 4 — Deferred follow-up`** entry marking the true-keyset
  internals `[-] DEFERRED → feature-flag-cj5]` (contract-preserving; tracked
  separately so this track stays closed). **No new active task** added to this
  (completed) track.

---

## Revision #2 — 2026-06-09 — Type: Spec + Plan (FR-4.3 true keyset implemented)

**Phase/task when raised:** post-completion follow-up — the user directed implementing the deferred true-keyset internals (`feature-flag-cj5`) autonomously.

**Trigger:** Revision #1 delivered the cursor contract via an interim encoded-offset shim and deferred true keyset. The user asked to complete it now, with a **clean cutover** (not additive migration).

**Changes made (code, across 10 commits 56aa12d…28dc14e):**
- Shared `stitchd_db::KeysetCursor` (base64url(JSON({created_at, id}))) + `effective_limit`.
- 8 top-level list entities cut over OFFSET → keyset: proto (page/per_page/total → cursor/limit/next_cursor), repo `list_*_paginated` → `list_*_keyset`, service handlers, gateway routes (`CursorPage::from_token`), all repo mock impls, and tests (each with a multi-page exactly-once correctness test).
- Removed the interim encoded-offset shim (`OffsetCursor`, `CursorParams::offset()`, `CursorPage::from_offset()`).
- Internal unbounded "list-all" consumers (gateway dependency-graph) now page through the cursor.

**Changes made (spec/plan):** FR-4.3 marked Done (keyset); "Last Revised" → Revision #2; plan's deferred follow-up item → `[x]` with per-entity SHAs.

**Rationale & impact:** O(1) per-page scans + concurrent-insert stability. **REST contract unchanged** (opaque token) ⇒ Admin UI untouched. **Behavior note:** `org-users` list order changed from `email` to `(created_at, id)` (the only user-visible ordering change; all other entities already ordered by `created_at`). Verified CI-green: workspace clippy -D warnings, sqlx-check, fmt, docs idempotent, OpenAPI contract 23/120, stitchd-db + 6 services + analytics (156) all pass. `feature-flag-cj5` closed.
