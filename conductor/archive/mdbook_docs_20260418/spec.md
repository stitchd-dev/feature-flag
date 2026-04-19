# Spec: mdBook Documentation Site

## Overview

Create a `docs/` directory at the project root containing an mdBook-based documentation
site for Stitchd Feature Flag. The site covers the REST Admin API, gRPC SDK protocol,
Rust SDK usage, deployment, and architecture. All content is auto-generated from
existing source artifacts (OpenAPI annotations, .proto files, Rust doc comments).

## Functional Requirements

### FR-1: mdBook Setup
- `docs/` directory with `book.toml`, `src/SUMMARY.md`, and chapter source files
- `mdbook build` produces a static site in `docs/book/`
- CI step: build the book and fail on warnings

### FR-2: REST API Reference (auto-generated)
- Add `utoipa` + `utoipa-axum` to `stitchd-server` for OpenAPI 3.1 spec generation
- Expose `/api-docs/openapi.json` endpoint on the server
- Generate `docs/src/api/openapi.json` via a `cargo xtask` or build script
- Render with `mdbook-openapi` (or embed Swagger UI as a static HTML chapter)

### FR-3: gRPC / Protobuf API Reference (auto-generated)
- Use `protoc-gen-doc` to generate Markdown from `.proto` files in `stitchd-proto`
- Output lands in `docs/src/grpc/`
- Generation wired into a `cargo xtask docs` command

### FR-4: Rust SDK API Reference (auto-generated)
- `cargo doc --no-deps -p stitchd-sdk` generates rustdoc HTML
- Link from mdBook chapter to the rustdoc output (or embed via iframe/static copy)
- All public SDK types and methods must have doc comments

### FR-5: SDK Usage Guide (auto-generated from doc tests)
- `//!` module-level doc comments in `stitchd-sdk` serve as the usage narrative
- `cargo doc --test` ensures all code examples compile and run
- Content copied into `docs/src/sdk/`

### FR-6: Deployment & Self-Hosting Guide
- Markdown chapter at `docs/src/deployment/`
- Content derived from existing `conductor/product.md` and `tech-stack.md`
- Covers PostgreSQL 16+, ClickHouse 24+, environment variables, SDK key setup

### FR-7: Architecture Overview
- Markdown chapter at `docs/src/architecture/`
- Diagrams via `mdbook-mermaid` (Mermaid.js integration)
- Covers multi-tenancy model, scoping model, data stores, evaluation flow

### FR-8: `cargo xtask docs` Command
- Single command to regenerate all auto-generated content and build the book
- Sequence: protoc-gen-doc → OpenAPI export → rustdoc copy → mdbook build

## Non-Functional Requirements

- `mdbook build` must complete with zero warnings in CI
- All public SDK symbols must have doc comments (enforced via `#![deny(missing_docs)]`)
- No hand-written content duplicated from source code — single source of truth

## Acceptance Criteria

- [ ] `cargo xtask docs` runs end-to-end without errors
- [ ] `docs/book/index.html` is produced and navigable
- [ ] REST API chapter renders all endpoints from the live OpenAPI spec
- [ ] gRPC chapter renders all services/messages from `.proto` files
- [ ] SDK chapter links to or embeds rustdoc for all public types
- [ ] Deployment and Architecture chapters exist with accurate content
- [ ] CI workflow builds the book and fails on missing doc comments

## Out of Scope

- Hosting / publishing the book (GitHub Pages, Netlify, etc.)
- Versioned docs (multiple mdBook versions for different releases)
- Admin UI documentation
- Client-side SDK (browser/mobile) docs
