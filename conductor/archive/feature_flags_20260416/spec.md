# Specification: Feature Flags Module (Feature)

## Overview
Implement the core Feature Flags module, enabling typed flags, variants, and rule-based evaluation with percentage-based allocations. This module will integrate with the existing Rule Engine and Segmentation logic to provide a flexible targeting system for self-hosted deployments.

## Functional Requirements
1.  **Typed Flag Definitions**: Support for `Boolean`, `Integer`, `Double`, `String`, and `JSON` flag types.
2.  **Variant Management**:
    - Each flag must have at least one variant.
    - All variants must match the flag's declared type.
    - One variant must be designated as the "Default" (Off) variant.
3.  **Evaluation Engine**:
    - Evaluate flag rules in order (first match wins).
    - If no rule matches, return the default variant.
    - Integration with the Rule Engine for "Is in Segment" and targeting conditions.
4.  **Percentage-Based Allocation**:
    - Support allocations with 0.1% granularity.
    - **Hashing Logic**: Use `Flag Key` + `Environment ID` + `Configured Context Parameter(s)` to generate a consistent hash.
    - **Parameter Entry Format**: For each configured parameter, the string used for hashing is `context_type` + `parameter_key` + `parameter_value`.
    - **Configurable Hashing**: Each flag definition specifies which parameter(s) from the evaluation context to use for hashing (e.g., `user_id`, `session_id`).
5.  **Persistence**:
    - Store flag and variant definitions in PostgreSQL.
    - Implement version-based optimistic concurrency for all mutable entities.
    - Automatic audit logging for all changes.

## Non-Functional Requirements
1.  **Type Safety**: Strict type checking at the application level to ensure variant values match the flag type.
2.  **Performance**: Evaluation should be highly efficient to minimize latency.
3.  **Auditability**: Every change to a flag or its variants must be captured in the central audit log.

## Acceptance Criteria
1.  API endpoints for CRUD operations on Flags and Variants are implemented.
2.  Evaluation logic correctly returns the expected variant based on rules and context.
3.  Percentage allocations are consistent for the same hash key across evaluations.
4.  Attempts to add variants with mismatched types are rejected.
5.  Optimistic locking prevents lost updates when two users edit the same flag simultaneously.

## Out of Scope
- SDK Implementation (to be handled in a separate track).
- Admin UI (separate project).
- Streaming/Push updates (future enhancement).
