# xtask

Build tool for the Stitchd workspace. Invoked via:

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
