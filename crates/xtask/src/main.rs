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

    ensure_tool("mdbook", "^0.5")?;
    ensure_tool("mdbook-mermaid", "^0.17")?;

    // Step 1: Generate gRPC reference from .proto files
    generate_grpc_docs(&root)?;
    // Step 2: Export OpenAPI JSON from the server binary
    export_openapi(&root)?;
    // Step 3: Build rustdoc for stitchd-sdk and copy into docs/
    generate_sdk_rustdoc(&root)?;

    // Step 4: build the mdBook site
    mdbook_build(&root)?;

    println!("✓ Documentation built at docs/book/");
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
        .args(["doc", "--no-deps", "-p", "stitchd-sdk"])
        .current_dir(root)
        .status()
        .context("failed to run `cargo doc`")?;
    anyhow::ensure!(status.success(), "`cargo doc` exited with {status}");

    // Copy target/doc/stitchd_sdk → docs/book/rustdoc/
    let src = root.join("target/doc/stitchd_sdk");
    if src.exists() {
        copy_dir_all(&src, &out_dir)
            .with_context(|| format!("failed to copy rustdoc from {}", src.display()))?;
    }

    // Extract the # Quickstart section from lib.rs module doc → quickstart.md
    extract_quickstart(root)?;

    Ok(())
}

/// Extract the `# Quickstart` section from `stitchd-sdk/src/lib.rs` module docs
/// and write it to `docs/src/sdk/quickstart.md`.
fn extract_quickstart(root: &Path) -> Result<()> {
    let lib_rs = root.join("crates/stitchd-sdk/src/lib.rs");
    let source =
        std::fs::read_to_string(&lib_rs).context("failed to read stitchd-sdk/src/lib.rs")?;

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
            "# SDK Quickstart\n\n> Auto-extracted from `stitchd-sdk/src/lib.rs` module docs.\n> Run `cargo xtask docs` to regenerate.\n\n{}\n",
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
