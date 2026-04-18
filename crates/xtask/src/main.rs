use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
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

    ensure_tool("mdbook", "0.5")?;
    ensure_tool("mdbook-mermaid", "0.17")?;

    // Step 1: protoc-gen-doc (Phase 3 will wire this up)
    // Step 2: Export OpenAPI JSON from the server binary
    export_openapi(&root)?;
    // Step 3: rustdoc copy (Phase 4 will wire this up)

    // Step 4: build the mdBook site
    mdbook_build(&root)?;

    println!("✓ Documentation built at docs/book/");
    Ok(())
}

fn export_openapi(root: &Path) -> Result<()> {
    let out_path = root.join("docs/src/api/openapi.json");
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).context("failed to create docs/src/api/")?;
    }

    println!("Exporting OpenAPI JSON → {}", out_path.display());

    // Build the server binary first (no-op if already up to date).
    let build_status = Command::new("cargo")
        .args(["build", "-p", "stitchd-server"])
        .current_dir(root)
        .status()
        .context("failed to run `cargo build -p stitchd-server`")?;
    anyhow::ensure!(build_status.success(), "`cargo build` exited with {build_status}");

    // Run the binary with --export-openapi <path>.
    let binary = root.join("target/debug/stitchd-server");
    let export_status = Command::new(&binary)
        .args(["--export-openapi", out_path.to_str().unwrap()])
        .current_dir(root)
        .status()
        .with_context(|| format!("failed to run `{}`", binary.display()))?;
    anyhow::ensure!(
        export_status.success(),
        "`stitchd-server --export-openapi` exited with {export_status}"
    );

    Ok(())
}

fn mdbook_build(root: &Path) -> Result<()> {
    let docs_dir = root.join("docs");
    let status = Command::new("mdbook")
        .args(["build", docs_dir.to_str().unwrap()])
        .status()
        .context("failed to run `mdbook build`")?;

    anyhow::ensure!(status.success(), "`mdbook build` exited with {status}");
    Ok(())
}

/// Ensures a cargo-installable CLI tool is available in PATH.
/// Installs it via `cargo install` if not found.
fn ensure_tool(name: &str, version: &str) -> Result<()> {
    if which(name) {
        return Ok(());
    }
    println!("Installing {name}@{version} via `cargo install`…");
    let status = Command::new("cargo")
        .args(["install", "--locked", "--version", version, name])
        .status()
        .with_context(|| format!("failed to run `cargo install {name}`"))?;
    anyhow::ensure!(
        status.success(),
        "`cargo install {name}` exited with {status}"
    );
    Ok(())
}

fn which(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn project_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set — run via `cargo xtask`");
    // crates/xtask  →  <workspace_root>
    PathBuf::from(manifest)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
