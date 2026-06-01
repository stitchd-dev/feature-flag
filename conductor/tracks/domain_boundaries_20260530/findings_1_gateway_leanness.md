# Findings 1.1: Gateway Leanness Audit
*Date: 2026-05-30*

## Summary
- Total route files audited: 16
- TRANSLATION routes: 62
- ORCHESTRATION routes: 4
- DOMAIN-LOGIC-LEAK routes: 12 distinct handlers (spanning 14 specific code locations)

---

## Route-by-Route Classification

### flags.rs
| Handler | Classification | Notes |
|---------|---------------|-------|
| `list_flags` | TRANSLATION | Pure proxy |
| `create_flag` | DOMAIN-LOGIC-LEAK | Validates variant values against flag type; parses/normalises value_type string |
| `get_flag` | TRANSLATION | Pure proxy |
| `update_flag` | DOMAIN-LOGIC-LEAK | Read-modify-write to preserve `enabled` when omitted |
| `delete_flag` | TRANSLATION | Pure proxy |
| `archive_flag` | TRANSLATION | Pure proxy |
| `restore_flag` | TRANSLATION | Pure proxy |
| `update_variants` | DOMAIN-LOGIC-LEAK | Boolean flag invariant (exactly 2 variants, true/false values); variant type validation; read-modify-write to carry metadata |
| `update_rules` | DOMAIN-LOGIC-LEAK | `rule_body_to_proto` validates hash_inputs (FR-8 business rules); legacy hash_targets normalisation; read-modify-write to carry flag metadata |
| `update_flag_hashing` | TRANSLATION | Pure proxy |
| `set_default_rule_distribution` | DOMAIN-LOGIC-LEAK | `validate_hash_inputs`; parses gRPC error message prefix `"invalid_distribution:"` to rewrite 422 |
| `evaluate_preview` | DOMAIN-LOGIC-LEAK | UI-shape vs EC-shape bundle reconstruction; re-parses `results_json`; reshapes RolloutDebugJson/RuleTraceJson |

### experiments.rs
| Handler | Classification | Notes |
|---------|---------------|-------|
| `list_experiments` | TRANSLATION | Pure proxy |
| `create_experiment` | ORCHESTRATION + DOMAIN-LOGIC-LEAK | Calls flag-service + analytics-service to validate binding; enforces XOR invariant, rule-kind rule, context-type registry lookup |
| `get_experiment` | TRANSLATION | Pure proxy |
| `update_experiment` | ORCHESTRATION + DOMAIN-LOGIC-LEAK | Same binding validation fan-out as create |
| `delete_experiment` | TRANSLATION | Pure proxy |
| `transition_experiment` | TRANSLATION | Status string parse is acceptable |
| `list_iterations` | TRANSLATION | Maps pagination |
| `get_results` | DOMAIN-LOGIC-LEAK | Back-compat shim: synthesises per-context-type bundles from flat variant_results; defaults empty context_type to `"user"` |
| `list_exposures` | TRANSLATION | Required-param guard acceptable |

### events.rs
| Handler | Classification | Notes |
|---------|---------------|-------|
| `ingest_event` | TRANSLATION | Pure proxy |
| `ingest_batch` | TRANSLATION | Pure proxy |
| `list_event_definitions` | TRANSLATION | Pure proxy |
| `create_event_definition` | TRANSLATION | Pure proxy |
| `get_event_definition` | TRANSLATION | Pure proxy |
| `update_event_definition` | TRANSLATION | Pure proxy |
| `delete_event_definition` | TRANSLATION | Pure proxy |
| `track_events` | TRANSLATION | SDK context read + forward |
| `forward_to_analytics` | DOMAIN-LOGIC-LEAK | Stamps `properties["_test"] = "true"` — test-marking is analytics data policy |
| `track_events_admin` | DOMAIN-LOGIC-LEAK | env_id resolution from JWT vs body fallback; stamps `_test=true` |
| `get_event_firings` | TRANSLATION | Pure proxy |
| `get_event_stats` | TRANSLATION | Pure proxy |

### metrics.rs
| Handler | Classification | Notes |
|---------|---------------|-------|
| `create_metric` | TRANSLATION | Body→proto mapping |
| `list_metrics` | TRANSLATION | Pure proxy |
| `get_metric` | TRANSLATION | Pure proxy |
| `update_metric` | TRANSLATION | Body→proto mapping |
| `delete_metric` | TRANSLATION | Pure proxy |
| `preview_metric` | TRANSLATION | Pure proxy |
| `parse_aggregator` (helper) | DOMAIN-LOGIC-LEAK (borderline) | Enumerates all valid aggregator strings; unknown values return upstream error — adding a new aggregator in analytics-service silently breaks the gateway |

### segments.rs
| Handler | Classification | Notes |
|---------|---------------|-------|
| `list_segments` | TRANSLATION | Pure proxy |
| `create_segment` | DOMAIN-LOGIC-LEAK | `validate_segment_condition_expr` enforces forbidden-operator allow-list |
| `list_segments_in_env` | TRANSLATION | Pure proxy |
| `create_segment_in_env` | DOMAIN-LOGIC-LEAK | Same condition validation |
| `get_segment` | TRANSLATION | Pure proxy |
| `update_segment` | DOMAIN-LOGIC-LEAK | Same condition validation |
| `delete_segment` | TRANSLATION | Pure proxy |
| `patch_segment_entries` | TRANSLATION | Pure proxy |
| `lookup_segment_entry` | TRANSLATION | Pure proxy |

### management.rs
| Handler | Classification | Notes |
|---------|---------------|-------|
| `create_user` | DOMAIN-LOGIC-LEAK | Default `org_role = "org_member"` |
| All others | TRANSLATION | Pure proxy |

### admin.rs
| Handler | Classification | Notes |
|---------|---------------|-------|
| `seed_user` | DOMAIN-LOGIC-LEAK | Default `org_role = "org_admin"` |
| All others | TRANSLATION | Pure proxy |

### auth.rs, auth_providers.rs, oidc.rs, saml.rs
All handlers: **TRANSLATION** — pure proxy or trivial bearer-token extraction.

### stats.rs, context_intel.rs, eval_stats.rs, sdk_backend.rs
All handlers: **TRANSLATION** — pure proxy with gateway-appropriate metadata injection.

### event_admin.rs
| Handler | Classification | Notes |
|---------|---------------|-------|
| `list_events` | TRANSLATION | Pagination math acceptable |
| `create_event` | DOMAIN-LOGIC-LEAK | Defaults `name` to `event_key` |
| `get_event` | TRANSLATION | Pure proxy |
| `update_event` | ORCHESTRATION | Key→id resolution (GET then PATCH) — two calls because proto requires ID not key |
| `delete_event` | ORCHESTRATION | Same key→id resolution |

---

## Domain-Logic-Leak Details

### [flags.rs:157–181] `validate_hash_inputs` — FR-8 hash selector business rules
- **Leak type**: business-rule
- **Owning service**: flag-service
- **Evidence**:
  ```rust
  // flags.rs:157
  pub fn validate_hash_inputs(selectors: &[HashSelectorJson]) -> Result<(), String> {
      if selectors.is_empty() {
          return Err("hash_inputs must not be empty".to_string());
      }
      // duplicate detection, empty parameter guard...
  }
  ```
- **Proposed fix**: move to `flag-service` `MutateFlag`/`UpdateRules` gRPC handler; return `INVALID_ARGUMENT`; the gateway calls gRPC → maps status to 400.

### [flags.rs:368–412] `validate_variant_values` — variant value type enforcement + key uniqueness
- **Leak type**: business-rule/validation
- **Owning service**: flag-service
- **Evidence**:
  ```rust
  // flags.rs:368
  fn validate_variant_values(variants: &[VariantBody], value_type: FlagValueType) -> Option<String> {
      for v in variants {
          let ok = match value_type {
              FlagValueType::Bool => matches!(v.value, serde_json::Value::Bool(_)),
              FlagValueType::Int => v.value.as_i64().is_some(),
              // ...
          };
      }
  }
  ```
- **Proposed fix**: move to flag-service `MutateFlag`; return `INVALID_ARGUMENT` on mismatch.

### [flags.rs:861–879] `update_variants` — boolean flag structural invariant
- **Leak type**: business-rule
- **Owning service**: flag-service
- **Evidence**:
  ```rust
  // flags.rs:861
  if current.value_type == (FlagValueType::Bool as i32) {
      if body.variants.len() != 2 {
          return Err(GatewayError::BadRequest("Boolean flags must have exactly 2 variants"));
      }
      // true/false value check
  }
  ```
- **Proposed fix**: move to flag-service; return `INVALID_ARGUMENT`.

### [flags.rs:659–674] `update_flag` — read-modify-write to preserve `enabled`
- **Leak type**: business-rule (stateful defaults)
- **Owning service**: flag-service
- **Evidence**:
  ```rust
  // flags.rs:659
  let current_enabled = if body.enabled.is_none() {
      client.get_flag(get_req).await.ok()
          .map(|r| r.into_inner().enabled)
          .unwrap_or(true)
  } else { body.enabled.unwrap_or(true) };
  ```
- **Proposed fix**: add `FieldMask` or `partial_update: bool` to `MutateFlagRequest`; service preserves unset fields. Eliminates the pre-fetch.

### [flags.rs:847–918] + [flags.rs:1095–1145] — read-modify-write to carry flag metadata (update_variants / update_rules)
- **Leak type**: business-rule (stateful orchestration)
- **Owning service**: flag-service
- **Evidence**:
  ```rust
  // flags.rs:1109
  let current = client.get_flag(get_req).await.map_err(...)?.into_inner();
  let flag = FeatureFlag {
      key: flag_key, enabled: current.enabled, name: current.name,
      description: current.description, value_type: current.value_type,
      rules: proto_rules, ..Default::default()
  };
  ```
- **Proposed fix**: add `ReplaceVariants` and `ReplaceRules` mutation kinds to `MutateFlagRequest`.

### [flags.rs:1326–1336] `set_default_rule_distribution` — gRPC error message prefix parsing
- **Leak type**: business-rule (error classification)
- **Owning service**: flag-service
- **Evidence**:
  ```rust
  // flags.rs:1326
  if s.code() == tonic::Code::InvalidArgument
      && s.message().starts_with("invalid_distribution:")
  {
      return Err(GatewayError::InvalidDistribution(...));
  }
  ```
- **Proposed fix**: flag-service should use proto `ErrorDetails` or a distinct error code for distribution validation failures.

### [flags.rs:1418–1599] `evaluate_preview` — context bundle reconstruction + opaque results_json re-parsing
- **Leak type**: inline-eval / query-building
- **Owning service**: flag-service
- **Proposed fix**: (a) `EvaluatePreviewRequest` accepts flat UI-shape list directly; (b) `EvaluatePreviewResponse` carries structured `repeated ContextPreviewResult` proto rather than opaque JSON string.

### [experiments.rs:373–458] `validate_experiment_binding` — multi-service domain validation
- **Leak type**: business-rule
- **Owning service**: experimentation-service
- **Evidence**: Calls flag-service + analytics-service to validate binding; enforces XOR invariant, rule-kind rule, context-type registry lookup.
- **Proposed fix**: experimentation-service `CreateExperiment`/`UpdateExperiment` RPCs perform these validations internally.

### [experiments.rs:908–924] `get_results` — back-compat shim synthesising context-type bundles
- **Leak type**: inline-eval (legacy migration logic)
- **Owning service**: experimentation-service
- **Proposed fix**: experimentation-service always populates `results_by_context_type`, defaulting context_type to `"user"` for legacy rows.

### [events.rs:585–590] `forward_to_analytics` — `_test` property mutation
- **Leak type**: business-rule
- **Owning service**: analytics-service
- **Proposed fix**: add `mark_test: bool` to `TrackEventsRequest` proto; analytics-service stamps the property.

### [segments.rs:623–672] `SEGMENT_FORBIDDEN_OPS` + `validate_segment_condition_expr` — operator allow-list
- **Leak type**: business-rule
- **Owning service**: segmentation-service
- **Proposed fix**: segmentation-service validates `condition_expr` bytes and returns `INVALID_ARGUMENT` on forbidden operators.

### [management.rs:224] + [admin.rs:147] — default org_role
- **Leak type**: business-rule (defaulting)
- **Owning service**: management-service
- **Proposed fix**: management-service defaults `org_role` when absent.

### [event_admin.rs:219] `create_event` — name defaulting
- **Leak type**: business-rule (defaulting)
- **Owning service**: analytics-service
- **Proposed fix**: analytics-service defaults name to event_key in `CreateEventDefinition`.

### [metrics.rs:250–263] `parse_aggregator` / `parse_goal_direction` — domain enum string validation
- **Leak type**: inline-eval (protocol brittleness)
- **Owning service**: analytics-service
- **Proposed fix**: use proto enum fields for `aggregator` and `goal_direction`.

---

## Cross-Cutting Concerns (gateway-appropriate)

The following are correctly placed in the gateway and MUST NOT be moved:

1. **JWT validation middleware** (`middleware::auth`) — validates Bearer tokens via auth-service, injects `RbacContext`.
2. **SDK key validation middleware** (`middleware::sdk_auth`) — validates `x-sdk-key`, injects `SdkContext`.
3. **Permission guard** (`require_permission`) — O(n) check over injected RBAC claims; no service call.
4. **Rate limiting / quota enforcement** — `DefaultBodyLimit::max`, per-env event quota cap.
5. **Request tracing, structured logging, Prometheus metrics** — infrastructure cross-cutting.
6. **`x-env-id` gRPC metadata injection** — propagating trusted env to downstream via metadata is a routing concern.
7. **HTTP status code translation** (`GatewayError::from(tonic::Status)`, `status_to_gw_err`) — proto code → HTTP code mapping.
8. **Pagination parameter normalisation** (`PaginationParams`) — enforcing min/max page bounds.
9. **Protocol-shape adaptation** (proto ↔ JSON DTOs) — transforming between wire representations is core translation; NOT leaks even when verbose.
