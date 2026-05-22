# Track Learnings: docs_refresh_20260522

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

Project-wide patterns are in [`conductor/patterns.md`](../../patterns.md). Most relevant to
this track:

- **xtask layout** — `crates/xtask/src/main.rs` already orchestrates proto-doc gen,
  OpenAPI export, SDK rustdoc extract, mdbook build. Extensions in Phase 2 layer on top of
  this; don't restructure.
- **Parallel worker file-ownership boundary** — workflow.md "File-ownership boundary in
  worker prompts" section. Phase 3 workers MUST receive explicit owned/forbidden file lists.
- **Beads close gotcha** — `bd close --no-auto` is unreliable in current Beads; use plain
  `bd close <id>` and `--force` on phantom-dep errors. Documented in
  `conductor/patterns.md` "Experimentation Patterns" section.
- **`cargo sqlx prepare -- --tests`** — N/A for this track (no sqlx changes expected) but
  keep in mind if any task touches sqlx macros.

## Inherited from prior docs tracks

Seeded from `conductor/archive/mdbook_docs_20260418/` and
`conductor/archive/docs_microservices_20260422/`:

- mdBook source lives at `docs/src/`; `SUMMARY.md` is the canonical TOC. Any new page must
  be added to `SUMMARY.md` or it becomes orphan.
- `docs/src/grpc/*` is auto-generated; never hand-edit. Run `cargo xtask docs` to regen.
- `docs/openapi-pre-decomposition.json` is a frozen reference used by
  `scripts/check_openapi_contract.py` contract-check CI job — do NOT delete without
  understanding the contract check.

---

<!-- Learnings from implementation will be appended below -->
