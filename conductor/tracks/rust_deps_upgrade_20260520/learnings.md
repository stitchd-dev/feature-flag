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

## [2026-05-20 ~18:40] - Phases 2 + 3 + 4 (combined): all dep bumps in one cluster

**Plan deviation:** User mid-implementation feedback —
> "keep all version to minor version only in workspace.....
> And then refresh the workdpace."

This collapsed the original 3-phase ordering (compatible refresh → clean
majors → openidconnect 4) into a single rewrite where every entry in
`[workspace.dependencies]` is pinned to `major.minor` at the latest
available, plus regenerated `Cargo.lock` from scratch.

**Files changed:** 57 files; commit `6245e11`. See commit message for the
full per-bump catalogue.

### Patterns learned

- **`cargo upgrade --incompatible --pinned`** (from `cargo-edit`) bumps
  every workspace dep to its latest including major bumps; `--pinned`
  overrides the safety default that skips bare-major specs like `"1"`.
- **`cargo generate-lockfile`** (after `rm Cargo.lock`) is the cleanest way
  to refresh the lockfile to "newest within Cargo.toml ranges" without
  the `cargo update --workspace` MSRV-aware resolver holding things back.
- **Normalising bare-major specs to major.minor** is best done by reading
  the regenerated lockfile and writing `<crate> = "<major>.<minor>"` from
  the resolved version. Pre-bump baseline can be captured ahead of time.
- **`tonic 0.13 → 0.14`** split out `tonic-prost` (runtime codec) and
  `tonic-prost-build` (build helper). Add both to workspace + each crate
  that generates or consumes protobuf-derived types. `tonic_build` itself
  is now codec-agnostic; `tonic_build::configure()` no longer exists.
- **`clickhouse 0.15`** made `Client::insert()` async (returns `Future`)
  and added a `<Row>` type param. All call sites need `.await` and an
  explicit type: `client.insert::<EventRow>("table").await?`.
- **`rand 0.10`**: `thread_rng` → `rng`; `gen_range` → `random_range`;
  `RngCore` import → `Rng`; convenience methods like `random_range` are
  on `rand::RngExt` (auto-impl for any `Rng`).
- **`openidconnect 4`** flipped to endpoint type-state. After
  `from_provider_metadata`, the client is
  `Client<EndpointSet (auth), EndpointNotSet × 3,
   EndpointMaybeSet (token), EndpointMaybeSet (userinfo)>`. To store one
  in an enum, declare a `pub type` alias — the default `CoreClient`
  alias is `EndpointNotSet` for all four and won't compile. The async
  http client is now passed by reference (`&Client`) rather than as a
  closure.
- **`reqwest 0.13`** renamed the feature `rustls-tls` → `rustls`. `form`
  is no longer in the default feature set with `default-features = false`
  — add it explicitly.
- **`oauth2 5` (transitive via openidconnect 4)** pulls in `reqwest 0.12`,
  while our workspace uses `reqwest 0.13`. Two reqwest versions in the
  same tree work, but the two `reqwest::Error` types are distinct — keep
  `OidcError::Http(#[from] reqwest::Error)` bound to the workspace
  version and surface openidconnect's http errors via `String`-wrapped
  variants (`OidcError::Discovery`, `OidcError::TokenExchange`).
- **`aes-gcm 0.10` + deprecated GenericArray slice helpers.** When
  `-D warnings` is on, `Key::from_slice`, `Nonce::from_slice`, and
  `nonce.as_slice()` are deprecation errors. Use
  `Aes256Gcm::new_from_slice` and convert `[u8; 12]` ↔ `Nonce<U12>`
  through the `From` impl (`nonce_bytes.into()`).
- **`sha2 0.11` + format strings.** Newer sha2 returns a `digest`
  type that no longer implements `LowerHex` directly, so
  `format!("{digest:x}")` breaks. Use `hex::encode(digest)`.

### Clippy lints introduced by Rust 1.95

`.cargo/config.toml` sets `rustflags = ["-D", "warnings"]` workspace-wide,
so every new lint is an error.

- **`collapsible_if` (24 sites).** Rewrote every nested
  `if let A { if B { ... } }` into a Rust 2024 let-chain
  `if let A && B { ... }`. Hot files: stitchd-db pg/scylla repos (sqlx
  unique-violation mapping), saml.rs, recommendation.rs, gateway
  event_quota middleware, sdk client, analytics event_definition,
  experimentation service.
- **`duration_suboptimal_units` (8 sites).** `Duration::from_secs(60)` →
  `Duration::from_mins(1)`, `from_secs(3600)` → `from_hours(1)`,
  `from_secs(24 * 3600)` → `from_hours(24)`. `from_mins`/`from_hours` are
  stable since Rust 1.81, so the 1.95 MSRV satisfies them.
- **`clippy.toml` MSRV** had to bump from 1.85 → 1.95 to match the
  workspace; otherwise clippy emits "MSRV in clippy.toml and Cargo.toml
  differ" warnings.

### Verification

- `cargo check --workspace --all-targets` ✅
- `cargo clippy --workspace --all-targets --features stitchd-sdk-rust/test-util` ✅
- `cargo fmt --all --check` ✅
- `cargo sqlx prepare --workspace --check -- --all-targets` ✅
- `cargo test --workspace` ✅ (1 pre-existing flake — see below)

### Residuals / follow-ups

- **`cargo outdated --workspace --depth 1`** reports "All dependencies
  are up to date" at end of track.
- **Transitive duplication accepted** (unavoidable without ecosystem
  alignment): `reqwest 0.12` (via oauth2 5 → openidconnect 4) coexists
  with our `reqwest 0.13`; `schemars 0.9` + `1.2`; `indexmap 1.x` + `2.x`;
  `hashbrown 0.12` + `0.17`; `serde_derive` etc. These appear in
  `cargo outdated --workspace` (no `--depth 1`) but don't block our
  workspace deps.
- **Pre-existing flake:** `stitchd-flag-service/tests/eval_preview_clickhouse.rs::evaluate_preview_writes_rows_to_clickhouse`
  fails with `NotFound: test-flag@<project-id>` because it's an external
  smoke test that requires `test-flag` to be seeded locally with
  `TEST_PROJECT_ID=b787ef8e-...`. Verified the same failure occurs on
  the pre-bump `main` branch — not a regression. Worth documenting on
  the test (or skipping it when the seed data isn't present).

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
