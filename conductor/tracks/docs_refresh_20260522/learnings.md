# Track Learnings: docs_refresh_20260522

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

Project-wide patterns are in [`conductor/patterns.md`](../../patterns.md). Most relevant to
this track:

- **xtask layout** — `crates/xtask/src/main.rs` already orchestrates proto-doc gen,
  OpenAPI export, SDK rustdoc extract, mdbook build. Extensions in Phase 2 layer on top of
  this; don't restructure.
- **Parallel worker file-ownership boundary** — workflow.md "File-ownership boundary in
  worker prompts" section. Phase 3 workers MUST receive explicit owned/forbidden file lists.
- **Beads close gotcha** — `bd close --no-auto` is unreliable in current Beads; use plain
  `bd close <id>` and `--force` on phantom-dep errors. Documented in
  `conductor/patterns.md` "Experimentation Patterns" section.
- **`cargo sqlx prepare -- --tests`** — N/A for this track (no sqlx changes expected) but
  keep in mind if any task touches sqlx macros.

## Inherited from prior docs tracks

Seeded from `conductor/archive/mdbook_docs_20260418/` and
`conductor/archive/docs_microservices_20260422/`:

- mdBook source lives at `docs/src/`; `SUMMARY.md` is the canonical TOC. Any new page must
  be added to `SUMMARY.md` or it becomes orphan.
- `docs/src/grpc/*` is auto-generated; never hand-edit. Run `cargo xtask docs` to regen.
- `docs/openapi-pre-decomposition.json` is a frozen reference used by
  `scripts/check_openapi_contract.py` contract-check CI job — do NOT delete without
  understanding the contract check.

---

<!-- Learnings from implementation will be appended below -->

## 2026-05-22 — Phase 1 Discovery findings

### Doc inventory headline numbers
- 39 `.md` files in `docs/src/` (after orphan delete: 35)
- 5 orphans (4 under `internal/` + `api/rest.md`) — all deleted in Tasks 1.2 + 1.3
- 13 crate-level READMEs; 2 crates MISSING readmes (`stitchd-analytics-service`,
  `stitchd-stats-service`) — `cargo rdme` in Task 2.2 will create these.
- 17 narrative pages remaining to refresh in Phase 3 across topics A/B/C/D (Topic E spot-check only)

### Gitignore facts
- `docs/src/grpc/*.md` (except `README.md`) is gitignored — pure build artifacts.
- `docs/src/api/openapi.json` is gitignored — pure build artifact.

### Generator stack (already wired in `crates/xtask/src/main.rs`)
1. `generate_grpc_docs()` — scans `proto/*.proto`, writes domain-grouped Markdown
   to `docs/src/grpc/<name>.md`; rewrites `# Internal gRPC Services` section of `SUMMARY.md`.
2. `export_openapi()` — `cargo build -p stitchd-gateway` + run binary with
   `--export-openapi docs/src/api/openapi.json`. Reads `#[utoipa::path]` annotations.
3. `generate_sdk_rustdoc()` — `cargo doc --no-deps -p stitchd-sdk-rust` → copy to
   `docs/book/rustdoc/`; extract `# Quickstart` from `sdks/rust/src/lib.rs` `//!` →
   `docs/src/sdk/quickstart.md`.
4. `mdbook_build()` — `mdbook build docs/` → `docs/book/`.

### Contract-check load-bearing file
- `docs/openapi-pre-decomposition.json` is used by `scripts/check_openapi_contract.py`.
  KEEP — do NOT delete. Documents intentional surface gaps from
  `boundaries_20260518` canonical-URL refactor.

### Baseline snapshot (post-xtask run)
- `/tmp/docs_refresh_baseline_20260522/` contains all generator outputs as they were
  immediately after `cargo xtask docs` against commit `fcf204c`:
  - `grpc/` (14 files: 1 README + 13 per-proto pages, gitignored)
  - `quickstart.md` (auto-extracted from `sdks/rust/src/lib.rs`)
  - `openapi.json` (147KB, gitignored, exported by `stitchd-gateway --export-openapi`)
- Phase 2 must produce zero diff against this snapshot for the existing generators.

### Discovered out-of-scope warnings (filed inline for follow-up)
1. **3 rustdoc warnings** in `sdks/rust/src/client.rs:19–21` about public docs linking to
   private items (`GrpcDefinitionFetcher`, `HttpMembershipFetcher`, `HttpEventSink`).
   Either make the items `pub`, or update the doc-comments to use non-link form.
   Filed as `feature-flag-0yf` discovered-during note; will be addressed if it lands
   naturally during Task 3.4 (SDK landing).
2. **2 mdbook warnings** about unclosed HTML tags `<context>` and `<contextpreviewresult>`
   in `docs/src/grpc/flags_v1_flag_service.md`. These come from the proto-md generator
   (`crates/xtask/src/main.rs::proto_to_markdown`) not escaping angle-bracket type names
   inside table cells. Fix: wrap type names in backticks (already done for some, missed
   for these). To address in Task 2.3 or 2.4 alongside the link-checker work.

### Phase 3 file ownership table (for parallel workers)
| Worker | Topic | Files |
|--------|-------|-------|
| A | Intro + Architecture | `docs/src/introduction.md`, `docs/src/architecture/{README,multi-tenancy,evaluation-flow,data-stores,events,metrics,service-flows}.md` |
| B | Gateway | `docs/src/gateway/{overview,sdk-api,admin-api,grpc,openapi}.md` |
| C | Deployment (minus env-vars) | `docs/src/deployment/{README,postgres,clickhouse,scylladb,sdk-keys}.md` |
| D | SDK | `docs/src/sdk/README.md` + `sdks/rust/src/lib.rs` `//!` Quickstart section |
| E | Experimentation spot-check | `docs/src/experimentation/{index,attribution,default-rule-experiments}.md` |
