# Plan: Upgrade Rust Toolchain to 1.95 + Bring Workspace Deps to Latest Cross-Compatible

All phases run **sequentially** — each phase mutates `Cargo.toml` /
`Cargo.lock`, so the lockfile state from one phase is the precondition for
the next. No `<!-- execution: parallel -->` annotations.

Before starting any phase, install the discovery tooling once:
`cargo install cargo-edit cargo-outdated --locked`.

## Phase 1: Toolchain pin (MSRV + CI)

- [ ] Task 1: Bump `workspace.package.rust-version = "1.95"` in root `Cargo.toml`.
- [ ] Task 2: Pin all 5 `dtolnay/rust-toolchain@stable` uses in
      `.github/workflows/ci.yml` to `dtolnay/rust-toolchain@1.95.0`.
- [ ] Task 3: Confirm `rust-toolchain.toml` stays on `channel = "stable"`
      (per saved feedback memory). No edit needed; document the rationale in
      `learnings.md`.
- [ ] Task 4: Baseline verify — `cargo check --workspace --all-targets` +
      `cargo test --workspace --lib` still pass with no dep changes yet.
- [ ] Task: Conductor - User Manual Verification 'Toolchain pin (MSRV + CI)' (Protocol in workflow.md)

## Phase 2: Compatible (within-semver) refresh

- [ ] Task 1: Run `cargo outdated --workspace --depth 1` and snapshot the
      report to `learnings.md` as the baseline.
- [ ] Task 2: Run `cargo update --workspace`; commit the lockfile delta on
      its own.
- [ ] Task 3: Verify `cargo check --workspace --all-targets`, `cargo clippy
      --workspace --all-targets --features stitchd-sdk-rust/test-util --
      -D warnings`, `cargo fmt --all --check`.
- [ ] Task 4: Run `cargo test --workspace --lib --bins` (unit + bin tests,
      no DB).
- [ ] Task: Conductor - User Manual Verification 'Compatible refresh' (Protocol in workflow.md)

## Phase 3: Major bumps — clean cluster

Bump all majors except `openidconnect` in one logical commit cluster — they
are interdependent (e.g. `tonic 0.14` pulls in `tonic-prost`).

- [ ] Task 1: Run `cargo upgrade --incompatible --exclude openidconnect`;
      review the manifest diff before refreshing the lockfile.
- [ ] Task 2: Add new workspace deps `tonic-prost 0.14` and
      `tonic-prost-build 0.14` to root `Cargo.toml`.
- [ ] Task 3: Add `tonic-prost` (runtime) + `tonic-prost-build` (build) to
      `crates/stitchd-proto/Cargo.toml`.
- [ ] Task 4: Update `crates/stitchd-proto/build.rs` —
      `tonic_build::configure()` → `tonic_prost_build::configure()`; the
      `compile_protos` includes arg now takes `[PathBuf, ...]` so clone the
      `PathBuf`s instead of borrowing them.
- [ ] Task 5: Adjust `reqwest 0.13` workspace features — drop `rustls-tls`,
      add `rustls`, add `form`, add `http2`.
- [ ] Task 6: `cargo update --workspace` and capture the lockfile delta.
- [ ] Task 7: Fix `rand 0.10` API surface in
      `crates/stitchd-core/src/auth/crypto.rs` and
      `crates/stitchd-core/src/auth/totp.rs` — `use rand::Rng` (was
      `RngCore`), `rand::thread_rng()` → `rand::rng()`, `gen_range` →
      `random_range` (with `use rand::RngExt`).
- [ ] Task 8: Replace manual nonce generation in
      `crates/stitchd-core/src/auth/crypto.rs::encrypt` with
      `Aes256Gcm::generate_nonce(&mut AeadOsRng)` (needs
      `use aes_gcm::AeadCore`).
- [ ] Task 9: Add `.await` to all 6 `client.insert("...")` call sites
      (clickhouse 0.15 made insert async): `stitchd-event-writer/src/writer.rs`
      lines 94/114/136, `stitchd-db/src/clickhouse/eval_log.rs:57`,
      `stitchd-db/tests/event_metric_e2e.rs:415`,
      `stitchd-analytics-service/src/grpc/{ingestion.rs:363,
      event_query.rs:481, metric.rs:1378}`.
- [ ] Task 10: Verify `cargo check --workspace --all-targets` is green.
- [ ] Task 11: Surface any new clippy lints from the bumped versions; fix
      inline (do not allow new warnings).
- [ ] Task 12: Run `cargo test --workspace --lib --bins`.
- [ ] Task: Conductor - User Manual Verification 'Major bumps — clean cluster' (Protocol in workflow.md)

## Phase 4: openidconnect 3 → 4 migration

Isolated as its own phase because it requires non-trivial code refactor in
`crates/stitchd-core/src/auth/oidc.rs`.

- [ ] Task 1: Read openidconnect 4.0.1 changelog + module docs (issuer
      discovery, endpoint type-state, oauth2 5.x changes).
- [ ] Task 2: Bump `openidconnect = { version = "4", features = ["reqwest"] }`
      in root `Cargo.toml`; `cargo update --workspace`.
- [ ] Task 3: Refactor `ProviderInner::Oidc` storage type in
      `crates/stitchd-core/src/auth/oidc.rs` to the post-discovery
      `Client<...HasAuthUrl, ..., HasTokenUrl, ...>` shape (use a `pub type`
      alias to keep the enum readable).
- [ ] Task 4: Replace
      `CoreProviderMetadata::discover_async(issuer, openidconnect::reqwest::async_http_client)`
      with the v4 form that takes an `openidconnect::reqwest::Client`
      instance built once and shared.
- [ ] Task 5: Adjust `from_discovery` and `build_google_client_stub` so the
      returned client carries the `EndpointSet` markers required by
      `authorize_url` and `exchange_code` (use `set_auth_uri` /
      `set_token_uri` where v4 cannot infer them).
- [ ] Task 6: Update `exchange_code` call to pass the http client at request
      time (v4 contract) instead of via the closure.
- [ ] Task 7: Confirm `OidcError::Http(#[from] reqwest::Error)` still
      compiles — the underlying reqwest version may now be 0.13 (and the
      transitive 0.11 brought in by other deps must not conflict at the
      error From impl).
- [ ] Task 8: Run all four `oidc` unit tests
      (`google_constructor_does_not_panic`, `github_constructor_does_not_panic`,
      `authorization_url_contains_code_challenge_and_state`,
      `pkce_verifiers_differ_between_calls`) and confirm they still pass.
- [ ] Task 9: Run the wider auth integration tests
      (`stitchd-auth-service/tests/saml_integration.rs` and friends) and
      fix any fallout.
- [ ] Task: Conductor - User Manual Verification 'openidconnect 3 → 4 migration' (Protocol in workflow.md)

## Phase 5: Full verification + CI

- [ ] Task 1: `cargo check --workspace --all-targets` — green.
- [ ] Task 2: `cargo clippy --workspace --all-targets
      --features stitchd-sdk-rust/test-util -- -D warnings` — green.
- [ ] Task 3: `cargo fmt --all --check` — green.
- [ ] Task 4: Bring up local infra (`docker compose up -d` for Postgres +
      ClickHouse + Scylla per `conductor/workflow.md` setup).
- [ ] Task 5: `cargo sqlx prepare --workspace --check -- --all-targets` —
      offline query metadata still valid.
- [ ] Task 6: `cargo test --workspace` — full suite passes, integration
      tests included.
- [ ] Task 7: Final `cargo outdated --workspace` and capture any remaining
      entries; for each, record in `learnings.md` either "intentionally
      pinned because X" or "follow-up: bump in next track".
- [ ] Task 8: Push the branch, open PR, wait for CI; address any CI-only
      failures (Linux-specific runtime issues from updated deps).
- [ ] Task: Conductor - User Manual Verification 'Full verification + CI' (Protocol in workflow.md)
