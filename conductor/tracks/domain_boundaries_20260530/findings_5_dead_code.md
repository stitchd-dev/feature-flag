# Findings 1.5: Dead-Code Audit
*Date: 2026-05-30*

## Summary
- DELETE-NOW items: 6
- PROPOSE items: 4

---

## DELETE-NOW Items

### DC-001: `spawn_eval_log_write` — never called
- **File**: `crates/stitchd-flag-service/src/eval_log_writer.rs:33`
- **Type**: unused public function (and companion type alias `EvalContextRow` at line 25)
- **Evidence**: Zero call sites outside the defining file. The SDK eval-log write path in `sdk_backend.rs` calls `insert_eval_log_rows` directly (line 292). `service.rs` explicitly notes preview does NOT write eval logs (line 1153). `build_eval_log_rows` (line 74) is only called by `spawn_eval_log_write` in production and has no external callers.
- **Risk**: NONE — no call sites exist anywhere in the codebase.

### DC-002: `_use_user_status` suppressor + unused `UserStatus` import in `jwt.rs`
- **File**: `crates/stitchd-core/src/auth/jwt.rs:12` (import), `jwt.rs:123–124` (suppressor function)
- **Type**: unused import + dead-code suppressor
- **Evidence**: `UserStatus` is imported alongside `OrgRole` at line 12 but never used in `jwt.rs`. The comment at line 121 says it was added as a "clippy-clean unused import suppressor." Fix: remove `UserStatus` from the import list and delete the `_use_user_status` function.
- **Risk**: NONE — the type remains available from `stitchd-core::auth::types`.

### DC-003: `MockProviderRepo::with` — uncalled test helper
- **File**: `crates/stitchd-auth-service/src/auth_provider.rs:428–433`
- **Type**: unused test-scope function (marked `#[allow(dead_code)]`)
- **Evidence**: Zero call sites. All tests in the file use `MockProviderRepo::empty()` (lines 546, 589, 625, 681, 727, 769, 812). The `::with(p)` variant was prepared for tests never written.
- **Risk**: NONE — removing a dead variant of a test-only struct.

### DC-004: `make_stub_state_with_flag` + suppressor `_use_with_flag`
- **File**: `crates/stitchd-gateway/src/tests/helpers.rs:60–62` and `crates/stitchd-gateway/src/routes/flags.rs:1846–1848`
- **Type**: unused test utility + dead-code suppressor
- **Evidence**: `make_stub_state_with_flag` is a trivial alias for `make_stub_state()` (its body is literally `make_stub_state()`). No test function calls it — it is only referenced via the `_use_with_flag` no-op at `flags.rs:1847`. Comment at `flags.rs:1845` says "exported for other tests" but no other file imports or invokes it.
- **Risk**: NONE — both the helper and the suppressor can be removed together.

### DC-005: `compute_hash_percentage` — retired segment hash function
- **File**: `crates/stitchd-core/src/hashing.rs:10–19`
- **Type**: unused public function
- **Evidence**: Zero callers outside its own file's test module. The active percentage-allocation path uses `calculate_allocation` (line 35). `compute_hash_percentage` predates the unified basis-point hash model; its `f64` return type is incompatible with the current u32 basis-point contract.
- **Risk**: NONE — removing it also removes 4 self-referential unit tests in the same `#[cfg(test)]` block.

### DC-006: `verify_hash_cutover.rs` xtask — migration verification for a completed migration
- **File**: `crates/xtask/src/verify_hash_cutover.rs` (370 lines) + dispatch arm at `crates/xtask/src/main.rs:18,25,35`
- **Type**: one-time migration tool; migration fully complete
- **Evidence**: The file's own doc comment: "Phase 3 task 3.5 of `flag_eval_unify_20260522`." That track is archived. The legacy `context_hash_specs` field this tool monitored no longer exists in the schema. Removal also requires deleting: `mod verify_hash_cutover;` at `main.rs:18`, the `Some("verify-hash-cutover") =>` arm at `main.rs:25`, and the help text line at `main.rs:35`.
- **Risk**: NONE — migration is baked into the v1 baseline.

---

## PROPOSE Items

### PR-001: `feature_flag_rules.frozen` write path — column written but never read in production
- **File**: `crates/stitchd-db/src/repository/pg/experiment.rs:626–639`
- **Type**: database write to a column whose value is never queried into domain objects
- **Evidence**: `flag_lock.rs` doc comment (line 3) says "the flag-lock model **replaces** today's per-rule `frozen` boolean." Production access-control uses `is_flag_locked`. The only `SELECT frozen` queries are in `tests/experiment.rs` (10+ integration test assertions). The column exists in the baseline schema (line 287 of `20260525000001_v1_baseline.sql`) and is still written by `apply_transition`.
- **Rationale for PROPOSE vs DELETE-NOW**: Removing the write and dropping the column requires a new migration + updating 10+ test assertions. Schema decision. product.md §5 explicitly states this flag was replaced ("Replaces the old per-rule `frozen` flag") — so the write path is serving a retired mechanism.
- **Risk**: LOW — write path is harmless redundancy; dropping the column requires a migration.

### PR-002: `hash_input_spec_cutover.rs` test file — migration corpus test for a completed migration
- **File**: `crates/stitchd-db/tests/hash_input_spec_cutover.rs` (677 lines)
- **Type**: migration verification test suite for a completed migration
- **Evidence**: Doc comment: "Phase 3 of `flag_eval_unify_20260522` — migration cutover verification." `context_hash_specs` no longer exists in the schema. Schema-assertion tests pass trivially. Pure-Rust corpus tests (lines 297–448) validate `canonical_sort_to_hash_inputs` converter logic also present in the xtask.
- **Rationale for PROPOSE vs DELETE-NOW**: If DC-006 (xtask removal) is accepted, these tests also lose their sole justification. Recommend deleting alongside DC-006.
- **Risk**: LOW — tests pass trivially against the current schema.

### PR-003: `EvalContextRow` type alias — dead since `spawn_eval_log_write` is dead
- **File**: `crates/stitchd-flag-service/src/eval_log_writer.rs:25`
- **Type**: unused public type alias (consequential to DC-001)
- **Evidence**: `EvalContextRow` is used only in the signature of `spawn_eval_log_write`. No external crate imports it.
- **Rationale for PROPOSE vs DELETE-NOW**: Must be removed together with DC-001, not before.
- **Risk**: NONE when removed together with DC-001.

### PR-004: Misleading `#[allow(dead_code)]` on `seed_org` in `users.rs` test module
- **File**: `crates/stitchd-db/src/auth/users.rs:480`
- **Type**: misleading `#[allow(dead_code)]` annotation on a function that is actually called
- **Evidence**: `seed_org` is called at lines 662, 687, 710. The annotation was probably left from when the function was first written before tests called it.
- **Rationale for PROPOSE vs DELETE-NOW**: One-line annotation removal (not a function deletion). Included because the annotation is actively misleading.
- **Risk**: NONE — removing the attribute reveals the function is live; no behavioral change.

---

## Product.md Baseline Check

Active modules per product.md:
1. Segmentation (rule-based + ScyllaDB list-based)
2. Feature Flags + Rule Engine (unified `evaluate_flag` orchestrator)
3. Events (pre-registered, ClickHouse, SDK ingestion)
4. Metrics (Aggregation / Ratio / Funnel, ClickHouse preview)
5. Experimentation (ITT attribution, whole-flag lock, Frequentist + Bayesian + CUPED + SRM + Guardrails)
6. Auth (JWT, Password, OIDC, SAML, MFA, Invites, Rate Limiting)
7. Admin UI (React 19 + Vite SPA)
8. Server-Side Rust SDK
9. Context Intelligence (registry, autocomplete, explorer)
10. Eval Analytics (`flag_evaluation_log` → `eval_stats` + experiment assignment MV)

Code found that appears to serve retired/unlisted modules:
- **DC-006** (`verify_hash_cutover.rs`): Serves the retired `flag_eval_unify_20260522` migration track. The dual-write `context_hash_specs` field the tool validated no longer exists.
- **DC-005** (`compute_hash_percentage`): Predates the unified basis-point hashing model in product.md §2. Its `f64` return type is incompatible with the current u32 basis-point contract.
- **DC-001** (`spawn_eval_log_write`): Eval-log write is an active feature, but this specific function is the unused alternative write path.
- **PR-001** (`frozen` write path): product.md §5 explicitly states the per-rule `frozen` flag was **replaced** by the whole-flag lock. The write path is serving a retired mechanism even though the column persists in the schema.
