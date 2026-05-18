# 00 — Crate / Package Naming Convention

This document defines the naming rules that every Stitchd SDK MUST follow,
regardless of implementation language. It exists to make multi-language
discoverability predictable for both consumers and maintainers.

---

## 1. Package-Name Pattern

All Stitchd SDK packages MUST be published under the pattern:

```
stitchd-sdk-{language}
```

where `{language}` is the canonical short name listed in
[§ 4 — Supported Languages](#4-supported-languages).

Examples:

| Language | Package name |
|----------|--------------|
| Rust | `stitchd-sdk-rust` |
| TypeScript / JavaScript | `stitchd-sdk-typescript` |
| Python | `stitchd-sdk-python` |
| Go | `stitchd-sdk-go` |
| Java | `stitchd-sdk-java` |
| C# | `stitchd-sdk-csharp` |
| Ruby | `stitchd-sdk-ruby` |

> **Migration note (Rust):** The initial Rust crate was published as
> `stitchd-sdk` before this convention was established. That name is treated as
> an alias for `stitchd-sdk-rust` and SHOULD be deprecated once the renamed
> crate is stable on crates.io.

---

## 2. Directory-Layout Pattern

Within the repository, each SDK MUST live at:

```
sdks/{language}/
```

Concretely:

```
sdks/
├── spec/          Language-agnostic behavioural contract (this directory)
├── rust/          → publishes as stitchd-sdk-rust
├── typescript/    → publishes as stitchd-sdk-typescript   (planned)
├── python/        → publishes as stitchd-sdk-python       (planned)
├── go/            → publishes as stitchd-sdk-go           (planned)
├── java/          → publishes as stitchd-sdk-java         (planned)
├── csharp/        → publishes as stitchd-sdk-csharp       (planned)
└── ruby/          → publishes as stitchd-sdk-ruby         (planned)
```

The directory name MUST exactly match the `{language}` canonical short name
(lower-case, no hyphens or underscores within the name itself).

---

## 3. Language-Specific Package Manager Namespaces

Some ecosystems have their own namespace conventions. The canonical mapping is:

| Language | Package manager | Published identifier |
|----------|-----------------|----------------------|
| Rust | crates.io | `stitchd-sdk-rust` |
| TypeScript | npm | `@stitchd/sdk-typescript` |
| Python | PyPI | `stitchd-sdk-python` |
| Go | Go modules | `github.com/stitchd/sdk-go` |
| Java | Maven Central | `io.stitchd:sdk-java` |
| C# | NuGet | `Stitchd.Sdk.CSharp` |
| Ruby | RubyGems | `stitchd-sdk-ruby` |

### Rationale for the npm scoped package

npm supports both global names (`stitchd-sdk-typescript`) and scoped names
(`@stitchd/sdk-typescript`). Stitchd MUST use the scoped form
`@stitchd/sdk-typescript` because:

- Scoped packages are immune to typosquatting on the global registry.
- The `@stitchd/` scope signals official ownership and allows multiple
  Stitchd packages (e.g. `@stitchd/react`) to share a recognisable namespace.
- The scoped form is already the de-facto standard for SDK packages from
  organisations (e.g. `@aws-sdk/`, `@google-cloud/`, `@opentelemetry/`).

Other ecosystems where a scoped or grouped namespace exists (Maven, NuGet, Go
modules) follow their ecosystem's idiomatic form using the `stitchd` organisation
identifier as the namespace root.

---

## 4. Supported Languages

| Canonical name | Status | Directory | Published identifier |
|----------------|--------|-----------|----------------------|
| `rust` | **ships now** | `sdks/rust/` | `stitchd-sdk-rust` (crates.io) |
| `typescript` | planned | `sdks/typescript/` | `@stitchd/sdk-typescript` (npm) |
| `python` | planned | `sdks/python/` | `stitchd-sdk-python` (PyPI) |
| `go` | planned | `sdks/go/` | `github.com/stitchd/sdk-go` |
| `java` | planned | `sdks/java/` | `io.stitchd:sdk-java` (Maven Central) |
| `csharp` | planned | `sdks/csharp/` | `Stitchd.Sdk.CSharp` (NuGet) |
| `ruby` | planned | `sdks/ruby/` | `stitchd-sdk-ruby` (RubyGems) |

Additional languages MAY be added by opening a spec PR that extends this table
before creating the corresponding `sdks/{language}/` directory.

---

## 5. SDK-Internal Symbol Names

SDK-internal types and functions MUST NOT carry the language suffix.

The language is already expressed by the package name and the import path; adding
it to every symbol creates noise and discourages idiomatic usage.

**Correct (Rust):**

```rust
use stitchd_sdk_rust::SdkClient;
use stitchd_sdk_rust::SdkConfig;
use stitchd_sdk_rust::EvalResult;
```

**Incorrect:**

```rust
use stitchd_sdk_rust::RustSdkClient;   // language suffix is redundant
use stitchd_sdk_rust::RustEvalResult;  // language suffix is redundant
```

The same principle applies in every language: a TypeScript consumer imports
`SdkClient`, not `TypeScriptSdkClient`; a Python consumer uses `SdkClient`,
not `PythonSdkClient`.

---

## 6. Normative Summary

- Implementations MUST use the `stitchd-sdk-{language}` package-name pattern.
- Implementations MUST place source code under `sdks/{language}/`.
- Implementations MUST use the ecosystem-idiomatic namespace form listed in § 3
  when publishing to a package registry.
- Implementations MUST NOT include the language name in exported symbol
  identifiers (types, functions, constants).
- New languages SHOULD be added to § 4 via a spec PR before the SDK directory
  is created.
