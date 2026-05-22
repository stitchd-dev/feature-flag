# Track Learnings: flag_eval_unify_20260522

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### From flag_eval_preview_20260514 (parent track)

- **Admin vs SDK response shape:** Always define a separate `AdminFooJson` struct in the gateway for admin UI responses. The SDK-facing `FooJson` must stay minimal. (from: flags_crud_20260512)
- **Proto condition payload:** `rule_payload` in `FlagRule` is `serde_json::to_vec(&ConditionExpr)` — a JSON-encoded condition tree stored as bytes. Deserialize with `serde_json::from_slice`. (from: flags_crud_20260512)
- **Domain model change order:** `stitchd-core` structs → DB repo → flag service → proto → mapping.rs → gateway handler. Skipping steps causes compile errors deep in the chain. (from: flags_crud_20260512)
- **`verbatimModuleSyntax`:** Always use `import type { Foo }` for type-only imports in the admin UI. (from: admin_ui_20260427)
- **RBAC UI gating:** Use `disabled` + `style={{ opacity: 0.35 }}` for actions lacking permission. (from: env_sdk_rbac_20260429)
- **Cargo must run from the worktree root:** Always `cd .worktrees/flag_eval_unify_20260522/` before Cargo commands. (from: env_sdk_rbac_20260429)

### Additional patterns directly relevant to this track

- **Formik + Yup is the only form pattern in admin UI:** All admin forms use `<Formik>` + Yup schema. Primitives in `admin/src/components/form/`; schemas in `admin/src/lib/validation/`. API errors surface via `formik.setStatus({ error: message })`, rendered by `<FormErrorBanner />`. (from: boundaries_20260518)
- **`validateOnChange={false}` for async Yup validators:** When using async `.test()` validators, always set `validateOnChange={false}` on `<Formik>` to prevent an API call on every keystroke. Trigger validation on blur or submit only. (from: boundaries_20260518)
- **`cargo sqlx prepare` skips `#[cfg(test)]` by default:** The `-- --tests` flag is REQUIRED on `cargo sqlx prepare --workspace -- --tests`. Without it, test-only queries silently leave the offline cache unpopulated and CI fails with `no cached data for this query`. (from: scheduled_stats_20260423 + workflow.md)
- **`STITCHD_DATABASE_URL` vs `DATABASE_URL`:** sqlx-cli needs plain `DATABASE_URL`. Always `export DATABASE_URL="$STITCHD_DATABASE_URL"` before running sqlx commands. (from: boundaries_20260518)
- **In-process tonic mock servers for integration tests:** Bind `TcpListener::bind("127.0.0.1:0")` for a random free port; wrap with `tokio_stream::wrappers::TcpListenerStream`; pass to `tonic::transport::Server::builder().serve_with_incoming(...)` in a `tokio::spawn`. No external mocking library, no port conflicts. (from: boundaries_20260518)
- **`--features test-util` required for SDK clippy `--all-targets`:** `cargo clippy -p stitchd-sdk-rust --all-targets` fails with unresolved imports unless `--features test-util` is passed — conformance test helpers are behind that feature gate. (from: boundaries_20260518)
- **E2E infra-dependent tests need explicit `#[ignore]`:** Tests that require a running service daemon must be marked `#[ignore = "needs running <service>"]`. Without the annotation, the test attempts a real connection and fails silently in CI. (from: boundaries_20260518)
- **Recursive Types:** Recursive enums or structs (expression trees) must use `Box<T>` for recursive variants. Relevant when extending `ConditionExpr` or related rule-engine types. (from: rule_engine_20260412)
- **Discovered out-of-scope work:** When a worker finds a pre-existing issue clearly outside the current task scope, file a new beads bug with priority 2 and reference it from the report-back. Do not fix inline — it bloats the diff and may conflict with planned work. (from: boundaries_20260518)

### Track-specific context

- **Two paths to unify (current state):**
  - Preview path: `crates/stitchd-core/src/evaluation/preview.rs::evaluate_preview()` and `evaluate_single()`, called from `crates/stitchd-flag-service/src/service.rs:871 (FlagServiceImpl::evaluate_preview)`. Pre-fetches list memberships from Scylla via `resolve_list_memberships()` at service.rs:197–262.
  - SDK path: `sdks/rust/src/client.rs::SdkClient::evaluate_inner()` (line 762), plus `resolve_segments()` (line 887). LFU-cached list memberships via the in-SDK `MembershipCache`.
- **Shared primitives already in core (KEEP as-is):**
  - `stitchd-core::hashing::calculate_allocation` (Murmur3 → 0–100.0)
  - `stitchd-core::rule_engine::eval_expr` + `eval_leaf` (segment membership check reads from `EvaluationInput.resolved_segments`)
  - `stitchd-core::segment::{SegmentEvaluator, RuleBasedSegment, ListBasedSegment}`
- **Bucket-mapping invariant (preserve byte-equivalence):** `((percentage * 10.0).floor() as u32).min(999)` — same in both paths today. Do NOT change this expression.
- **Empty-string sentinel for missing context/parameter (preserve):** Preview's current behaviour when a `PercentageTarget` selector resolves to a missing context/parameter is to push an empty string into the hash input list. This must be preserved by the unified `HashInputSpec` resolver for hash-stability.

---

<!-- Learnings from implementation will be appended below -->
