# Implementation Plan: Feature Flag Evaluation & Segmentation Core

## Phase 1: Core Evaluation Types & Context Handling
Focus: Define the fundamental types for rules, conditions, and evaluation contexts.

- [ ] Task: Define `Context` and `ContextValue` types with support for numeric, string, boolean, and semver types.
- [ ] Task: Implement `SipHash` based deterministic allocation for percentage-based variants.
- [ ] Task: Implement `ConditionOperator` and basic comparison logic.
- [ ] Task: Conductor - User Manual Verification 'Core Evaluation Types & Context Handling' (Protocol in workflow.md)

## Phase 2: Rule Engine Implementation
Focus: Implement the logic for evaluating conditions and combining them into rules.

- [ ] Task: Implement `Condition` evaluation logic against a `Context`.
- [ ] Task: Implement `Rule` logic (ordered list of conditions with AND combinator).
- [ ] Task: Implement `RuleSet` (ordered list of rules with first-true-exit behavior).
- [ ] Task: Conductor - User Manual Verification 'Rule Engine Implementation' (Protocol in workflow.md)

## Phase 3: Segmentation Logic
Focus: Implement rule-based and list-based segment evaluation.

- [ ] Task: Implement rule-based segment evaluation (link segment to a `RuleSet`).
- [ ] Task: Implement list-based segment lookup logic.
- [ ] Task: Implement "Is in Segment" condition for the core rule engine.
- [ ] Task: Conductor - User Manual Verification 'Segmentation Logic' (Protocol in workflow.md)

## Phase 4: Database Persistence (PostgreSQL)
Focus: Implement storage for segments and rules using SQLx.

- [ ] Task: Implement PostgreSQL repository for Segment configurations.
- [ ] Task: Implement PostgreSQL repository for Segment List entries (with partitioning support via `pg_partman`).
- [ ] Task: Integrate repositories with the core evaluation engine.
- [ ] Task: Conductor - User Manual Verification 'Database Persistence (PostgreSQL)' (Protocol in workflow.md)
