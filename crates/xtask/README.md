# xtask

<!-- cargo-rdme start -->

`xtask` — workspace utility commands invoked via `cargo xtask <task>`.

Implements three tasks:
- `docs` — regenerate gRPC reference, OpenAPI JSON, env-vars table, SDK rustdoc, and
  the mdBook site under `docs/book/`. Idempotent: running twice produces no diff.
- `scylla-migrate` — apply pending CQL migrations from
  `crates/stitchd-db/scylla-migrations/` against the configured ScyllaDB cluster.
- `verify-hash-cutover` — re-hash a frozen corpus of `(legacy
  context_hash_specs, new hash_inputs)` pairs and report bucket-identical vs.
  operator-review-required percentages. See Phase 3 of
  `conductor/tracks/flag_eval_unify_20260522/`.

<!-- cargo-rdme end -->

```bash
cargo run --manifest-path crates/xtask/Cargo.toml -- <command>
```

or the workspace alias (if configured):

```bash
cargo xtask <command>
```

## Commands

| Command | Description |
|---------|-------------|
| `docs` | Regenerate every doc artifact and build the mdBook site under `docs/book/`. |
| `scylla-migrate` | Apply pending CQL migrations from `crates/stitchd-db/scylla-migrations/`. |

## `docs` pipeline

Executes, in order:

1. `generate_grpc_docs` — walks `proto/*.proto`, writes domain-grouped Markdown to `docs/src/grpc/` and rewrites the gRPC section of `SUMMARY.md`.
2. `export_openapi` — builds `stitchd-gateway`, runs `--export-openapi`, writes `docs/src/api/openapi.json`.
3. `generate_env_vars` — scrapes `env::var("STITCHD_*")` + `env_or("STITCHD_*", ...)` usage across `crates/*/src/` and `sdks/*/src/`, emits `docs/src/deployment/env-vars.md`.
4. `generate_crate_readmes` — runs `cargo rdme --workspace-project <name>` for every workspace crate, regenerating `crates/*/README.md` from the crate's top-level `//!` docs.
5. `extract_sdk_quickstart` — extracts the `# Quickstart` section from `sdks/rust/src/lib.rs` `//!` into `docs/src/sdk/quickstart.md`.
6. `mdbook_build` — `mdbook build docs/` → `docs/book/html/`.
7. `generate_sdk_rustdoc` — `cargo doc --no-deps -p stitchd-sdk-rust`, copy to `docs/book/rustdoc/`. **Must run after step 6** because mdbook clears its build dir on rebuild.
8. `check_internal_links` — verifies every relative-path markdown link inside `docs/src/` resolves to an existing file (or the corresponding rendered artifact under `docs/book/` for `.html` cross-refs). Fails the build on any broken link.

## Idempotency (self-test)

`cargo xtask docs` is **idempotent**: running it twice in a row must produce zero git diff.

```bash
# Run, then assert no drift:
cargo run --manifest-path crates/xtask/Cargo.toml -- docs
git diff --exit-code   # must be 0
```

CI runs this assertion via the docs job; any developer who hand-edits a generator-owned file (e.g. `docs/src/deployment/env-vars.md`, `crates/*/README.md`, or any file under `docs/src/grpc/` other than `README.md`) will see CI fail until the change is moved into the corresponding source-of-truth (the relevant `//!` preamble, a `STITCHD_*` env-var declaration, or a `.proto` file).

## Tool Management

`xtask` auto-installs missing tools via `cargo install`:
- `mdbook` (^0.5) — mdBook builder
- `mdbook-mermaid` (^0.17) — Mermaid diagram preprocessor
- `cargo-rdme` (^1.5) — generates crate READMEs from `//!` docs

Tool version pins live in [`Cargo.toml`](./Cargo.toml) under `[package.metadata.xtask-tools]` so they remain reproducible.

## Development

```bash
# Build docs locally
cargo run --manifest-path crates/xtask/Cargo.toml -- docs

# Apply pending ScyllaDB migrations
cargo run --manifest-path crates/xtask/Cargo.toml -- scylla-migrate
```
