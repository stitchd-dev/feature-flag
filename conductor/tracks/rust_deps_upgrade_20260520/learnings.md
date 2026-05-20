# Track Learnings: rust_deps_upgrade_20260520

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

From `conductor/patterns.md` — directly relevant to this track:

### Cargo / Rust 2024 setup
- Rust 2024 edition requires `resolver = "3"` in workspace `Cargo.toml`.
  (from: scaffold_20260411)
- `rustfmt.toml` options like `imports_granularity`, `group_imports`,
  `wrap_comments`, `normalize_comments` are **nightly-only** — silently
  no-op on stable. (from: scaffold_20260411)
- `std::env::set_var` is **unsafe** in Rust 2024 — already used in
  `crates/stitchd-proto/build.rs` with the correct `unsafe {}` wrapper.
  Don't regress this when touching that file. (from: scaffold_20260411)

### Build / RPC
- **Vendored protoc**: `crates/stitchd-proto/build.rs` sets `PROTOC` env
  var from `protoc-bin-vendored`. This contract must hold across the
  tonic 0.13 → 0.14 bump. (from: scaffold_20260411)

### Observability
- **OpenTelemetry version alignment**: all OTel crates must pin to the
  *same* minor (in this bump: `0.32`); `tracing-opentelemetry` then needs
  the matching version (`0.33` in this case — confirm at bump time).
  Mismatched minors produce incompatible types. (from: scaffold_20260411)
- `opentelemetry_sdk` `Resource::new()` is private since 0.28 — use
  `Resource::builder()`. Likely already in the codebase post-0.28 bump;
  confirm the 0.32 builder API hasn't shifted again. (from: scaffold_20260411)

### Storage
- `clickhouse` crate v0.13 has no `derive` feature — the workspace dep
  uses `uuid, time, lz4`. 0.15 may have added a `derive` feature; do not
  enable it unless we need it. (from: scaffold_20260411)
- **SQLx Offline Compilation**: `cargo sqlx prepare --workspace --check`
  is part of the acceptance gate — required because `sqlx::query!`
  macros need either a live DB or fresh `.sqlx/` cache. (from: segmentation_20260412)
- **`STITCHD_DATABASE_URL` vs `DATABASE_URL`**: alias before sqlx CLI:
  `export DATABASE_URL="$STITCHD_DATABASE_URL"`. (from: boundaries_20260518)

### Auth
- `crates/stitchd-core/src/auth/oidc.rs` already uses `governor` +
  `tower_governor` patterns elsewhere — unrelated to this bump but
  signals these deps are mature and any major bumps to them need code
  audit. (from: auth_20260421)

### Toolchain convention
- **Both** `rust-toolchain.toml` (`channel = "stable"`) and CI's
  `dtolnay/rust-toolchain@stable` references stay on `stable` — never
  pin to a specific version. The declarative MSRV in
  `[workspace.package].rust-version` is the sole "Rust X.Y" enforcement
  point. (from: this track's predecessor wisp + Revision 1 of this track —
  saved as user-level feedback memory `feedback_rust_toolchain_channel.md`.)

---

<!-- Learnings from implementation will be appended below -->

## [2026-05-20 17:55] - Phase 1 Task 1: Bump MSRV to 1.95

- **Implemented:** `[workspace.package].rust-version = "1.95"` in root
  `Cargo.toml`.
- **Files changed:** `Cargo.toml` (1 line)
- **Commit:** `de7579f`
- **Learnings:**
  - Patterns: All crates inherit MSRV via `rust-version.workspace = true` —
    no per-crate edits required.
  - Gotchas: This is a *declarative* compatibility floor only; the actual
    toolchain choice lives in `rust-toolchain.toml` (channel = "stable")
    and in CI's `dtolnay/rust-toolchain@stable`. Three separate concerns.
  - Context: Running stable on macOS aarch64 already reports `rustc 1.95.0`
    so `cargo check --workspace --all-targets` passed without any rustup
    install.

---

## [2026-05-20 17:58] - Phase 1 Task 2: Confirm CI stays on @stable (revised)

- **Implemented:** Decision-only — no edit. Confirmed all 5
  `dtolnay/rust-toolchain@stable` references in `.github/workflows/ci.yml`
  remain unchanged.
- **Files changed:** none (revert + revision; see commit `2540f5e` for
  spec/plan/revisions update).
- **Commit:** `2540f5e` (revision); reverted earlier pin attempt `448bc3b`.
- **Learnings:**
  - Patterns: For "bump Rust to X.Y" requests in this repo, edit
    `workspace.package.rust-version` only. Leave both `rust-toolchain.toml`
    and CI references on `stable`.
  - Gotchas: Original spec called for pinning CI; user rolled it back.
    Feedback memory expanded to cover the CI case explicitly.
  - Context: See `revisions.md` Revision 1 for full rationale.

---

## [2026-05-20 18:10] - Phase 2 Task 1: Outdated baseline

`cargo outdated --workspace --depth 1` at track start (post-MSRV bump, pre-dep-bumps):

| Crate (consumer)        | Dep            | Project | Latest |
|-------------------------|----------------|---------|--------|
| stitchd-db              | metrics        | 0.24.3  | 0.24.6 |
| stitchd-stats-service   | metrics        | 0.24.3  | 0.24.6 |
| stitchd-auth-service    | metrics        | 0.24.3  | 0.24.6 |
| stitchd-flag-service    | metrics        | 0.24.3  | 0.24.6 |
| stitchd-segmentation-service | metrics   | 0.24.3  | 0.24.6 |
| stitchd-analytics-service | metrics      | 0.24.3  | 0.24.6 |
| stitchd-experimentation-service | metrics | 0.24.3 | 0.24.6 |
| stitchd-gateway         | metrics        | 0.24.3  | 0.24.6 |
| stitchd-sdk-rust        | metrics        | 0.24.3  | 0.24.6 |
| stitchd-sdk-rust        | metrics-util   | 0.19.1  | 0.20.4 |

Note: within-semver compat shows `---` because cargo-outdated reports the in-range compatible *upgrade*, and the workspace req `metrics = "0.24"` already permits 0.24.6. `cargo update --workspace` should pick it up.

The 16 incompatible (major) bumps land in Phase 3.

---

## [2026-05-20 18:02] - Phase 1 Tasks 3 + 4: Baseline verify

- **Implemented:**
  - Task 3 (decision-only): confirmed `rust-toolchain.toml` stays on
    `channel = "stable"` — no edit.
  - Task 4: `cargo check --workspace --all-targets` + `cargo test
    --workspace --lib` ran at the new MSRV with `DATABASE_URL` aliased
    from `STITCHD_DATABASE_URL`. **1479 unit tests across 12 crates, 0
    failures.**
- **Files changed:** none (verify-only).
- **Commit:** none yet — bundled with phase-close commit below.
- **Learnings:**
  - Patterns: `cargo test --workspace --lib` covers `#[sqlx::test]` cases
    that require Postgres up + `DATABASE_URL` set. Without those, ~24
    tests in `stitchd-analytics-service` panic at `EnvVar(NotPresent)`.
    Always run `source .env.local && export DATABASE_URL="$STITCHD_DATABASE_URL"`
    before invoking `cargo test`.
  - Gotchas: Tests are FAST when Postgres is already up (under 20s for
    full --lib sweep), so the alias is the only friction.
  - Context: Postgres, ClickHouse, ScyllaDB containers had been running
    36h on this machine; `docker ps` showed them all healthy. No
    `docker compose up` needed.

---
