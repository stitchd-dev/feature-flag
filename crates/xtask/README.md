# xtask

<!-- cargo-rdme start -->

`xtask` — workspace utility commands invoked via `cargo xtask <task>`.

Implements two tasks:
- `docs` — regenerate gRPC reference, OpenAPI JSON, env-vars table, SDK rustdoc, and
  the mdBook site under `docs/book/`. Idempotent: running twice produces no diff.
- `scylla-migrate` — apply pending CQL migrations from
  `crates/stitchd-db/scylla-migrations/` against the configured ScyllaDB cluster.

<!-- cargo-rdme end -->

```bash
cargo run --manifest-path crates/xtask/Cargo.toml -- <command>
```

or the workspace alias (if configured):

```bash
cargo xtask <command>
```

## Commands

| Command | Description |
|---------|-------------|
| `docs` | Build the mdBook documentation and open `docs/book/index.html` |

## Tool Management

`xtask` auto-installs `mdbook` and `mdbook-mermaid` via `cargo install` if they are absent from `PATH`. Pinned versions are declared in `[package.metadata.xtask-tools]` in `Cargo.toml` for reproducible builds.

## Development

```bash
# Build docs locally
cargo run --manifest-path crates/xtask/Cargo.toml -- docs
```
