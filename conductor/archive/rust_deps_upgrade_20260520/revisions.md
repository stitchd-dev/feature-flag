# Revisions: rust_deps_upgrade_20260520

Tracks revisions to spec.md and plan.md made during implementation.

## Revision 1 — 2026-05-20 — Spec + Plan

**Type:** Spec + Plan

**Triggered by:** User feedback during Phase 1 Task 2 implementation:
> "i am ok with stable tag in ci"

**Context (current phase/task when issue surfaced):** Phase 1 ("Toolchain pin
(MSRV + CI)"), Task 2 — pinning the 5 `dtolnay/rust-toolchain@stable`
references in `.github/workflows/ci.yml` to `@1.95.0`. The pin had been
committed (`448bc3b`); user interrupted and asked to keep CI on `@stable`.

**Changes made:**

1. `.github/workflows/ci.yml` — reverted (`git reset --hard HEAD~1`) to keep
   all 5 `dtolnay/rust-toolchain@stable` references unchanged.
2. `spec.md` — Functional Requirements: removed the bullet requiring CI pins
   to `@1.95.0`. The MSRV bump in `workspace.package.rust-version` is the
   sole "Rust 1.95" enforcement point.
3. `plan.md`:
   - Phase 1 renamed: "Toolchain pin (MSRV + CI)" → "Toolchain pin (MSRV only)".
   - Phase 1 Task 2 reworded to "Confirm CI keeps `@stable`; no edit" and
     marked `[x]` (decision-only task).
   - Phase 1 final verification task name updated accordingly.
4. Saved feedback memory broadened: previously only covered
   `rust-toolchain.toml`; now covers both `rust-toolchain.toml` AND
   `dtolnay/rust-toolchain@stable` in CI.

**Rationale:** User wants the project to track Rust stable in both developer
toolchains and CI runners. Pinning CI to a specific patch release would
force a CI bump every time a new Rust patch ships. The MSRV in
`Cargo.toml` already encodes the declarative compatibility floor; CI
running stable just means we always test on the freshest validated stable.
