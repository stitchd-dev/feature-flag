# Track Spec: Feature Flag Evaluation & Segmentation Core

## 1. Goal
Implement the core rule evaluation engine, rule-based and list-based segmentation logic, and the associated data structures in `stitchd-core` and `stitchd-db`.

## 2. Requirements

### 2.1 Rule Engine
- Implement a first-true-exit rule evaluation engine.
- Support core condition operators (`==`, `!=`, `<`, `>`, `IN`, `NOT IN`, `CONTAINS`, `REGEX`).
- Support complex rule sets with `AND` combinators and a top-level `NOT`.
- Implement evaluation context handling, including type conversion (int, double, semver, string, boolean).

### 2.2 Segmentation
- Support rule-based segments (reusing the rule engine).
- Support list-based segments (include/exclude lists per context-type).
- Implement persistent storage for segment configurations and list entries in PostgreSQL.
- Ensure efficient lookup for list-based segments using partitioning or optimized indexing.

### 2.3 Evaluation Integration
- Link flags to rules and segments.
- Implement the "Is in Segment" rule capability.
- Support deterministic variant allocation based on hashing (SipHasher).

## 3. Tech Stack Reference
- **Language:** Rust 2024
- **Persistence:** PostgreSQL with SQLx
- **Data Types:** UUID, Chrono, SemVer
- **Serialization:** Serde / Serde JSON

## 4. Verification Criteria
- [ ] Rule engine correctly evaluates complex contexts against nested rule sets.
- [ ] Segments can be created, updated, and correctly linked to flags.
- [ ] List-based segments correctly filter contexts based on explicit ID lists.
- [ ] High-performance evaluation with minimal latency.
- [ ] All code covered by unit and integration tests (>=90% coverage).
