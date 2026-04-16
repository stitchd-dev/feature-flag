# Rust Style Guide

## Toolchain & Enforcement
- **Edition:** Rust 2024
- **Formatter:** `rustfmt` — enforced in CI (`cargo fmt --check`)
- **Linter:** `clippy` — enforced in CI (`cargo clippy -- -D warnings`)
- **MSRV:** Pin in `Cargo.toml` via `rust-version`
- **Deny in lib roots:**
  ```rust
  #![deny(warnings, missing_docs, clippy::all)]
  #![warn(clippy::pedantic, clippy::nursery)]
  ```

---

## Project & Module Structure

### Workspace Layout
```
/
├── crates/
│   ├── stitchd-core/        # Domain types, rule engine, shared models
│   ├── stitchd-server/      # Axum REST + tonic gRPC server
│   ├── stitchd-db/          # sqlx queries, migrations, repo layer
│   ├── stitchd-events/      # ClickHouse event ingestion
│   ├── stitchd-sdk/         # Rust client SDK
│   └── stitchd-proto/       # Protobuf generated code (prost)
└── Cargo.toml               # Workspace root
```

### Module Rules
- One concern per module; avoid `mod.rs` — prefer `module_name.rs` + directory
- Keep `lib.rs` as a thin re-export surface only
- Put integration tests in `tests/` at crate root; unit tests in `#[cfg(test)]` inline
- Feature-gate optional dependencies: `#[cfg(feature = "...")]`

---

## Naming Conventions

| Item | Convention | Example |
|---|---|---|
| Types, Traits, Enums | `UpperCamelCase` | `FeatureFlag`, `RuleEngine` |
| Functions, methods, variables | `snake_case` | `evaluate_flag`, `context_key` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_VARIANTS` |
| Lifetimes | Short lowercase | `'a`, `'ctx` |
| Type parameters | Single upper or descriptive | `T`, `K`, `ContextKey` |
| Modules | `snake_case` | `rule_engine`, `segment` |
| Crates | `kebab-case` | `stitchd-core` |

### Semantic Naming
- Prefer descriptive names over short abbreviations (`environment` not `env`, `parameter` not `param`) except in very local scopes
- Boolean variables/fields: prefix with `is_`, `has_`, `can_`, `should_`
- Builder methods that consume `self`: return `Self`
- Conversion methods: follow `as_`, `to_`, `into_` conventions per Rust API guidelines

---

## Types & Data Modelling

### Prefer Newtypes for Domain Primitives
```rust
// Prefer this:
pub struct FlagKey(String);
pub struct EnvironmentId(Uuid);
pub struct TenantId(Uuid);

// Over this:
pub fn get_flag(flag_key: String, env_id: Uuid) { ... }
```

### Use Enums Exhaustively
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagValueType {
    Bool,
    Int,
    Double,
    String,
    Json,
}

// Always match exhaustively — avoid `_ =>` catch-alls unless explicitly justified
match flag_type {
    FlagValueType::Bool => { ... }
    FlagValueType::Int => { ... }
    FlagValueType::Double => { ... }
    FlagValueType::String => { ... }
    FlagValueType::Json => { ... }
}
```

### Builder Pattern for Complex Structs
```rust
#[derive(Debug, Default)]
pub struct FeatureFlagBuilder {
    key: Option<FlagKey>,
    value_type: Option<FlagValueType>,
    variants: Vec<Variant>,
}

impl FeatureFlagBuilder {
    pub fn key(mut self, key: FlagKey) -> Self { self.key = Some(key); self }
    pub fn value_type(mut self, t: FlagValueType) -> Self { self.value_type = Some(t); self }
    pub fn variant(mut self, v: Variant) -> Self { self.variants.push(v); self }
    pub fn build(self) -> Result<FeatureFlag, BuildError> { ... }
}
```

### Avoid Primitive Obsession
- Domain IDs must be newtypes wrapping `Uuid`, not raw `String` or `Uuid`
- Context parameter values must use the typed enum:
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterValue {
    Int(i64),
    Double(f64),
    SemVer(semver::Version),
    String(String),
    Boolean(bool),
}
```

---

## Error Handling

### Library Crates — `thiserror`
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuleEngineError {
    #[error("context type `{0}` not found")]
    ContextNotFound(String),
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}
```

### Application / Binary Crates — `anyhow`
```rust
use anyhow::{Context, Result};

fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config from {}", path.display()))?;
    toml::from_str(&content).context("invalid config format")
}
```

### Rules
- Never use `.unwrap()` or `.expect()` in library or server code — only in tests and infallible known-safe cases with a comment
- Propagate errors with `?`; add context with `.with_context()`
- Define one error enum per module boundary, not per function
- Never silently discard errors — at minimum log them

---

## Async Patterns (Tokio)

### Runtime
- Use `#[tokio::main]` only at binary entry points
- Use `tokio::spawn` for fire-and-forget background tasks; hold `JoinHandle` if you care about completion
- Prefer `tokio::select!` over nested futures for race conditions

### Avoid Blocking in Async Contexts
```rust
// Wrong — blocks the async executor
let result = std::fs::read_to_string("file.txt")?;

// Correct
let result = tokio::fs::read_to_string("file.txt").await?;

// For CPU-heavy work:
let result = tokio::task::spawn_blocking(|| expensive_computation()).await??;
```

### Async Traits
- Use `async_trait` crate for trait methods that are async until Rust stable supports it natively
- Prefer returning `impl Future` in concrete types where object-safety is not needed

### Cancellation Safety
- Document whether async functions are cancellation-safe
- Avoid holding locks across `.await` points; use `tokio::sync::Mutex` when unavoidable

---

## Ownership & Borrowing

### Prefer Borrowing in Function Signatures
```rust
// Prefer &str over String in function params
fn find_flag(key: &str) -> Option<&FeatureFlag> { ... }

// Prefer &[T] over Vec<T>
fn evaluate_rules(rules: &[Rule]) -> bool { ... }
```

### Clone Discipline
- Don't `.clone()` to satisfy the borrow checker without understanding why
- Prefer `Arc<T>` for shared ownership across threads
- Prefer `Cow<'a, str>` for strings that are sometimes owned, sometimes borrowed

### Lifetimes
- Annotate lifetimes explicitly when the compiler cannot infer them
- Avoid lifetime parameters on structs unless the struct is genuinely a view/slice type
- Prefer owned data in long-lived structs (services, state); borrow only in short-lived request handlers

---

## Traits & Generics

### Trait Design
```rust
// Good — focused, single responsibility
pub trait FlagEvaluator {
    fn evaluate(&self, flag: &FeatureFlag, contexts: &[Context]) -> EvaluationResult;
}

// Avoid mega-traits with many methods — split by concern
```

### Blanket Implementations
- Use sparingly; document the invariant being implemented
- Prefer `impl Trait for ConcreteType` over `impl<T: Bound> Trait for T`

### Generic Bounds
```rust
// Prefer where clauses for readability with multiple bounds
pub fn process<T>(item: T) -> Result<Output>
where
    T: Serialize + Send + Sync + 'static,
{ ... }
```

---

## Concurrency & State

### Shared State
```rust
// Application state passed via Axum's State extractor
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<PgPool>,
    pub flag_store: Arc<dyn FlagStore + Send + Sync>,
    pub config: Arc<Config>,
}
```

### Locking
- Prefer `tokio::sync::RwLock` for read-heavy shared state
- Prefer `tokio::sync::Mutex` for exclusive mutation
- Never hold a lock across an `.await` point unless using tokio-aware locks
- Consider `dashmap` for concurrent hash maps instead of `Mutex<HashMap>`

---

## Database (sqlx)

### Query Style
- Use compile-time checked queries: `sqlx::query_as!` and `sqlx::query!`
- All schema changes via migrations in `crates/stitchd-db/migrations/`
- Name migrations: `YYYYMMDDHHMMSS_descriptive_name.sql`
- No raw string query construction — never format SQL strings

```rust
let flag = sqlx::query_as!(
    FeatureFlagRow,
    r#"SELECT id, key, value_type as "value_type: FlagValueType", enabled
       FROM feature_flags
       WHERE key = $1 AND project_id = $2"#,
    key,
    project_id
)
.fetch_optional(&pool)
.await?;
```

### Repository Pattern
```rust
#[async_trait]
pub trait FlagRepository: Send + Sync {
    async fn find_by_key(&self, key: &FlagKey, project_id: ProjectId) -> Result<Option<FeatureFlag>>;
    async fn list_by_project(&self, project_id: ProjectId) -> Result<Vec<FeatureFlag>>;
    async fn upsert(&self, flag: &FeatureFlag) -> Result<()>;
}

pub struct PgFlagRepository {
    pool: PgPool,
}
```

- One repository per aggregate root
- Repositories take `&self` — pool is internally `Arc`'d
- Return domain types, not DB row types

---

## gRPC (tonic + prost)

### Proto Organisation
```
proto/
├── common/v1/         # Shared types (Context, ParameterValue)
├── flags/v1/          # Feature flag service
├── segments/v1/       # Segmentation service
└── events/v1/         # Event ingestion service
```

### Service Implementation
```rust
#[tonic::async_trait]
impl FlagSyncService for FlagSyncServiceImpl {
    async fn sync(
        &self,
        request: Request<SyncRequest>,
    ) -> Result<Response<SyncResponse>, Status> {
        let sdk_key = extract_sdk_key(&request)?;
        // validate, then delegate to domain service
        let payload = self.flag_service
            .build_client_payload(&sdk_key)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(payload.into()))
    }
}
```

- Map domain errors to `tonic::Status` at the service boundary — never let domain errors leak raw
- Extract SDK key from metadata in a shared interceptor
- Keep service impl thin — delegate to domain services

---

## REST API (Axum)

### Route Organisation
```rust
pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/v1/flags", flags::router())
        .nest("/v1/segments", segments::router())
        .nest("/v1/experiments", experiments::router())
        .layer(AuthLayer::new())
        .with_state(state)
}
```

### Handler Pattern
```rust
pub async fn create_flag(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(body): Json<CreateFlagRequest>,
) -> Result<Json<FlagResponse>, ApiError> {
    body.validate()?;
    let flag = state.flag_service.create(claims.project_id, body.into()).await?;
    Ok(Json(flag.into()))
}
```

### Error Response Envelope
```rust
#[derive(Serialize)]
pub struct ApiError {
    pub code: String,       // machine-readable, e.g. "FLAG_NOT_FOUND"
    pub message: String,    // human-readable
    pub details: Option<serde_json::Value>,
}

impl IntoResponse for ApiError { ... }
```

---

## Testing

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_with_not_operator_inverts_result() {
        let rule = Rule::new(Condition::Always).negate();
        assert!(!rule.evaluate(&context()));
    }
}
```

### Integration Tests
- Place in `crates/<name>/tests/`
- Use `sqlx::test` macro for DB tests — each test gets a fresh transaction
- Use `wiremock` for external HTTP mocking
- Use `tonic` test client for gRPC service tests

### Test Naming
```rust
// Pattern: what_it_does_when_condition
fn returns_default_variant_when_no_rules_match() { ... }
fn rejects_event_with_unknown_key() { ... }
fn percentage_allocation_is_deterministic_for_same_hash_input() { ... }
```

### Coverage Target
- Minimum 90% line coverage enforced in CI via `cargo-tarpaulin`
- Domain logic (rule engine, percentage allocation, segment evaluation) must have near-100% unit coverage

---

## Documentation

- All public types, functions, and trait methods must have `///` doc comments
- Include `# Examples` section for non-trivial public APIs
- Use `#[doc = include_str!("../README.md")]` on crate root for crate-level docs
- Document panics (`# Panics`), errors (`# Errors`), and safety (`# Safety`) sections where applicable

---

## Performance Guidelines

- Prefer stack allocation; avoid unnecessary `Box<T>` for small, short-lived types
- Use `SmallVec` or `ArrayVec` for collections expected to be small (< 8 items)
- Avoid `clone()` in hot paths — profile before optimising
- Use `Arc<str>` or `Arc<[T]>` instead of `Arc<String>` / `Arc<Vec<T>>` for immutable shared data
- Context evaluation is hot-path — rule engine must be allocation-minimal
- Benchmark critical paths with `criterion`

---

## Security

- Never log `privateParameters` fields — enforce at the logging boundary with a wrapper type
- SDK keys must be stored as hashed values (bcrypt or argon2) — never plaintext
- Validate and sanitise all external inputs at API boundaries before passing to domain
- Use `secrecy::Secret<T>` for sensitive values (tokens, keys) to prevent accidental logging
