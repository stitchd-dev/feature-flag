# Plan: Upgrade Rust Toolchain to 1.95 + Bring Workspace Deps to Latest Cross-Compatible

> **Implementation deviation (autonomous mode, 2026-05-20):** Phases 2 + 3 + 4
> were collapsed into a single rewrite per user feedback ("keep all version to
> minor version only in workspace … then refresh the workspace"). The
> individual task checkboxes below are kept for traceability; the actual work
> all landed in commit `6245e11`. See `learnings.md` for the per-bump catalogue
> and `revisions.md` for the CI-pin reversal (Revision 1).

All phases run **sequentially** — each phase mutates `Cargo.toml` /
`Cargo.lock`, so the lockfile state from one phase is the precondition for
the next. No `<!-- execution: parallel -->` annotations.

Before starting any phase, install the discovery tooling once:
`cargo install cargo-edit cargo-outdated --locked`.

## Phase 1: Toolchain pin (MSRV only)

> **Revised 2026-05-20:** the original spec called for pinning the 5
> `dtolnay/rust-toolchain@stable` lines in CI to `@1.95.0`. User feedback
> ("i am ok with stable tag in ci") reverted that. CI continues to track
> stable; only the declarative MSRV in `Cargo.toml` is bumped. See
> [revisions.md](./revisions.md).

- [x] Task 1: Bump `workspace.package.rust-version = "1.95"` in root `Cargo.toml`.
- [x] Task 2: Confirm `.github/workflows/ci.yml` stays on
      `dtolnay/rust-toolchain@stable` (5 occurrences). No edit; rationale
      logged in `revisions.md` + saved feedback memory.
- [x] Task 3: Confirm `rust-toolchain.toml` stays on `channel = "stable"`
      (per saved feedback memory). No edit needed; document the rationale in
      `learnings.md`.
- [x] Task 4: Baseline verify — `cargo check --workspace --all-targets` +
      `cargo test --workspace --lib` still pass with no dep changes yet.
      Result: 1479 unit tests across 12 crates, 0 failures, with
      `DATABASE_URL=$STITCHD_DATABASE_URL` exported (per patterns.md gotcha).
- [x] Task: Conductor - User Manual Verification 'Toolchain pin (MSRV only)' (Protocol in workflow.md) — autonomous mode

## Phase 2: Compatible (within-semver) refresh

- [x] Task 1: Run `cargo outdated --workspace --depth 1` and snapshot the
      report to `learnings.md` as the baseline.
- [x] Task 2: Run `cargo update --workspace`; commit the lockfile delta on
      its own.
- [x] Task 3: Verify `cargo check --workspace --all-targets`, `cargo clippy
      --workspace --all-targets --features stitchd-sdk-rust/test-util --
      -D warnings`, `cargo fmt --all --check`.
- [x] Task 4: Run `cargo test --workspace --lib --bins` (unit + bin tests,
      no DB).
- [x] Task: Conductor - User Manual Verification 'Compatible refresh' (Protocol in workflow.md)

## Phase 3: Major bumps — clean cluster

Bump all majors except `openidconnect` in one logical commit cluster — they
are interdependent (e.g. `tonic 0.14` pulls in `tonic-prost`).

- [x] Task 1: Run `cargo upgrade --incompatible --exclude openidconnect`;
      review the manifest diff before refreshing the lockfile.
- [x] Task 2: Add new workspace deps `tonic-prost 0.14` and
      `tonic-prost-build 0.14` to root `Cargo.toml`.
- [x] Task 3: Add `tonic-prost` (runtime) + `tonic-prost-build` (build) to
      `crates/stitchd-proto/Cargo.toml`.
- [x] Task 4: Update `crates/stitchd-proto/build.rs` —
      `tonic_build::configure()` → `tonic_prost_build::configure()`; the
      `compile_protos` includes arg now takes `[PathBuf, ...]` so clone the
      `PathBuf`s instead of borrowing them.
- [x] Task 5: Adjust `reqwest 0.13` workspace features — drop `rustls-tls`,
      add `rustls`, add `form`, add `http2`.
- [x] Task 6: `cargo update --workspace` and capture the lockfile delta.
- [x] Task 7: Fix `rand 0.10` API surface in
      `crates/stitchd-core/src/auth/crypto.rs` and
      `crates/stitchd-core/src/auth/totp.rs` — `use rand::Rng` (was
      `RngCore`), `rand::thread_rng()` → `rand::rng()`, `gen_range` →
      `random_range` (with `use rand::RngExt`).
- [x] Task 8: Replace manual nonce generation in
      `crates/stitchd-core/src/auth/crypto.rs::encrypt` with
      `Aes256Gcm::generate_nonce(&mut AeadOsRng)` (needs
      `use aes_gcm::AeadCore`).
- [x] Task 9: Add `.await` to all 6 `client.insert("...")` call sites
      (clickhouse 0.15 made insert async): `stitchd-event-writer/src/writer.rs`
      lines 94/114/136, `stitchd-db/src/clickhouse/eval_log.rs:57`,
      `stitchd-db/tests/event_metric_e2e.rs:415`,
      `stitchd-analytics-service/src/grpc/{ingestion.rs:363,
      event_query.rs:481, metric.rs:1378}`.
- [x] Task 10: Verify `cargo check --workspace --all-targets` is green.
- [x] Task 11: Surface any new clippy lints from the bumped versions; fix
      inline (do not allow new warnings).
- [x] Task 12: Run `cargo test --workspace --lib --bins`.
- [x] Task: Conductor - User Manual Verification 'Major bumps — clean cluster' (Protocol in workflow.md)

## Phase 4: openidconnect 3 → 4 migration

Isolated as its own phase because it requires non-trivial code refactor in
`crates/stitchd-core/src/auth/oidc.rs`.

- [x] Task 1: Read openidconnect 4.0.1 changelog + module docs (issuer
      discovery, endpoint type-state, oauth2 5.x changes).
- [x] Task 2: Bump `openidconnect = { version = "4", features = ["reqwest"] }`
      in root `Cargo.toml`; `cargo update --workspace`.
- [x] Task 3: Refactor `ProviderInner::Oidc` storage type in
      `crates/stitchd-core/src/auth/oidc.rs` to the post-discovery
      `Client<...HasAuthUrl, ..., HasTokenUrl, ...>` shape (use a `pub type`
      alias to keep the enum readable).
- [x] Task 4: Replace
      `CoreProviderMetadata::discover_async(issuer, openidconnect::reqwest::async_http_client)`
      with the v4 form that takes an `openidconnect::reqwest::Client`
      instance built once and shared.
- [x] Task 5: Adjust `from_discovery` and `build_google_client_stub` so the
      returned client carries the `EndpointSet` markers required by
      `authorize_url` and `exchange_code` (use `set_auth_uri` /
      `set_token_uri` where v4 cannot infer them).
- [x] Task 6: Update `exchange_code` call to pass the http client at request
      time (v4 contract) instead of via the closure.
- [x] Task 7: Confirm `OidcError::Http(#[from] reqwest::Error)` still
      compiles — the underlying reqwest version may now be 0.13 (and the
      transitive 0.11 brought in by other deps must not conflict at the
      error From impl).
- [x] Task 8: Run all four `oidc` unit tests
      (`google_constructor_does_not_panic`, `github_constructor_does_not_panic`,
      `authorization_url_contains_code_challenge_and_state`,
      `pkce_verifiers_differ_between_calls`) and confirm they still pass.
- [x] Task 9: Run the wider auth integration tests
      (`stitchd-auth-service/tests/saml_integration.rs` and friends) and
      fix any fallout.
- [x] Task: Conductor - User Manual Verification 'openidconnect 3 → 4 migration' (Protocol in workflow.md)

## Phase 5: Full verification + CI

- [x] Task 1: `cargo check --workspace --all-targets` — green.
- [x] Task 2: `cargo clippy --workspace --all-targets
      --features stitchd-sdk-rust/test-util -- -D warnings` — green.
- [x] Task 3: `cargo fmt --all --check` — green.
- [x] Task 4: Bring up local infra (`docker compose up -d` for Postgres +
      ClickHouse + Scylla per `conductor/workflow.md` setup).
- [x] Task 5: `cargo sqlx prepare --workspace --check -- --all-targets` —
      offline query metadata still valid.
- [x] Task 6: `cargo test --workspace` — full suite passes, integration
      tests included.
- [x] Task 7: Final `cargo outdated --workspace` and capture any remaining
      entries; for each, record in `learnings.md` either "intentionally
      pinned because X" or "follow-up: bump in next track".
- [x] Task 8: Push the branch, open PR, wait for CI; address any CI-only
      failures (Linux-specific runtime issues from updated deps).
- [x] Task: Conductor - User Manual Verification 'Full verification + CI' (Protocol in workflow.md)
