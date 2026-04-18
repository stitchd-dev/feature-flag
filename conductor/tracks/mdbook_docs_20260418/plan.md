# Plan: mdBook Documentation Site

## Phase 1: mdBook Foundation & xtask Scaffold [checkpoint: 59f707c]

- [x] Task 1.1: Create `docs/` directory with `book.toml` and `src/SUMMARY.md` skeleton [15df307]
  - `book.toml` configures title, authors, preprocessors (mermaid, links)
  - `src/SUMMARY.md` lists all planned chapters as stubs
- [x] Task 1.2: Add `xtask` crate to workspace
  - New crate `crates/xtask` with a `docs` subcommand
  - Registers in `Cargo.toml` workspace members
  - Subcommand runs doc generation steps in sequence (stubs initially)
- [x] Task 1.3: Add `mdbook` + `mdbook-mermaid` to `xtask` as dependencies
  - Invoke `mdbook build` programmatically from xtask
  - Verify `mdbook build` succeeds with stub chapters
- [ ] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: REST API Reference (utoipa)
<!-- depends: phase1 -->

- [x] Task 2.1: Add `utoipa` + `utoipa-axum` to `stitchd-server`
  <!-- files: crates/stitchd-server/Cargo.toml, Cargo.toml, crates/stitchd-server/src/api/ -->
  - Add to workspace dependencies and server `Cargo.toml`
  - Annotate all existing route handlers with `#[utoipa::path]`
  - Annotate all request/response types with `#[derive(ToSchema)]`
- [x] Task 2.2: Expose `/api-docs/openapi.json` endpoint
  <!-- files: crates/stitchd-server/src/api/, crates/stitchd-server/src/startup.rs -->
  <!-- depends: task2.1 -->
  - Register `SwaggerUi` or raw JSON handler in the Axum router
  - Smoke-test: server returns valid JSON at that path
- [x] Task 2.3: Wire OpenAPI export into `cargo xtask docs`
  <!-- files: crates/xtask/src/main.rs, docs/src/api/ -->
  <!-- depends: task2.2 -->
  - xtask spawns `stitchd-server --export-openapi` or reads the JSON via reqwest
  - Writes output to `docs/src/api/openapi.json`
- [x] Task 2.4: Embed Swagger UI as a static mdBook chapter
  <!-- files: docs/src/api/, docs/src/SUMMARY.md -->
  <!-- depends: task2.3 -->
  - Download Swagger UI dist assets into `docs/src/api/`
  - Create `docs/src/api/rest.md` with embedded `<iframe>` or inline HTML
- [ ] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: gRPC / Protobuf Reference
<!-- depends: phase1 -->

- [x] Task 3.1: Add `protoc-gen-doc` generation to xtask
  <!-- files: crates/xtask/src/main.rs, docs/src/grpc/ -->
  - xtask invokes `protoc` with `protoc-gen-doc` plugin against `proto/` directory
  - Uses vendored `protoc` from `protoc-bin-vendored` (already in build deps)
  - Output format: Markdown, destination `docs/src/grpc/`
- [x] Task 3.2: Wire generated Markdown into SUMMARY.md
  <!-- files: docs/src/grpc/, docs/src/SUMMARY.md -->
  <!-- depends: task3.1 -->
  - xtask updates `docs/src/grpc/README.md` and per-service chapter files
  - Ensure chapter links resolve in `SUMMARY.md`
- [ ] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Rust SDK Docs
<!-- depends: phase1 -->

- [ ] Task 4.1: Add `#![deny(missing_docs)]` to `stitchd-sdk`
  <!-- files: crates/stitchd-sdk/src/ -->
  - Audit all public types, traits, and functions for missing doc comments
  - Add doc comments with usage examples (`# Examples` blocks with `cargo test` doc tests)
- [ ] Task 4.2: Wire `cargo doc` into xtask
  <!-- files: crates/xtask/src/main.rs, docs/book/rustdoc/ -->
  <!-- depends: task4.1 -->
  - xtask runs `cargo doc --no-deps -p stitchd-sdk`
  - Copies `target/doc/stitchd_sdk/` into `docs/book/rustdoc/`
- [ ] Task 4.3: Add SDK chapter to mdBook
  <!-- files: docs/src/sdk/, docs/src/SUMMARY.md -->
  <!-- depends: task4.2 -->
  - `docs/src/sdk/README.md` with narrative intro and link to rustdoc
  - `docs/src/sdk/quickstart.md` extracted from `//!` module doc in `stitchd-sdk/src/lib.rs`
- [ ] Task: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: Deployment & Architecture Chapters
<!-- depends: phase1 -->
<!-- execution: parallel -->

- [ ] Task 5.1: Write Deployment chapter
  <!-- files: docs/src/deployment/ -->
  - `docs/src/deployment/README.md` — overview, prerequisites
  - `docs/src/deployment/postgres.md` — PostgreSQL 16+ setup
  - `docs/src/deployment/clickhouse.md` — ClickHouse 24+ setup
  - `docs/src/deployment/env-vars.md` — all environment variables
  - `docs/src/deployment/sdk-keys.md` — SDK key creation and rotation
- [ ] Task 5.2: Write Architecture chapter with Mermaid diagrams
  <!-- files: docs/src/architecture/ -->
  - `docs/src/architecture/README.md` — high-level system diagram (Mermaid)
  - `docs/src/architecture/evaluation-flow.md` — flag evaluation sequence diagram
  - `docs/src/architecture/multi-tenancy.md` — tenant → env → SDK key model
  - `docs/src/architecture/data-stores.md` — PostgreSQL + ClickHouse split
- [ ] Task: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)

## Phase 6: CI Integration
<!-- depends: phase2, phase3, phase4, phase5 -->
<!-- execution: parallel -->

- [ ] Task 6.1: Add `cargo xtask docs` to CI workflow
  <!-- files: .github/workflows/ -->
  - New GitHub Actions job: `docs-build`
  - Runs `cargo xtask docs` and fails on non-zero exit
  - Caches `mdbook` and `protoc-gen-doc` binaries
- [ ] Task 6.2: Enforce `missing_docs` in CI
  <!-- files: clippy.toml, .github/workflows/ -->
  - `clippy.toml` or CI `cargo clippy` flags confirm `missing_docs` lint fires
  - Verify CI fails if a public SDK symbol loses its doc comment
- [ ] Task: Conductor - User Manual Verification 'Phase 6' (Protocol in workflow.md)
