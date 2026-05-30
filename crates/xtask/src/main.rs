//! `xtask` — workspace utility commands invoked via `cargo xtask <task>`.
//!
//! Implements two tasks:
//! - `docs` — regenerate gRPC reference, OpenAPI JSON, env-vars table, SDK rustdoc, and
//!   the mdBook site under `docs/book/`. Idempotent: running twice produces no diff.
//! - `scylla-migrate` — apply pending CQL migrations from
//!   `crates/stitchd-db/scylla-migrations/` against the configured ScyllaDB cluster.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() -> Result<()> {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("docs") => docs(),
        Some("scylla-migrate") => scylla_migrate(),
        _ => {
            eprintln!("Usage: cargo xtask <task>");
            eprintln!();
            eprintln!("Tasks:");
            eprintln!(
                "  docs                  Regenerate all documentation and build the mdBook site"
            );
            eprintln!("  scylla-migrate        Apply pending ScyllaDB CQL migrations");
            std::process::exit(1);
        }
    }
}

fn scylla_migrate() -> Result<()> {
    use stitchd_db::scylla::{ScyllaClient, ScyllaConfig, migrate};

    let config = ScyllaConfig::from_env();
    println!(
        "Connecting to ScyllaDB at {} (keyspace: {})",
        config.uri, config.keyspace
    );

    let migrations_dir = {
        let root = project_root();
        root.join("crates/stitchd-db/scylla-migrations")
            .to_string_lossy()
            .to_string()
    };

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?
        .block_on(async {
            let client = ScyllaClient::connect(&config)
                .await
                .context("failed to connect to ScyllaDB")?;
            migrate::run(&client, &migrations_dir)
                .await
                .context("migration failed")?;
            println!("✓ ScyllaDB migrations applied successfully");
            Ok(())
        })
}

fn docs() -> Result<()> {
    let root = project_root();

    ensure_tool("mdbook", "^0.5")?;
    ensure_tool("mdbook-mermaid", "^0.17")?;
    ensure_cargo_subcommand("cargo-rdme", "^1.5")?;

    // Step 1: Generate gRPC reference from .proto files
    generate_grpc_docs(&root)?;
    // Step 2: Export OpenAPI JSON from the server binary
    export_openapi(&root)?;
    // Step 3: Generate env-vars reference by scraping STITCHD_ usage from Rust source
    generate_env_vars(&root)?;
    // Step 4: Regenerate every crate-level README.md from its `//!` rustdoc
    generate_crate_readmes(&root)?;
    // Step 5: extract SDK Quickstart from sdks/rust/src/lib.rs //! into docs/src/sdk/.
    //   (This populates a docs/src/-side file that mdbook then renders. Separate from
    //   step 7's `cargo doc` + `docs/book/rustdoc/` copy, which must run AFTER mdbook
    //   build because mdbook wipes `docs/book/` on each rebuild.)
    extract_sdk_quickstart(&root)?;

    // Step 6: build the mdBook site
    mdbook_build(&root)?;

    // Step 7: Build SDK rustdoc and copy into docs/book/rustdoc/. Runs AFTER mdbook
    // build because mdbook wipes docs/book/ on rebuild; copying before mdbook would
    // be deleted before the link-checker (step 8) ran.
    generate_sdk_rustdoc(&root)?;

    // Step 8: verify every internal markdown link resolves (in-tree, no network).
    // Implemented in xtask rather than as an mdbook-linkcheck backend because the
    // mdbook-linkcheck 0.7 binary is incompatible with mdbook 0.5.x (RenderContext
    // schema drift). See note in docs/book.toml.
    check_internal_links(&root)?;

    println!("✓ Documentation built at docs/book/");
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 7: internal-link check (post-mdbook-build)
// ---------------------------------------------------------------------------

/// Walk every `.md` under `docs/src/` for relative-path markdown links and verify
/// each target exists. Reports all broken links, then returns an error if any were
/// found (so `cargo xtask docs` exits non-zero in CI).
///
/// Scope:
/// - Checks `[text](path)` and `[text](path#anchor)` and reference-style `[text]: path`.
/// - Resolves paths relative to the linking `.md`'s parent directory.
/// - Skips URL schemes (`http:`, `https:`, `mailto:`, `data:`) and bare `#anchor` links.
/// - Treats links to `.html` as if they were the corresponding `.md` (mdbook rewrites
///   on output).
/// - Anchors are NOT verified (mdbook's heading-to-id mapping is non-trivial).
fn check_internal_links(root: &Path) -> Result<()> {
    let src_dir = root.join("docs/src");
    println!(
        "Checking internal markdown links under {}",
        src_dir.display()
    );

    let mut md_files: Vec<PathBuf> = Vec::new();
    collect_markdown_files(&src_dir, &mut md_files)?;
    md_files.sort();

    let mut broken: Vec<(PathBuf, String, PathBuf)> = Vec::new();
    for md in &md_files {
        let content = std::fs::read_to_string(md)
            .with_context(|| format!("failed to read {}", md.display()))?;
        for link in extract_relative_links(&content) {
            let resolved = resolve_link_target(md, &link);
            let target_for_check = strip_anchor_and_normalize(&resolved);
            if !link_target_ok(root, &target_for_check) {
                broken.push((md.clone(), link, target_for_check));
            }
        }
    }

    if !broken.is_empty() {
        eprintln!("\n✗ {} broken internal link(s) found:\n", broken.len());
        for (src, link, target) in &broken {
            eprintln!(
                "  • {}\n    → link `{}` resolves to non-existent {}\n",
                src.strip_prefix(root).unwrap_or(src).display(),
                link,
                target.display(),
            );
        }
        anyhow::bail!("{} broken internal link(s)", broken.len());
    }

    println!(
        "✓ Internal-link check passed ({} files scanned)",
        md_files.len()
    );
    Ok(())
}

fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Extract all relative-path link destinations from a markdown source. Returns the
/// raw destination string (path + optional anchor). Skips URL schemes and bare anchors.
fn extract_relative_links(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Inline link: `[text](dest)` — find `](`.
        if let Some(close_text) = content[i..].find("](") {
            let abs = i + close_text + 2;
            // Find the closing `)` for this link. Bail on whitespace/newline since
            // mdbook doesn't allow them in destinations without escaping.
            if let Some(close_dest) = content[abs..].find([')', '\n', ' ']) {
                if content.as_bytes().get(abs + close_dest) == Some(&b')') {
                    let dest = &content[abs..abs + close_dest];
                    if is_relative_link(dest) {
                        out.push(dest.to_string());
                    }
                }
                i = abs + close_dest;
                continue;
            }
        }
        break;
    }

    // Reference-style: lines like `[label]: dest`
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[')
            && let Some(close) = trimmed.find("]: ")
        {
            let dest = trimmed[close + 3..].split_whitespace().next().unwrap_or("");
            if is_relative_link(dest) {
                out.push(dest.to_string());
            }
        }
    }

    out
}

fn is_relative_link(dest: &str) -> bool {
    let d = dest.trim();
    if d.is_empty() {
        return false;
    }
    if d.starts_with('#') {
        return false;
    }
    for scheme in ["http://", "https://", "mailto:", "data:", "tel:", "ftp:"] {
        if d.starts_with(scheme) {
            return false;
        }
    }
    true
}

/// Resolve a relative-path link against the linking file's parent dir.
fn resolve_link_target(from_md: &Path, link: &str) -> PathBuf {
    let parent = from_md.parent().unwrap_or(Path::new(""));
    parent.join(link)
}

/// Strip the `#anchor` suffix (and `?query` if any), then normalize `./` and `../`
/// components. Preserves a leading `/` for absolute paths.
fn strip_anchor_and_normalize(p: &Path) -> PathBuf {
    let s = p.to_string_lossy().to_string();
    let no_anchor = s.split('#').next().unwrap_or(&s);
    let no_query = no_anchor.split('?').next().unwrap_or(no_anchor);
    let is_absolute = no_query.starts_with('/');

    let mut components: Vec<&str> = Vec::new();
    for part in no_query.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            other => components.push(other),
        }
    }
    let joined = components.join("/");
    PathBuf::from(if is_absolute {
        format!("/{joined}")
    } else {
        joined
    })
}

/// Decide whether a link target should be considered "resolved". A link is OK if:
/// - The exact path exists (file or directory)
/// - For `.html` links: the corresponding rendered file under `docs/book/` exists
///   (rustdoc cross-refs from `src/sdk/README.md` use `../rustdoc/index.html` which
///   only materialises after `mdbook build` + the SDK rustdoc copy step)
/// - For paths under `docs/src/grpc/`: the file is allowed to be absent at scan
///   time if the path matches a known auto-generated proto-md pattern; but in
///   practice we always run xtask in order so the grpc/* files exist by step 7.
fn link_target_ok(root: &Path, target: &Path) -> bool {
    if target.exists() {
        return true;
    }
    // For `.html` links, try the corresponding rendered file under docs/book/.
    let s = target.to_string_lossy();
    if let Some(rel) = s.strip_prefix(&format!("{}/docs/src/", root.display())) {
        let book_path = root.join("docs/book").join(rel);
        if book_path.exists() {
            return true;
        }
    }
    false
}

/// All workspace crates that should have a `cargo-rdme`-generated README.md, by
/// `package.name` in Cargo.toml (NOT directory name — `sdks/rust/Cargo.toml`'s package
/// name is `stitchd-sdk-rust`).
const CRATE_README_TARGETS: &[&str] = &[
    "stitchd-analytics-service",
    "stitchd-auth-service",
    "stitchd-core",
    "stitchd-db",
    "stitchd-event-writer",
    "stitchd-experimentation-service",
    "stitchd-flag-service",
    "stitchd-gateway",
    "stitchd-proto",
    "stitchd-segmentation-service",
    "stitchd-stats-service",
    "stitchd-sdk-rust",
    "xtask",
];

fn generate_crate_readmes(root: &Path) -> Result<()> {
    println!("Regenerating crate READMEs via cargo-rdme");
    for name in CRATE_README_TARGETS {
        let status = Command::new("cargo")
            .args([
                "rdme",
                "--workspace-project",
                name,
                "--intralinks-strip-links",
            ])
            .current_dir(root)
            .status()
            .with_context(|| format!("failed to run cargo-rdme for {name}"))?;
        anyhow::ensure!(
            status.success(),
            "`cargo rdme --workspace-project {name}` exited with {status}"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 1: gRPC / Protobuf documentation
// ---------------------------------------------------------------------------

fn generate_grpc_docs(root: &Path) -> Result<()> {
    let proto_dir = root.join("proto");
    let out_dir = root.join("docs/src/grpc");
    std::fs::create_dir_all(&out_dir).context("failed to create docs/src/grpc/")?;

    println!("Generating gRPC docs → {}", out_dir.display());

    let mut proto_files: Vec<PathBuf> = Vec::new();
    collect_proto_files(&proto_dir, &mut proto_files)?;
    proto_files.sort();

    let mut all_chapters: Vec<(String, String)> = Vec::new(); // (filename, title)

    for proto_path in &proto_files {
        let source = std::fs::read_to_string(proto_path)
            .with_context(|| format!("failed to read {}", proto_path.display()))?;

        let md = proto_to_markdown(&source, proto_path);

        // Derive output filename from proto path relative to proto_dir
        let rel = proto_path.strip_prefix(&proto_dir).unwrap();
        let md_name = rel
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "_")
            .replace(".proto", ".md");

        let out_path = out_dir.join(&md_name);
        std::fs::write(&out_path, &md)
            .with_context(|| format!("failed to write {}", out_path.display()))?;

        // Extract title (first H1 heading)
        let title = md
            .lines()
            .find(|l| l.starts_with("# "))
            .map(|l| l.trim_start_matches("# ").to_string())
            .unwrap_or_else(|| md_name.trim_end_matches(".md").to_string());

        all_chapters.push((md_name, title));
    }

    // Write README.md as the chapter index
    let readme = build_grpc_readme(&all_chapters);
    std::fs::write(out_dir.join("README.md"), readme)
        .context("failed to write docs/src/grpc/README.md")?;

    // Patch SUMMARY.md: replace the gRPC section with per-proto entries
    patch_summary_grpc(root, &all_chapters)?;

    Ok(())
}

/// Replace the `# Internal gRPC Services` section in SUMMARY.md with
/// domain-grouped entries (Auth & Identity, Flag & Segmentation, Events & Experimentation).
fn patch_summary_grpc(root: &Path, chapters: &[(String, String)]) -> Result<()> {
    let summary_path = root.join("docs/src/SUMMARY.md");
    let content =
        std::fs::read_to_string(&summary_path).context("failed to read docs/src/SUMMARY.md")?;

    // Categorise chapters by domain prefix
    let mut auth_identity: Vec<&(String, String)> = Vec::new();
    let mut flag_segmentation: Vec<&(String, String)> = Vec::new();
    let mut events_experimentation: Vec<&(String, String)> = Vec::new();

    for chapter in chapters {
        let file = &chapter.0;
        if file.starts_with("auth_") || file.starts_with("management_") {
            auth_identity.push(chapter);
        } else if file.starts_with("flags_")
            || file.starts_with("segments_")
            || file.starts_with("common_")
        {
            flag_segmentation.push(chapter);
        } else {
            // events_, experiments_, and anything else
            events_experimentation.push(chapter);
        }
    }

    // Build the replacement section — domain-ordered flat list under the README
    let mut new_section = String::from("# Internal gRPC Services\n\n");
    new_section.push_str("- [gRPC Services](./grpc/README.md)\n");
    for (file, title) in auth_identity
        .iter()
        .chain(flag_segmentation.iter())
        .chain(events_experimentation.iter())
    {
        new_section.push_str(&format!("  - [{title}](./grpc/{file})\n"));
    }
    new_section.push('\n'); // blank line before next section

    // Locate the section by its heading and replace until the next `#` heading
    let grpc_heading = "# Internal gRPC Services";
    if let Some(start) = content.find(grpc_heading) {
        let after = &content[start + grpc_heading.len()..];
        let end_offset = after
            .find("\n# ")
            .map(|p| start + grpc_heading.len() + p + 1)
            .unwrap_or(content.len());

        let new_content = format!(
            "{}{}{}",
            &content[..start],
            new_section,
            &content[end_offset..]
        );
        std::fs::write(&summary_path, new_content)
            .context("failed to write docs/src/SUMMARY.md")?;
    }

    Ok(())
}

fn collect_proto_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_proto_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("proto") {
            out.push(path);
        }
    }
    Ok(())
}

fn build_grpc_readme(chapters: &[(String, String)]) -> String {
    let mut md = String::new();
    md.push_str("# Internal gRPC Services\n\n");
    md.push_str("Auto-generated from `.proto` files in the `proto/` directory.\n");
    md.push_str("Run `cargo xtask docs` to regenerate.\n\n");

    // Domain groups: Auth & Identity
    md.push_str("## Auth & Identity\n\n");
    for (file, title) in chapters
        .iter()
        .filter(|(f, _)| f.starts_with("auth_") || f.starts_with("management_"))
    {
        md.push_str(&format!("- [{}]({})\n", title, file));
    }
    md.push('\n');

    // Flag & Segmentation
    md.push_str("## Flag & Segmentation\n\n");
    for (file, title) in chapters.iter().filter(|(f, _)| {
        f.starts_with("flags_") || f.starts_with("segments_") || f.starts_with("common_")
    }) {
        md.push_str(&format!("- [{}]({})\n", title, file));
    }
    md.push('\n');

    // Events & Experimentation
    md.push_str("## Events & Experimentation\n\n");
    for (file, title) in chapters.iter().filter(|(f, _)| {
        !f.starts_with("auth_")
            && !f.starts_with("management_")
            && !f.starts_with("flags_")
            && !f.starts_with("segments_")
            && !f.starts_with("common_")
    }) {
        md.push_str(&format!("- [{}]({})\n", title, file));
    }

    md
}

/// Parse a single .proto file and produce a Markdown document.
fn proto_to_markdown(source: &str, path: &Path) -> String {
    let mut md = String::new();

    // Title from filename
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let title = stem
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    md.push_str(&format!("# {title}\n\n"));
    md.push_str("> Auto-generated from `");
    md.push_str(&path.to_string_lossy());
    md.push_str("`\n\n");

    // Extract package
    if let Some(pkg_line) = source.lines().find(|l| l.starts_with("package ")) {
        let pkg = pkg_line
            .trim_start_matches("package ")
            .trim_end_matches(';');
        md.push_str(&format!("**Package:** `{pkg}`\n\n"));
    }

    // Parse services, messages, and enums in order of appearance
    let mut pending_comment = String::new();
    let mut i = 0;
    let lines: Vec<&str> = source.lines().collect();

    while i < lines.len() {
        let line = lines[i].trim();

        // Accumulate `//` comments
        if line.starts_with("//") {
            let comment = line.trim_start_matches('/').trim();
            if pending_comment.is_empty() {
                pending_comment.push_str(comment);
            } else {
                pending_comment.push('\n');
                pending_comment.push_str(comment);
            }
            i += 1;
            continue;
        }

        if line.starts_with("service ") {
            let name = extract_name(line, "service ");
            md.push_str(&format!("## Service: `{name}`\n\n"));
            if !pending_comment.is_empty() {
                md.push_str(&format!("{}\n\n", pending_comment.trim()));
                pending_comment.clear();
            }
            // Parse RPCs inside the service block
            i += 1;
            while i < lines.len() {
                let inner = lines[i].trim();
                if inner.starts_with("//") {
                    let c = inner.trim_start_matches('/').trim();
                    if pending_comment.is_empty() {
                        pending_comment.push_str(c);
                    } else {
                        pending_comment.push('\n');
                        pending_comment.push_str(c);
                    }
                } else if inner.starts_with("rpc ") {
                    let rpc_doc = parse_rpc(inner);
                    md.push_str("### ");
                    md.push_str(&rpc_doc.name);
                    md.push_str("\n\n");
                    if !pending_comment.is_empty() {
                        md.push_str(&format!("{}\n\n", pending_comment.trim()));
                        pending_comment.clear();
                    }
                    md.push_str(&format!(
                        "- **Request:** `{}`\n- **Response:** `{}`\n\n",
                        rpc_doc.request, rpc_doc.response
                    ));
                } else if inner == "}" {
                    pending_comment.clear();
                    break;
                } else {
                    pending_comment.clear();
                }
                i += 1;
            }
        } else if line.starts_with("message ") {
            let name = extract_name(line, "message ");
            md.push_str(&format!("## Message: `{name}`\n\n"));
            if !pending_comment.is_empty() {
                md.push_str(&format!("{}\n\n", pending_comment.trim()));
                pending_comment.clear();
            }
            // Parse fields
            md.push_str("| Field | Type | Description |\n");
            md.push_str("|-------|------|-------------|\n");
            i += 1;
            let mut field_comment = String::new();
            let mut depth = 1i32;
            while i < lines.len() && depth > 0 {
                let inner = lines[i].trim();
                if inner.starts_with("//") {
                    let c = inner.trim_start_matches('/').trim();
                    if field_comment.is_empty() {
                        field_comment.push_str(c);
                    } else {
                        field_comment.push(' ');
                        field_comment.push_str(c);
                    }
                } else if inner.contains('{') {
                    depth += 1;
                    field_comment.clear();
                } else if inner == "}" {
                    depth -= 1;
                    field_comment.clear();
                } else if !inner.is_empty() && !inner.starts_with("//") {
                    if let Some(field) = parse_field(inner) {
                        let inline_comment = field.inline_comment.unwrap_or_default();
                        let desc = if !field_comment.is_empty() {
                            let s = field_comment.clone();
                            field_comment.clear();
                            s
                        } else {
                            inline_comment
                        };
                        md.push_str(&format!(
                            "| `{}` | `{}` | {} |\n",
                            field.name, field.ty, desc
                        ));
                    } else {
                        field_comment.clear();
                    }
                } else {
                    field_comment.clear();
                }
                i += 1;
            }
            md.push('\n');
        } else if line.starts_with("enum ") {
            let name = extract_name(line, "enum ");
            md.push_str(&format!("## Enum: `{name}`\n\n"));
            if !pending_comment.is_empty() {
                md.push_str(&format!("{}\n\n", pending_comment.trim()));
                pending_comment.clear();
            }
            md.push_str("| Value | Description |\n");
            md.push_str("|-------|-------------|\n");
            i += 1;
            let mut val_comment = String::new();
            while i < lines.len() {
                let inner = lines[i].trim();
                if inner.starts_with("//") {
                    let c = inner.trim_start_matches('/').trim();
                    if val_comment.is_empty() {
                        val_comment.push_str(c);
                    } else {
                        val_comment.push(' ');
                        val_comment.push_str(c);
                    }
                } else if inner == "}" {
                    val_comment.clear();
                    break;
                } else if !inner.is_empty() {
                    let val_name = inner.split_whitespace().next().unwrap_or(inner);
                    let inline = extract_inline_comment(inner);
                    let desc = if !val_comment.is_empty() {
                        let s = val_comment.clone();
                        val_comment.clear();
                        s
                    } else {
                        inline
                    };
                    md.push_str(&format!("| `{val_name}` | {desc} |\n"));
                } else {
                    val_comment.clear();
                }
                i += 1;
            }
            md.push('\n');
        } else {
            pending_comment.clear();
        }

        i += 1;
    }

    md
}

fn extract_name<'a>(line: &'a str, prefix: &str) -> &'a str {
    line.trim_start_matches(prefix)
        .split([' ', '{'])
        .next()
        .unwrap_or(line)
        .trim()
}

struct RpcInfo {
    name: String,
    request: String,
    response: String,
}

fn parse_rpc(line: &str) -> RpcInfo {
    // rpc Sync(SyncRequest) returns (SyncResponse);
    let name = line
        .trim_start_matches("rpc ")
        .split('(')
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();
    let request = line
        .split('(')
        .nth(1)
        .and_then(|s| s.split(')').next())
        .unwrap_or("?")
        .trim()
        .to_string();
    let response = line
        .split("returns")
        .nth(1)
        .and_then(|s| s.split('(').nth(1))
        .and_then(|s| s.split(')').next())
        .unwrap_or("?")
        .trim()
        .to_string();
    RpcInfo {
        name,
        request,
        response,
    }
}

struct FieldInfo {
    name: String,
    ty: String,
    inline_comment: Option<String>,
}

fn parse_field(line: &str) -> Option<FieldInfo> {
    // Strip inline comment first
    let (code, comment) = if let Some(pos) = line.find("//") {
        (&line[..pos], Some(line[pos + 2..].trim().to_string()))
    } else {
        (line, None)
    };

    let code = code.trim().trim_end_matches(';');
    let parts: Vec<&str> = code.split_whitespace().collect();

    // proto3 field forms:
    // repeated Type name = N;
    // map<K, V> name = N;
    // Type name = N;
    // oneof ... { } — skip
    if parts.len() < 3 {
        return None;
    }
    if parts[0] == "oneof" || parts[0] == "option" || parts[0] == "reserved" {
        return None;
    }

    let (ty, name) = if parts[0] == "repeated" {
        (format!("repeated {}", parts[1]), parts[2])
    } else if parts[0] == "map" || parts[0].starts_with("map<") {
        // map<K, V> name = N
        let (before, after) = code.split_once('>').unwrap_or(("map", ""));
        let map_type = format!("{}>", before.trim());
        let rest = after.trim();
        let name = rest.split_whitespace().next().unwrap_or("?");
        (map_type, name)
    } else {
        (parts[0].to_string(), parts[1])
    };

    Some(FieldInfo {
        name: name.to_string(),
        ty,
        inline_comment: comment,
    })
}

fn extract_inline_comment(line: &str) -> String {
    if let Some(pos) = line.find("//") {
        line[pos + 2..].trim().to_string()
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------------
// Step 3: Environment-variable reference (scraped from source)
// ---------------------------------------------------------------------------

/// One discovered environment-variable usage site.
#[derive(Debug, Clone)]
struct EnvVarInfo {
    name: String,
    crate_name: String,
    default: Option<String>,
    required: bool,
}

fn generate_env_vars(root: &Path) -> Result<()> {
    let out_path = root.join("docs/src/deployment/env-vars.md");
    println!("Generating env-vars → {}", out_path.display());

    let mut vars: BTreeMap<String, EnvVarInfo> = BTreeMap::new();

    // Scan both crates/ and sdks/ for STITCHD_ env-var usage.
    for scan_root in [root.join("crates"), root.join("sdks")] {
        if scan_root.is_dir() {
            scan_rust_files_for_env_vars(&scan_root, &scan_root, &mut vars)?;
        }
    }

    let md = render_env_vars_md(&vars);
    std::fs::write(&out_path, md)
        .with_context(|| format!("failed to write {}", out_path.display()))?;
    Ok(())
}

fn scan_rust_files_for_env_vars(
    base: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, EnvVarInfo>,
) -> Result<()> {
    // `std::fs::read_dir` returns entries in **filesystem-dependent** order
    // (APFS is alphabetical, ext4 is insertion/inode order, network filesystems
    // are arbitrary). Because `record_var()` uses "first sighting wins" to pick
    // the crate name for a shared env var, an unsorted walk produces different
    // output on different filesystems. Sort entries by path so the walk is
    // deterministic across local + CI + future runners.
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read dir {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.path())
        .collect();
    entries.sort();

    for path in entries {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

        // Skip target/, hidden dirs, tests/ and examples/ (env vars in tests + examples
        // aren't deployable config surface).
        if path.is_dir() {
            if name == "target" || name == "tests" || name == "examples" || name.starts_with('.') {
                continue;
            }
            scan_rust_files_for_env_vars(base, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let source = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let crate_name = crate_name_from_path(base, &path);
            extract_env_vars(&source, &crate_name, out);
        }
    }
    Ok(())
}

/// Derive the crate name from a source path under `crates/` or `sdks/`.
/// For `sdks/`, prefix the crate dir with `sdks/` so `sdks/rust` doesn't collide with
/// a hypothetical top-level `rust` crate.
fn crate_name_from_path(base: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(base).unwrap_or(file);
    let first = rel
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("?");
    let base_name = base.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if base_name == "sdks" {
        format!("sdks/{first}")
    } else {
        first.to_string()
    }
}

/// Parse a Rust source file for `env::var("STITCHD_X")` and `env_or("STITCHD_X", "default")`
/// calls. Records var name, originating crate, parsed default (if literal), and whether
/// the call surface marks it required (`.map_err(...)`) vs optional (`.ok()` / `.unwrap_or*`).
fn extract_env_vars(source: &str, crate_name: &str, out: &mut BTreeMap<String, EnvVarInfo>) {
    // Drop the file's `//` line comments before scanning so doc-comment mentions
    // of an env var don't get parsed as call sites.
    let stripped = strip_line_comments(source);

    // Pattern 1: env::var("STITCHD_X")  (may be prefixed with `std::`)
    let mut i = 0;
    let bytes = stripped.as_bytes();
    while i < bytes.len() {
        if let Some(pos) = find_pattern(&stripped[i..], "env::var(\"STITCHD_") {
            let absolute = i + pos;
            let after_open = absolute + "env::var(\"".len();
            if let Some(close_rel) = stripped[after_open..].find('"') {
                let var_name = &stripped[after_open..after_open + close_rel];
                // Skip past the closing `"` and the env::var `)` — start the chain
                // walker at depth 0, just after the call expression.
                let close_quote = after_open + close_rel;
                let chain_start = stripped[close_quote..]
                    .find(')')
                    .map(|p| close_quote + p + 1)
                    .unwrap_or(close_quote + 1);
                let chain_end = find_statement_end(&stripped, chain_start);
                let context = &stripped[chain_start..chain_end];
                let (default, required) = parse_env_var_chain(context, &stripped);
                record_var(out, var_name, crate_name, default, required);
                i = chain_start;
                continue;
            }
        }
        break;
    }

    // Pattern 2: env_or("STITCHD_X", "default")  (gateway helper; may be multi-line)
    let mut j = 0;
    while j < bytes.len() {
        if let Some(pos) = find_pattern(&stripped[j..], "env_or(\"STITCHD_") {
            let absolute = j + pos;
            let after_open = absolute + "env_or(\"".len();
            if let Some(close_rel) = stripped[after_open..].find('"') {
                let var_name = &stripped[after_open..after_open + close_rel];
                let after_name = after_open + close_rel + 1;
                // Scan forward for the second string literal (the default).
                let default = extract_next_string_literal(&stripped[after_name..]);
                record_var(out, var_name, crate_name, default, false);
                j = after_name;
                continue;
            }
        }
        break;
    }
}

/// Strip `//` line comments (preserving line structure with whitespace) so that
/// rustdoc references to env vars don't get scanned as call sites.
fn strip_line_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            out.push('\n');
        } else if let Some(pos) = line.find("//") {
            // Be conservative: only strip if `//` is preceded by a space (avoids stripping
            // mid-string `//`, though we accept the rare edge case of `// inside string`).
            let before = &line[..pos];
            if before.ends_with(' ') || before.is_empty() {
                out.push_str(before);
                out.push('\n');
            } else {
                out.push_str(line);
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn find_pattern(haystack: &str, needle: &str) -> Option<usize> {
    haystack.find(needle)
}

/// Walk from `start` forward through `s` and return the byte index of the first
/// statement-terminator at paren-depth 0 (`;`, `,`, or `}`). Used to bound the
/// chain-context window so it stops at the end of the owning statement or
/// struct-literal field. Skips contents of string literals (so a `;` inside a
/// "..." doesn't terminate the scan). Capped at 800 chars from `start`.
fn find_statement_end(s: &str, start: usize) -> usize {
    let cap = (start + 800).min(s.len());
    let slice = &s[start..cap];
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut prev_was_backslash = false;
    for (i, ch) in slice.char_indices() {
        if in_string {
            if ch == '"' && !prev_was_backslash {
                in_string = false;
            }
            prev_was_backslash = ch == '\\' && !prev_was_backslash;
            continue;
        }
        prev_was_backslash = false;
        match ch {
            '"' => in_string = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' => depth -= 1,
            '}' if depth == 0 => return start + i,
            '}' => depth -= 1,
            ';' | ',' if depth == 0 => return start + i,
            _ => {}
        }
    }
    cap
}

/// Look at the chain that follows an `env::var("X")` close-paren — i.e. starting at
/// `&str` containing `)<rest>` — and decide what default (if any) the call site supplies
/// and whether it treats the var as required. The full source is passed so that
/// const-identifier defaults (e.g. `.unwrap_or(DEFAULT_PORT)`) can be resolved by
/// scanning for a matching `const DEFAULT_PORT: ... = LITERAL;` declaration in the file.
fn parse_env_var_chain(context: &str, source: &str) -> (Option<String>, bool) {
    // Required if `.map_err(` appears in the immediate chain.
    let required = context.contains(".map_err(");

    // Default extraction — look for, in order of specificity:
    //   .unwrap_or_else(|_| "X".to_string())
    //   .unwrap_or_else(|_| "X".into())
    //   .unwrap_or("X".to_string())
    //   .unwrap_or("X".into())
    //   .unwrap_or(<numeric>)
    //   .unwrap_or(CONST_IDENT)  (resolves via in-file lookup)
    for marker in [
        ".unwrap_or_else(|_| \"",
        ".unwrap_or_else(|_| String::from(\"",
        ".unwrap_or(\"",
    ] {
        if let Some(pos) = context.find(marker) {
            let start = pos + marker.len();
            if let Some(end) = context[start..].find('"') {
                return (Some(context[start..start + end].to_string()), required);
            }
        }
    }

    // Numeric default: .unwrap_or(NNNN) — pull the integer literal.
    if let Some(pos) = context.find(".unwrap_or(") {
        let start = pos + ".unwrap_or(".len();
        let after = &context[start..];
        let first = after.chars().next();

        // Integer literal (with optional `_` separators).
        if first.is_some_and(|c| c.is_ascii_digit()) {
            let end_rel = after
                .find(|c: char| !c.is_ascii_digit() && c != '_')
                .unwrap_or(after.len());
            let lit: String = after[..end_rel]
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect();
            if !lit.is_empty() {
                return (Some(lit), required);
            }
        }

        // Identifier — try to resolve against an in-file `const IDENT: ... = N;`.
        // If not resolvable, fall through to "no default" rather than printing the
        // raw identifier name.
        if first.is_some_and(|c| c.is_ascii_uppercase() || c == '_') {
            let end_rel = after
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(after.len());
            let ident = &after[..end_rel];
            if !ident.is_empty()
                && let Some(value) = resolve_const_in_source(source, ident)
            {
                return (Some(value), required);
            }
        }
    }

    (None, required)
}

/// Look up a `const <IDENT>: ... = <LITERAL>;` in the source and return the literal.
/// Supports integer literals (with optional `_` separators) and quoted strings.
fn resolve_const_in_source(source: &str, ident: &str) -> Option<String> {
    let needle = format!("const {ident}");
    let pos = source.find(&needle)?;
    let after = &source[pos + needle.len()..];
    let eq = after.find('=')?;
    let tail = after[eq + 1..].trim_start();
    let semi = tail.find(';').unwrap_or(tail.len());
    let value = tail[..semi].trim();

    // Quoted string?
    if value.starts_with('"') && value.len() >= 2 {
        let inner = &value[1..];
        if let Some(end) = inner.find('"') {
            return Some(inner[..end].to_string());
        }
    }

    // Integer literal? (allow trailing type suffix like `_u16`)
    let digits: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '_')
        .filter(|c| c.is_ascii_digit())
        .collect();
    if !digits.is_empty() {
        return Some(digits);
    }

    None
}

/// Extract the next `"..."` string literal from a slice (used for the second arg of
/// `env_or("X", "Y")` which may be on the same line or split across lines).
fn extract_next_string_literal(s: &str) -> Option<String> {
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            if let Some(end_rel) = s[start..].find('"') {
                return Some(s[start..start + end_rel].to_string());
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Insert or merge an env-var sighting into the accumulator.
/// Multiple call sites for the same var: prefer the one with a default + non-required
/// (because a default means "optional with default" at any site).
fn record_var(
    out: &mut BTreeMap<String, EnvVarInfo>,
    name: &str,
    crate_name: &str,
    default: Option<String>,
    required: bool,
) {
    use std::collections::btree_map::Entry;
    match out.entry(name.to_string()) {
        Entry::Vacant(slot) => {
            slot.insert(EnvVarInfo {
                name: name.to_string(),
                crate_name: crate_name.to_string(),
                default,
                required,
            });
        }
        Entry::Occupied(mut slot) => {
            let existing = slot.get_mut();
            // Promote the most-specific known default.
            if existing.default.is_none() && default.is_some() {
                existing.default = default;
            }
            // If ANY call site is required-with-no-default, mark required.
            if required && existing.default.is_none() {
                existing.required = true;
            }
            // Don't overwrite the crate_name — first sighting wins (sorted scan).
        }
    }
}

/// Render the env-vars markdown page. Grouped by crate so each service's surface
/// is co-located. Crate ordering follows BTreeMap order (alphabetical), which is
/// stable across runs (deterministic = idempotent).
fn render_env_vars_md(vars: &BTreeMap<String, EnvVarInfo>) -> String {
    let mut md = String::new();
    md.push_str("# Environment Variables\n\n");
    md.push_str(
        "> **AUTO-GENERATED** by `cargo xtask docs`. To add or modify variables, change \
the source code in `crates/*/src/` or `sdks/*/src/`, then re-run `cargo xtask docs`. \
Do NOT hand-edit this file — your changes will be overwritten.\n\n",
    );
    md.push_str(
        "All configuration is passed via environment variables. There are no config files.\n\n",
    );
    md.push_str(
        "> **Naming convention:** Stitchd-owned variables carry the `STITCHD_` prefix. \
The sole exception is `RUST_LOG`, which follows the Rust ecosystem standard. \
Service port variables follow `STITCHD_<SERVICE>_GRPC_PORT` / `STITCHD_<SERVICE>_METRICS_PORT`; \
the gateway adds `STITCHD_GATEWAY_HTTP_PORT` and `STITCHD_GATEWAY_GRPC_PORT`.\n\n",
    );
    md.push_str(&format!(
        "_{} variables discovered across the workspace._\n\n",
        vars.len()
    ));

    // Group by crate.
    let mut by_crate: BTreeMap<String, Vec<&EnvVarInfo>> = BTreeMap::new();
    for v in vars.values() {
        by_crate.entry(v.crate_name.clone()).or_default().push(v);
    }
    for entries in by_crate.values_mut() {
        entries.sort_by_key(|v| v.name.clone());
    }

    md.push_str("## Variables by Crate\n\n");
    for (crate_name, entries) in &by_crate {
        md.push_str(&format!("### `{crate_name}`\n\n"));
        md.push_str("| Variable | Default | Required |\n");
        md.push_str("|----------|---------|----------|\n");
        for v in entries {
            let default_cell = match &v.default {
                Some(d) if !d.is_empty() => format!("`{d}`"),
                Some(_) => "_(empty string)_".to_string(),
                None => "—".to_string(),
            };
            let required_cell = if v.required {
                "**required**"
            } else if v.default.is_some() {
                "no (has default)"
            } else {
                "optional"
            };
            md.push_str(&format!(
                "| `{}` | {} | {} |\n",
                v.name, default_cell, required_cell
            ));
        }
        md.push('\n');
    }

    md.push_str("## Externally-Managed Variables\n\n");
    md.push_str("| Variable | Description |\n");
    md.push_str("|----------|-------------|\n");
    md.push_str(
        "| `RUST_LOG` | Log level filter, e.g. `info` or `info,stitchd_gateway=debug`. \
Follows the `tracing-subscriber` directive format. |\n",
    );
    md.push_str(
        "| `DATABASE_URL` | sqlx-cli only — alias of `STITCHD_DATABASE_URL`. \
See `conductor/workflow.md` (Setup). |\n",
    );
    md.push('\n');

    md.push_str("## Log Levels\n\n");
    md.push_str(
        "`RUST_LOG` follows the [`tracing-subscriber` directive format]\
(https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html):\n\n",
    );
    md.push_str("```\n# All info, verbose for stitchd crates\nRUST_LOG=info,stitchd_gateway=debug,stitchd_core=debug\n\n# Quiet mode\nRUST_LOG=warn\n```\n");

    md
}

// ---------------------------------------------------------------------------
// Step 2: OpenAPI JSON export
// ---------------------------------------------------------------------------

fn export_openapi(root: &Path) -> Result<()> {
    let out_path = root.join("docs/src/api/openapi.json");
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).context("failed to create docs/src/api/")?;
    }

    println!("Exporting OpenAPI JSON → {}", out_path.display());

    // Build the gateway binary first (no-op if already up to date).
    let build_status = Command::new("cargo")
        .args(["build", "-p", "stitchd-gateway"])
        .current_dir(root)
        .status()
        .context("failed to run `cargo build -p stitchd-gateway`")?;
    anyhow::ensure!(
        build_status.success(),
        "`cargo build` exited with {build_status}"
    );

    // Run the binary with --export-openapi <path>.
    let binary = root.join("target/debug/stitchd-gateway");
    let export_status = Command::new(&binary)
        .args(["--export-openapi", out_path.to_str().unwrap()])
        .current_dir(root)
        .status()
        .with_context(|| format!("failed to run `{}`", binary.display()))?;
    anyhow::ensure!(
        export_status.success(),
        "`stitchd-gateway --export-openapi` exited with {export_status}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 3: SDK rustdoc
// ---------------------------------------------------------------------------

fn generate_sdk_rustdoc(root: &Path) -> Result<()> {
    let out_dir = root.join("docs/book/rustdoc");
    println!("Generating SDK rustdoc → {}", out_dir.display());

    let status = Command::new("cargo")
        .args(["doc", "--no-deps", "-p", "stitchd-sdk-rust"])
        .current_dir(root)
        .status()
        .context("failed to run `cargo doc`")?;
    anyhow::ensure!(status.success(), "`cargo doc` exited with {status}");

    // Copy target/doc/stitchd_sdk_rust → docs/book/rustdoc/. Caller runs us AFTER
    // mdbook build so this directory survives the mdbook clean step.
    let src = root.join("target/doc/stitchd_sdk_rust");
    if src.exists() {
        // Wipe any prior copy to keep this step idempotent.
        if out_dir.exists() {
            std::fs::remove_dir_all(&out_dir).ok();
        }
        copy_dir_all(&src, &out_dir)
            .with_context(|| format!("failed to copy rustdoc from {}", src.display()))?;
    }

    Ok(())
}

/// Wrapper to keep the `docs()` step list readable. Delegates to [`extract_quickstart`].
fn extract_sdk_quickstart(root: &Path) -> Result<()> {
    extract_quickstart(root)
}

/// Extract the `# Quickstart` section from `sdks/rust/src/lib.rs` module docs
/// and write it to `docs/src/sdk/quickstart.md`.
fn extract_quickstart(root: &Path) -> Result<()> {
    let lib_rs = root.join("sdks/rust/src/lib.rs");
    let source = std::fs::read_to_string(&lib_rs).context("failed to read sdks/rust/src/lib.rs")?;

    // Collect `//!` lines and strip the prefix
    let doc_lines: Vec<&str> = source
        .lines()
        .filter(|l| l.starts_with("//!"))
        .map(|l| {
            let stripped = l.trim_start_matches("//!");
            stripped.strip_prefix(' ').unwrap_or(stripped)
        })
        .collect();

    // Find `# Quickstart` heading and collect until next `# ` heading or end
    let start = doc_lines.iter().position(|l| *l == "# Quickstart");
    if let Some(start) = start {
        let end = doc_lines[start + 1..]
            .iter()
            .position(|l| l.starts_with("# "))
            .map(|p| start + 1 + p)
            .unwrap_or(doc_lines.len());

        let quickstart_body = doc_lines[start..end].join("\n");
        let out = format!(
            "# SDK Quickstart\n\n> Auto-extracted from `sdks/rust/src/lib.rs` module docs.\n> Run `cargo xtask docs` to regenerate.\n\n{}\n",
            &quickstart_body["# Quickstart".len()..].trim_start()
        );

        let out_path = root.join("docs/src/sdk/quickstart.md");
        std::fs::write(&out_path, out).context("failed to write docs/src/sdk/quickstart.md")?;
    }

    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 4: mdBook build
// ---------------------------------------------------------------------------

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

/// Same as [`ensure_tool`] but for tools whose binary name is `cargo-<X>` and that are
/// invoked as `cargo <X>`. Checks via `cargo <X> --version` instead of `which`.
fn ensure_cargo_subcommand(crate_name: &str, version: &str) -> Result<()> {
    // crate_name is e.g. "cargo-rdme"; subcommand is the part after the dash.
    let subcommand = crate_name.strip_prefix("cargo-").unwrap_or(crate_name);
    let check = Command::new("cargo")
        .args([subcommand, "--version"])
        .output();
    if matches!(check, Ok(o) if o.status.success()) {
        return Ok(());
    }
    println!("Installing {crate_name}@{version} via `cargo install`…");
    let status = Command::new("cargo")
        .args(["install", "--locked", "--version", version, crate_name])
        .status()
        .with_context(|| format!("failed to run `cargo install {crate_name}`"))?;
    anyhow::ensure!(
        status.success(),
        "`cargo install {crate_name}` exited with {status}"
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
