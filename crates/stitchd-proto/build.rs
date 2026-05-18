use std::path::PathBuf;

fn main() {
    // Use the vendored protoc binary — no system install required.
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc not found");
    // SAFETY: build scripts are single-threaded; no concurrent env access.
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    let proto_root = workspace_root().join("proto");
    // SDK-facing contracts live in the language-agnostic spec directory.
    // sdks/spec/proto/ is included as a separate import path so .proto files
    // there can `import "sdk/v1/service.proto"` while files in proto/ can still
    // `import "common/v1/context.proto"`.
    let sdk_proto_root = workspace_root().join("sdks/spec/proto");

    let proto_files = [
        // Shared types
        proto_root.join("common/v1/context.proto"),
        // Existing SDK-facing services
        proto_root.join("flags/v1/flag_sync.proto"),
        proto_root.join("segments/v1/segment.proto"),
        // New microservice contracts
        proto_root.join("auth/v1/auth_service.proto"),
        proto_root.join("auth/v1/management.proto"),
        proto_root.join("auth/v1/oidc_login.proto"),
        proto_root.join("auth/v1/saml_login.proto"),
        proto_root.join("flags/v1/flag_service.proto"),
        proto_root.join("segments/v1/segmentation_service.proto"),
        proto_root.join("experiments/v1/experimentation_service.proto"),
        proto_root.join("management/v1/management_service.proto"),
        proto_root.join("stats/v1/stats_service.proto"),
        proto_root.join("analytics/v1/analytics.proto"),
        // SDK contracts (under sdks/spec/proto/, owned by sdk_rewrite_20260516)
        sdk_proto_root.join("sdk/v1/service.proto"),
        sdk_proto_root.join("sdk/v1/backend.proto"),
    ];

    // Re-run build script if any proto file changes.
    for path in &proto_files {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&proto_files, &[&proto_root, &sdk_proto_root])
        .expect("failed to compile proto files");
}

/// Resolves the workspace root from the crate manifest directory.
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/stitchd-proto; go up two levels.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("crates/ dir")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
