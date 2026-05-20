# Spec: Upgrade Rust Toolchain to 1.95 + Bring Workspace Deps to Latest Cross-Compatible

## Overview

Bring the workspace to the current Rust ecosystem baseline:

1. **Rust toolchain**: enforce **1.95.0** as the MSRV (`workspace.package.rust-version`)
   and pin CI installations to that version. `rust-toolchain.toml` continues
   to track `channel = "stable"` (per project convention — local developers
   pick up new stable releases automatically).
2. **Workspace dependencies**: bump every dep in `[workspace.dependencies]` (plus
   `metrics-util` dev-dep in `sdks/rust`) to the latest published version,
   accepting major-version bumps, using `cargo-outdated` for discovery and
   `cargo-edit` (`cargo upgrade`) for the manifest edits.

A scout wisp on 2026-05-20 catalogued the breakage surface — most majors are
mechanical fixes, but `openidconnect 3 → 4` requires a non-trivial refactor
of `crates/stitchd-core/src/auth/oidc.rs` (endpoint type-state + async http
client signature). That migration is in scope.

## Functional Requirements

- `workspace.package.rust-version` set to `"1.95"`.
- All `.github/workflows/*.yml` jobs that currently use
  `dtolnay/rust-toolchain@stable` pin to `dtolnay/rust-toolchain@1.95.0`
  (5 occurrences in `ci.yml`).
- `Cargo.toml` `[workspace.dependencies]` bumped to latest:
  - **Build / RPC**: `tonic 0.14`, `tonic-build 0.14`, `tonic-health 0.14`,
    `tonic-prost 0.14` (new), `tonic-prost-build 0.14` (new), `prost 0.14`,
    `prost-build 0.14`
  - **Storage**: `clickhouse 0.15` (insert API became async)
  - **Observability**: `tracing-opentelemetry 0.33`, `opentelemetry 0.32`,
    `opentelemetry_sdk 0.32`, `opentelemetry-otlp 0.32`,
    `opentelemetry-semantic-conventions 0.32`,
    `metrics-exporter-prometheus 0.18`
  - **Crypto / random**: `sha2 0.11`, `rand 0.10`
  - **HTTP**: `reqwest 0.13` (feature `rustls-tls` → `rustls`; add `form`)
  - **OIDC**: `openidconnect 4`
  - **Misc**: `quick-xml 0.40`, `metrics-util 0.20` (dev-dep in `sdks/rust`)
- Code adjusted to compile against new APIs:
  - `stitchd-proto/build.rs` uses `tonic_prost_build::configure()` with the
    new `compile_protos(&[PathBuf, ...])` signature.
  - `stitchd-proto/Cargo.toml` adds `tonic-prost` + `tonic-prost-build`.
  - `stitchd-event-writer`, `stitchd-db`, `stitchd-analytics-service` add
    `.await` to all `client.insert("...")` call sites (6 sites).
  - `stitchd-core/src/auth/crypto.rs`: switch `rand::thread_rng()` →
    `rand::rng()`, `gen_range` → `random_range`, `RngCore` import → `Rng`,
    and use `Aes256Gcm::generate_nonce` for the AEAD nonce.
  - `stitchd-core/src/auth/totp.rs`: same `rand` migration.
  - `stitchd-core/src/auth/oidc.rs`: migrate to openidconnect 4 API
    (endpoint type-state, `async_http_client` → reqwest client instance).

## Non-Functional Requirements

- **Lockfile** (`Cargo.lock`) regenerated via `cargo update --workspace`.
- **No new `unsafe` introduced.**
- **No silenced lints** — `cargo clippy --workspace --all-targets -- -D warnings`
  must still pass at the new toolchain.
- `cargo fmt --all --check` clean.
- **Cross-platform**: workspace continues to build on `x86_64-unknown-linux-gnu`
  (CI) and `aarch64-apple-darwin` (local).

## Acceptance Criteria

1. `rustc --version` reports `1.95.0` when honoured by the CI pin.
2. `cargo check --workspace --all-targets` succeeds.
3. `cargo clippy --workspace --all-targets --features stitchd-sdk-rust/test-util -- -D warnings`
   succeeds.
4. `cargo fmt --all --check` succeeds.
5. **Full test suite** (`cargo test --workspace` with Postgres + ClickHouse +
   Scylla running locally) passes — no regressions versus pre-upgrade baseline.
6. `cargo sqlx prepare --workspace --check -- --all-targets` succeeds (offline
   query metadata stays valid).
7. CI is green on the PR for this track.
8. `cargo outdated --workspace` reports no upgrade-able entries except those
   intentionally pinned (e.g. transitive duplicates we can't bump).

## Out of Scope

- Migrating to `axum 0.9` or other ecosystem deps not currently flagged by
  `cargo outdated` at track-creation time.
- Removing transitive dep duplication (e.g. dual `reqwest` versions pulled
  in by `lettre`) — accepted unless a vuln advisory targets the pinned
  version.
- Bumping `rust-toolchain.toml` off `channel = "stable"` — explicitly
  preserved per project convention.
- Code-style or architectural changes beyond what's needed to compile
  against the new dep APIs.
