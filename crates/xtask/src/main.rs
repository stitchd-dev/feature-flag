use anyhow::{Context, Result};
use std::process::Command;

fn main() -> Result<()> {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("docs") => docs(),
        _ => {
            eprintln!("Usage: cargo xtask <task>");
            eprintln!();
            eprintln!("Tasks:");
            eprintln!("  docs    Regenerate all documentation and build the mdBook site");
            std::process::exit(1);
        }
    }
}

fn docs() -> Result<()> {
    let root = project_root();

    // Step 1: protoc-gen-doc (Phase 3 will wire this up)
    // Step 2: OpenAPI export (Phase 2 will wire this up)
    // Step 3: rustdoc copy (Phase 4 will wire this up)

    // Step 4: build the mdBook site
    mdbook_build(&root)?;

    println!("✓ Documentation built at docs/book/");
    Ok(())
}

fn mdbook_build(root: &std::path::Path) -> Result<()> {
    let docs_dir = root.join("docs");
    let status = Command::new("mdbook")
        .args(["build", docs_dir.to_str().unwrap()])
        .status()
        .context("failed to run `mdbook build` — is mdbook installed?")?;

    anyhow::ensure!(status.success(), "`mdbook build` exited with {status}");
    Ok(())
}

fn project_root() -> std::path::PathBuf {
    // Walk up from this binary's manifest to the workspace root
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set — run via `cargo xtask`");
    // crates/xtask  →  <workspace_root>
    std::path::PathBuf::from(manifest)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
