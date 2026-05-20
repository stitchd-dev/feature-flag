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
- `rust-toolchain.toml` stays on `channel = "stable"`. Bump MSRV in
  `workspace.package.rust-version` instead. (from: this track's
  predecessor wisp on 2026-05-20 — saved as user-level feedback memory
  `feedback_rust_toolchain_channel.md`.)

---

<!-- Learnings from implementation will be appended below -->
