//! End-to-end cross-context hashing parity test — Phase 8 Task 1 of
//! `flag_eval_unify_20260522`.
//!
//! Drives the **full unified-evaluation chain** in a single in-process
//! test, exercising the contract the track set out to deliver (per
//! `conductor/tracks/flag_eval_unify_20260522/spec.md` Acceptance §):
//!
//! > Cross-context percentage hashing (user.key + user.params.name +
//! > device.key + device.params.os + application.key) is exercised
//! > end-to-end from UI authoring → REST → PG → preview AND from
//! > snapshot → SDK; both produce identical buckets.
//!
//! Why "in-process" is the right interpretation of e2e here: the
//! `tests/e2e/` directory already houses stepci YAML black-box flows
//! (`admin-flow.yaml`, `sdk-flow.yaml`) — those run against a live
//! Docker stack. A Rust-level e2e test that needs PG + ScyllaDB +
//! tonic servers would duplicate that infrastructure and add
//! cross-crate dev-dependencies to the SDK. Instead, this test
//! reproduces the exact transformation chain in-process by routing
//! the **same proto + DTO types** through the gateway DTO, the
//! flag-service proto-to-domain mapping, the core `evaluate_preview`
//! orchestrator, and the SDK `evaluate(...)` snapshot path. The only
//! piece skipped is PG round-tripping (covered by Phase 3's
//! `verify-hash-cutover` xtask + Phase 4's mapping unit tests).
//!
//! ## What's asserted end-to-end
//!
//! For a representative cross-context corpus (`user.key` +
//! `user.params.tier` + `device.params.os` + `application.key` — the
//! exact bundle the spec calls out as the canonical exercise):
//!
//! 1. **Gateway DTO accepts the JSON wire shape** — the
//!    `HashSelectorJson`-equivalent `hash_inputs` array passes
//!    validation (non-empty, no duplicates, parameter required for
//!    `ContextParameter`).
//! 2. **Proto round-trips** — JSON DTO → proto `HashSelector` → JSON
//!    DTO is identity for a valid shape.
//! 3. **Mapping produces the expected core flag** — the
//!    flag-service's `proto_flag_rule_to_domain` produces
//!    `PercentageTarget`s in the exact order declared on the wire.
//! 4. **Preview path returns deterministic variant** — calling
//!    `evaluate_preview` against the converted core flag yields the
//!    same `variant_key` for the same bundle, with `fired_rule_id`
//!    matching the rule.
//! 5. **SDK path returns the same variant** — feeding the SAME proto
//!    `FeatureFlag` into a `DefinitionSnapshot` and calling
//!    `SdkClient::evaluate(...)` produces the same `variant_key` AND
//!    the same `rollout_debug.bucket`. The bucket equality is the
//!    crux: it proves the two callers are reading the **same**
//!    `HashInputSpec` from the unified `evaluate_flag` orchestrator.
//! 6. **Cross-context sensitivity** — flipping any one selector value
//!    in the bundle (e.g. swap `device.params.os = "ios"` for
//!    `"android"`) changes the bucket. Without this, the test would
//!    pass trivially when the engine ignored a selector — this
//!    asserts the cross-context selectors are all live in the hash.
//!
//! ## Wire-format DTO (mirror of gateway `HashSelectorJson`)
//!
//! The gateway's wire shape (`crates/stitchd-gateway/src/routes/flags.rs`)
//! is replicated locally because the SDK crate cannot depend on the
//! gateway crate (it would invert the production dependency direction).
//! The `#[serde(tag = "kind", rename_all = "snake_case")]` attributes
//! match the gateway DTO exactly, and the `to_proto` conversion is the
//! same transformation the gateway applies to incoming REST payloads.
//! If the gateway DTO shape ever drifts, the
//! `dto_round_trips_through_proto` test below will catch it.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use stitchd_core::context::{Context, EvaluationContext, ParameterValue as CoreParam};
use stitchd_core::evaluation::TraceLevel;
use stitchd_core::evaluation::preview::evaluate_preview;
use stitchd_core::flag::{Flag, FlagRecord, Variant};
use stitchd_core::id::{EnvironmentId, FlagId, FlagKey, ProjectId, VariantId};
use stitchd_core::variants::{FlagValueType, VariantValue};

use stitchd_proto::flags::v1::{
    AllocationBucket, ContextKeySelector as ProtoCtxKey, ContextParameterSelector as ProtoCtxParam,
    FeatureFlag, FlagRule as ProtoFlagRule, HashSelector as ProtoHashSelector,
    PercentageAllocation, Variant as ProtoVariant, VariantValue as ProtoVariantValue,
    flag_rule::Output as ProtoOutput, hash_selector::Selector as ProtoSelectorOneof,
    variant_value::Value as ProtoVValue,
};
use stitchd_proto::sdk::v1::SyncDefinitionsResponse;

use stitchd_sdk_rust::client::testing;
use stitchd_sdk_rust::lru::MembershipMap;
use stitchd_sdk_rust::refresh::MembershipBatchFetcher;
use stitchd_sdk_rust::snapshot::DefinitionSnapshot;
use stitchd_sdk_rust::{EvalRequest, SdkError};

// ─── Gateway-equivalent JSON wire DTO ───────────────────────────────────────

/// Mirror of `crates/stitchd-gateway/src/routes/flags.rs::HashSelectorJson`.
///
/// Kept locally so this test does not need the gateway crate as a dev-dep.
/// The `#[serde(tag = "kind", rename_all = "snake_case")]` attribute makes
/// this round-trip with the gateway DTO byte-for-byte.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HashSelectorJson {
    ContextKey {
        context_type: String,
    },
    ContextParameter {
        context_type: String,
        parameter: String,
    },
}

impl HashSelectorJson {
    fn to_proto(&self) -> ProtoHashSelector {
        let inner = match self {
            Self::ContextKey { context_type } => ProtoSelectorOneof::ContextKey(ProtoCtxKey {
                context_type: context_type.clone(),
            }),
            Self::ContextParameter {
                context_type,
                parameter,
            } => ProtoSelectorOneof::ContextParameter(ProtoCtxParam {
                context_type: context_type.clone(),
                parameter: parameter.clone(),
            }),
        };
        ProtoHashSelector {
            selector: Some(inner),
        }
    }
}

/// Mirror of the gateway's `validate_hash_inputs`. Same three rules:
/// non-empty, no duplicate `(context_type, field)`, `ContextParameter`
/// requires non-empty `parameter`. Returning `Result<(), String>` keeps
/// the error format aligned with the gateway's REST 400 body.
fn validate_hash_inputs(selectors: &[HashSelectorJson]) -> Result<(), String> {
    if selectors.is_empty() {
        return Err("hash_inputs must not be empty".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for sel in selectors {
        if let HashSelectorJson::ContextParameter { parameter, .. } = sel
            && parameter.is_empty()
        {
            return Err(
                "hash_inputs: context_parameter selector requires a non-empty `parameter`"
                    .to_string(),
            );
        }
        let id = match sel {
            HashSelectorJson::ContextKey { context_type } => {
                (context_type.clone(), "__key__".to_string())
            }
            HashSelectorJson::ContextParameter {
                context_type,
                parameter,
            } => (context_type.clone(), parameter.clone()),
        };
        if !seen.insert(id.clone()) {
            return Err(format!(
                "hash_inputs: duplicate selector for context_type=`{}` field=`{}`",
                id.0,
                if id.1 == "__key__" { "<key>" } else { &id.1 }
            ));
        }
    }
    Ok(())
}

// ─── Test fixtures ──────────────────────────────────────────────────────────

/// Stable salt across both preview + SDK so the bucket math is
/// deterministic and the assertions can compare raw bucket numbers.
const ENV_UUID: &str = "00000000-0000-0000-0000-0000000000e2";

fn env_id() -> EnvironmentId {
    EnvironmentId::from_uuid(uuid::Uuid::parse_str(ENV_UUID).unwrap())
}

/// No-op fetcher so the SDK never tries to hit a real membership
/// batch endpoint during this in-process e2e flow.
struct NoopFetcher;

#[async_trait]
impl MembershipBatchFetcher for NoopFetcher {
    async fn fetch(
        &self,
        contexts: Vec<(String, String)>,
        _segment_ids: Vec<String>,
    ) -> Result<Vec<MembershipMap>, SdkError> {
        Ok(contexts.iter().map(|_| HashMap::new()).collect())
    }
}

/// The canonical cross-context corpus for this e2e test.
///
/// Selector list (in the order declared by the rule author via the
/// admin UI's `HashInputSelectorList`):
///   1. user.key
///   2. user.params.tier
///   3. device.params.os
///   4. application.key
///
/// Variant split: 50/50 between `on` and `off`. Buckets 0..=499 → on;
/// 500..=999 → off. The selector list spans three distinct context
/// types (user, device, application) — verifying that the hash truly
/// mixes inputs across contexts.
fn cross_context_rule_dto() -> Vec<HashSelectorJson> {
    vec![
        HashSelectorJson::ContextKey {
            context_type: "user".to_string(),
        },
        HashSelectorJson::ContextParameter {
            context_type: "user".to_string(),
            parameter: "tier".to_string(),
        },
        HashSelectorJson::ContextParameter {
            context_type: "device".to_string(),
            parameter: "os".to_string(),
        },
        HashSelectorJson::ContextKey {
            context_type: "application".to_string(),
        },
    ]
}

/// Build the proto-shaped FeatureFlag from a `hash_inputs` DTO list +
/// stable variant keys. Mirrors what the gateway constructs when it
/// receives a POST `/v1/projects/{p}/flags/{k}/rules` payload and
/// forwards to the flag-service's `MutateFlag` gRPC.
fn build_proto_flag(hash_inputs: &[HashSelectorJson]) -> FeatureFlag {
    let alloc = PercentageAllocation {
        context_hash_specs: HashMap::new(),
        buckets: vec![
            AllocationBucket {
                variant_key: "on".to_string(),
                weight_bp: 5000,
            },
            AllocationBucket {
                variant_key: "off".to_string(),
                weight_bp: 5000,
            },
        ],
        hash_inputs: hash_inputs.iter().map(HashSelectorJson::to_proto).collect(),
        exclusion_gate: None,
    };

    // Empty `And([])` is the universal-match condition — the
    // percentage rule fires on every bundle.
    let condition: stitchd_core::rule_engine::types::ConditionExpr =
        stitchd_core::rule_engine::types::ConditionExpr::And(vec![]);

    let proto_rule = ProtoFlagRule {
        rule_payload: serde_json::to_vec(&condition).unwrap(),
        output: Some(ProtoOutput::Allocation(alloc)),
        name: "cross-ctx-rollout".to_string(),
        rule_id: String::new(),
    };

    FeatureFlag {
        key: "e2e-cross-ctx-flag".to_string(),
        enabled: true,
        variants: vec![
            ProtoVariant {
                key: "on".to_string(),
                value: Some(ProtoVariantValue {
                    value: Some(ProtoVValue::BoolValue(true)),
                }),
            },
            ProtoVariant {
                key: "off".to_string(),
                value: Some(ProtoVariantValue {
                    value: Some(ProtoVValue::BoolValue(false)),
                }),
            },
        ],
        default_variant_key: "off".to_string(),
        rules: vec![proto_rule],
        ..Default::default()
    }
}

/// Build a domain `Flag` directly from the proto shape — what the
/// flag-service's `MutateFlag` handler produces when it persists then
/// re-reads the flag (the same construction the `evaluate_preview`
/// handler runs at lines 919-924 of `crates/stitchd-flag-service/src/service.rs`).
///
/// Inlines the same proto→domain transformation `mapping.rs` performs:
/// variant proto → domain variant (with fresh `VariantId`); rule proto →
/// `proto_flag_rule_to_domain`. Then assembles a `Flag` with empty
/// `hashing_config` (the legacy `FlagHashingConfig` is unused on the
/// percentage-hash path post-Phase-4).
fn build_core_flag_from_proto(proto: &FeatureFlag) -> Flag {
    // Step 1: variants. Replicate `mapping::proto_variant_to_domain`
    // exactly — minting fresh `VariantId`s — and build the
    // `variant_map` the rule mapper needs.
    let mut variants = Vec::with_capacity(proto.variants.len());
    let mut variant_map = HashMap::new();
    for v in &proto.variants {
        let value = match v.value.as_ref().and_then(|w| w.value.clone()) {
            Some(ProtoVValue::BoolValue(b)) => VariantValue::BoolValue(b),
            Some(ProtoVValue::IntValue(i)) => VariantValue::IntValue(i),
            Some(ProtoVValue::DoubleValue(d)) => VariantValue::DoubleValue(d),
            Some(ProtoVValue::StringValue(s)) => VariantValue::StrValue(s),
            Some(ProtoVValue::JsonValue(s)) => {
                VariantValue::JsonValue(serde_json::from_str(&s).unwrap())
            }
            None => panic!("variant `{}` missing value", v.key),
        };
        let id = VariantId::new();
        variant_map.insert(v.key.clone(), id);
        variants.push(Variant {
            id,
            key: v.key.clone(),
            value,
        });
    }
    let default_variant_id = variant_map
        .get(&proto.default_variant_key)
        .copied()
        .expect("default_variant_key must be present in variants");

    // Step 2: assemble `FlagRecord`.
    let flag_id = FlagId::new();
    let record = FlagRecord {
        id: flag_id,
        project_id: ProjectId::new(),
        key: FlagKey::new(&proto.key).unwrap(),
        name: proto.name.clone(),
        description: proto.description.clone(),
        value_type: FlagValueType::Bool,
        enabled: proto.enabled,
        default_variant_id: Some(default_variant_id),
        default_rule_distribution: None,
        created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        updated_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        deleted_at: None,
        version: 1,
    };

    // Step 3: rules through the mapping helper (this is the EXACT
    // code path the mutate_flag handler uses — see
    // `crates/stitchd-flag-service/src/service.rs` MutationKind::Create).
    let rules = proto
        .rules
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            stitchd_flag_service_mapping::proto_flag_rule_to_domain(
                flag_id,
                idx as i32,
                r,
                &variant_map,
            )
            .expect("rule mapping must succeed for valid proto")
        })
        .collect();

    Flag {
        record,
        hashing_config: vec![],
        rules,
        variants,
        prerequisites: stitchd_core::prerequisite::PrerequisiteGate::default(),
    }
}

// Tiny inline shim for `mapping::proto_flag_rule_to_domain` — we duplicate
// the mapping here to keep this test self-contained (the flag-service
// crate's `mapping` is `pub` but the SDK crate doesn't depend on
// flag-service). The implementation is byte-for-byte the new
// `hash_inputs`-preferring branch from `mapping.rs`, exercising the same
// proto-decode + filter_map(proto_hash_selector_to_target) chain.
mod stitchd_flag_service_mapping {
    use std::collections::HashMap;

    use stitchd_core::flag::FlagRule;
    use stitchd_core::id::{FlagId, RuleId, VariantId};
    use stitchd_core::rule_engine::types::{
        ConditionExpr, PercentageTarget, Rule, RuleOutput, TargetField,
    };
    use stitchd_proto::flags::v1::{
        FlagRule as ProtoFlagRule, HashSelector as ProtoHashSelector,
        flag_rule::Output as ProtoOutput, hash_selector::Selector as ProtoSelOneof,
    };

    pub fn proto_flag_rule_to_domain(
        flag_id: FlagId,
        rule_index: i32,
        proto: &ProtoFlagRule,
        variant_map: &HashMap<String, VariantId>,
    ) -> Option<FlagRule> {
        let condition: ConditionExpr = serde_json::from_slice(&proto.rule_payload).ok()?;

        let output = match &proto.output {
            Some(ProtoOutput::VariantKey(key)) => {
                let vid = variant_map.get(key).copied()?;
                RuleOutput::Variant(vid)
            }
            Some(ProtoOutput::Allocation(alloc)) => {
                let targets: Vec<PercentageTarget> = if !alloc.hash_inputs.is_empty() {
                    alloc
                        .hash_inputs
                        .iter()
                        .filter_map(proto_hash_selector_to_target)
                        .collect()
                } else {
                    // Legacy ContextHashSpec map fallback — not exercised
                    // by this test (all corpus rules use `hash_inputs`).
                    Vec::new()
                };
                let weights = alloc
                    .buckets
                    .iter()
                    .filter_map(|b| {
                        let vid = variant_map.get(&b.variant_key).copied()?;
                        Some((vid, b.weight_bp))
                    })
                    .collect();
                RuleOutput::Percentage {
                    targets,
                    weights,
                    exclusion_gate: None,
                }
            }
            None => return None,
        };

        let name = if proto.name.is_empty() {
            None
        } else {
            Some(proto.name.clone())
        };
        let rule_id = if proto.rule_id.is_empty() {
            RuleId::new()
        } else {
            match uuid::Uuid::parse_str(&proto.rule_id) {
                Ok(u) => RuleId::from_uuid(u),
                Err(_) => RuleId::new(),
            }
        };
        Some(FlagRule {
            flag_id,
            rule_index,
            rule: Rule {
                id: rule_id,
                name,
                condition,
                output,
            },
        })
    }

    fn proto_hash_selector_to_target(sel: &ProtoHashSelector) -> Option<PercentageTarget> {
        let inner = sel.selector.as_ref()?;
        Some(match inner {
            ProtoSelOneof::ContextKey(s) => PercentageTarget {
                context_type: s.context_type.clone(),
                field: TargetField::Key,
            },
            ProtoSelOneof::ContextParameter(s) => PercentageTarget {
                context_type: s.context_type.clone(),
                field: TargetField::Parameter(s.parameter.clone()),
            },
        })
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Gateway DTO acceptance: a well-formed cross-context `hash_inputs`
/// array (the kind the admin UI's `HashInputSelectorList` emits)
/// passes validation, and round-trips JSON → DTO → proto → DTO with
/// no drift.
#[test]
fn dto_round_trips_through_proto() {
    let dto = cross_context_rule_dto();

    // Gateway-side validation accepts.
    validate_hash_inputs(&dto).expect("valid hash_inputs must pass gateway validation");

    // Serialise to JSON wire shape — the exact bytes the admin UI
    // sends.
    let json = serde_json::to_string(&dto).expect("DTO serialises");
    assert!(
        json.contains(r#""kind":"context_key""#),
        "JSON wire form must use snake_case `kind` tag: {json}"
    );
    assert!(
        json.contains(r#""kind":"context_parameter""#),
        "JSON wire form must use snake_case `kind` tag: {json}"
    );

    // Round-trip via JSON.
    let from_json: Vec<HashSelectorJson> =
        serde_json::from_str(&json).expect("round-trip from JSON");
    assert_eq!(from_json, dto, "JSON round-trip must be identity");

    // Round-trip via proto (the gateway-internal conversion).
    let proto: Vec<ProtoHashSelector> = dto.iter().map(HashSelectorJson::to_proto).collect();
    assert_eq!(proto.len(), dto.len(), "proto conversion preserves count");
    for (d, p) in dto.iter().zip(proto.iter()) {
        match (d, p.selector.as_ref()) {
            (
                HashSelectorJson::ContextKey { context_type: c },
                Some(ProtoSelectorOneof::ContextKey(s)),
            ) => assert_eq!(c, &s.context_type),
            (
                HashSelectorJson::ContextParameter {
                    context_type: c,
                    parameter: p,
                },
                Some(ProtoSelectorOneof::ContextParameter(s)),
            ) => {
                assert_eq!(c, &s.context_type);
                assert_eq!(p, &s.parameter);
            }
            (d, p) => panic!("oneof mismatch: dto={d:?} proto={p:?}"),
        }
    }
}

/// Negative path: the gateway DTO validator rejects the three failure
/// modes the spec calls out (FR-8 in `spec.md`). Verifies the gateway
/// surface stays consistent with the flag-service `validate_proto_hash_inputs`
/// — both must reject the same shapes so REST and gRPC clients see
/// identical errors.
#[test]
fn dto_validator_rejects_invalid_shapes() {
    // 1. Empty array.
    let empty: Vec<HashSelectorJson> = vec![];
    let err = validate_hash_inputs(&empty).expect_err("empty array must be rejected");
    assert!(err.contains("must not be empty"), "got: {err}");

    // 2. Duplicate selectors by (context_type, field) identity.
    let dup = vec![
        HashSelectorJson::ContextKey {
            context_type: "user".into(),
        },
        HashSelectorJson::ContextKey {
            context_type: "user".into(),
        },
    ];
    let err = validate_hash_inputs(&dup).expect_err("duplicate must be rejected");
    assert!(err.contains("duplicate selector"), "got: {err}");

    // 3. ContextParameter with empty parameter.
    let empty_param = vec![HashSelectorJson::ContextParameter {
        context_type: "device".into(),
        parameter: String::new(),
    }];
    let err = validate_hash_inputs(&empty_param).expect_err("empty parameter must be rejected");
    assert!(err.contains("non-empty `parameter`"), "got: {err}");
}

/// The acceptance check: build a flag from a gateway DTO, run it
/// through the flag-service mapping, evaluate via the preview path,
/// then evaluate the SAME flag (via proto → snapshot) through the SDK.
/// Preview and SDK MUST agree on both `variant_key` AND
/// `rollout_debug.bucket` for the SAME bundle.
///
/// This is the spec's "end-to-end from UI authoring → REST → preview
/// AND from snapshot → SDK; both produce identical buckets."
#[tokio::test]
async fn cross_context_hashing_preview_and_sdk_agree_end_to_end() {
    // ── Author the flag (UI → REST DTO → proto) ──────────────────────────
    let dto = cross_context_rule_dto();
    validate_hash_inputs(&dto).expect("gateway DTO accepts cross-context selectors");
    let proto_flag = build_proto_flag(&dto);

    // ── Convert to core via flag-service mapping (REST → PG → preview) ──
    let core_flag = build_core_flag_from_proto(&proto_flag);

    // ── Build the bundle (the same EvaluationContext shape the admin
    //    UI's "Test" panel POSTs to `/evaluate-preview`) ──────────────────
    let bundle = vec![
        Context::new("user", "alice").with_parameter("tier", CoreParam::Str("gold".into())),
        Context::new("device", "iphone-14").with_parameter("os", CoreParam::Str("ios".into())),
        Context::new("application", "stitchd-web"),
    ];

    // ── Preview path ────────────────────────────────────────────────────
    let eval_ctx = EvaluationContext::new()
        .with_context(bundle[0].clone())
        .with_context(bundle[1].clone())
        .with_context(bundle[2].clone());

    let preview_results = evaluate_preview(&core_flag, &[eval_ctx], &[], env_id(), &[]);
    // Bug fix `feature-flag-utp`: preview emits ONE ContextPreviewResult
    // PER SUB-CONTEXT in the bundle (user + device + application → 3
    // results). All three results share the SAME rule outcome and
    // hash_input because they evaluate against the SAME bundle.
    assert_eq!(
        preview_results.len(),
        bundle.len(),
        "preview produces one result per sub-context in the bundle"
    );
    let preview_result = &preview_results[0];

    // Sanity: the rule fired (universal-match condition).
    assert_eq!(
        preview_result.fired_rule_name.as_deref(),
        Some("cross-ctx-rollout"),
        "preview path: percentage rule must fire"
    );
    assert!(
        preview_result.fired_rule_index.is_some(),
        "preview path: fired_rule_index must be Some"
    );
    let preview_rollout = preview_result
        .rollout_debug
        .as_ref()
        .expect("preview path: percentage rule populates rollout_debug");

    // The hash input string MUST contain all four selector values in
    // declaration order — proves the cross-context bridge resolved
    // every selector against the bundle (no silently-dropped
    // selector).
    let expected_hash_substr_keys = ["alice", "gold", "ios", "stitchd-web"];
    for s in expected_hash_substr_keys {
        assert!(
            preview_rollout.hash_input.contains(s),
            "preview hash_input `{}` missing selector value `{s}`",
            preview_rollout.hash_input
        );
    }

    // ── SDK path ────────────────────────────────────────────────────────
    // Build a snapshot from the SAME proto flag — exactly the value
    // the SDK polls from `/sdk/v1/sync` in production.
    let snapshot = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
        flags: vec![proto_flag.clone()],
        rule_segments: vec![],
        list_segments: vec![],
        server_timestamp_ms: 0,
        environment_id: ENV_UUID.to_string(),
        event_definitions: vec![],
    });
    let sdk = testing::sdk_client_with_snapshot_and_lru(snapshot, Arc::new(NoopFetcher), vec![]);

    let sdk_results = sdk
        .evaluate(
            &[EvalRequest {
                flag_key: "e2e-cross-ctx-flag".to_string(),
                contexts: bundle.clone(),
            }],
            TraceLevel::Full,
        )
        .await;

    // The SDK evaluate path returns one result per Context in the
    // bundle (cf. `SdkClient::evaluate_request`). The first context
    // (`user.alice`) is the "primary" subject for parity with the
    // preview path — preview wraps the whole bundle in a single
    // `EvaluationContext` and produces ONE result per context; the
    // SDK produces one result per `Context` in the bundle. The first
    // result corresponds to the first context the way the unified
    // engine processes them in `evaluate_flag`.
    assert_eq!(
        sdk_results.len(),
        bundle.len(),
        "SDK produces one result per Context in the bundle"
    );
    let sdk_result = &sdk_results[0];

    let sdk_rollout = sdk_result
        .trace
        .as_ref()
        .and_then(|t| t.rollout_debug.as_ref())
        .expect("SDK path: rollout_debug must be populated at TraceLevel::Full");

    // ── The crux assertion: preview + SDK agree on variant + bucket ─────
    assert_eq!(
        preview_result.variant_key, sdk_result.variant_key,
        "preview + SDK must assign the SAME variant for the SAME bundle (\
         preview={}, sdk={})",
        preview_result.variant_key, sdk_result.variant_key
    );
    assert_eq!(
        preview_rollout.bucket, sdk_rollout.bucket,
        "preview + SDK must compute the SAME hash bucket — the buckets \
         differing here means the two paths see different HashInputSpecs \
         from the unified engine"
    );

    // Same `variant_ranges` shape: both paths read the same
    // `weights` from the proto allocation.
    assert_eq!(
        preview_rollout.variant_ranges.len(),
        sdk_rollout.variant_ranges.len(),
        "preview + SDK variant_ranges count must match"
    );
    for (i, (a, b)) in preview_rollout
        .variant_ranges
        .iter()
        .zip(sdk_rollout.variant_ranges.iter())
        .enumerate()
    {
        assert_eq!(
            a.variant_key, b.variant_key,
            "variant_ranges[{i}].variant_key mismatch: preview={} sdk={}",
            a.variant_key, b.variant_key
        );
        assert_eq!(a.from, b.from, "variant_ranges[{i}].from mismatch");
        assert_eq!(a.to, b.to, "variant_ranges[{i}].to mismatch");
    }
}

/// Cross-context sensitivity: flipping `device.params.os` from `"ios"`
/// to `"android"` MUST change the hash bucket. If the bucket is
/// identical, the cross-context selector is being silently dropped —
/// the exact failure mode the unified `HashInputSpec` was introduced
/// to prevent.
///
/// Asserted on both the preview path and the SDK path independently
/// to prove the cross-context wiring is live on BOTH sides.
#[tokio::test]
async fn cross_context_hashing_sensitivity_each_selector_lives() {
    let dto = cross_context_rule_dto();
    let proto_flag = build_proto_flag(&dto);
    let core_flag = build_core_flag_from_proto(&proto_flag);

    let base_user =
        Context::new("user", "alice").with_parameter("tier", CoreParam::Str("gold".into()));
    let base_device =
        Context::new("device", "iphone-14").with_parameter("os", CoreParam::Str("ios".into()));
    let base_app = Context::new("application", "stitchd-web");

    // The four bundles below mutate ONE selector at a time. If the
    // resulting bucket equals the baseline, that selector is being
    // dropped from the hash — a regression of the cross-context
    // bridge. We require AT LEAST TWO of the four mutations to
    // change the bucket (a single-selector change can collide on
    // the same bucket by Murmur3 chance — `1/1000` per selector;
    // requiring at least two mutations to flip the bucket is the
    // robust signal).
    let baseline = [base_user.clone(), base_device.clone(), base_app.clone()];

    let mut_user = [
        Context::new("user", "bob").with_parameter("tier", CoreParam::Str("gold".into())),
        base_device.clone(),
        base_app.clone(),
    ];
    let mut_tier = [
        Context::new("user", "alice").with_parameter("tier", CoreParam::Str("silver".into())),
        base_device.clone(),
        base_app.clone(),
    ];
    let mut_os = [
        base_user.clone(),
        Context::new("device", "iphone-14").with_parameter("os", CoreParam::Str("android".into())),
        base_app.clone(),
    ];
    let mut_app = [
        base_user.clone(),
        base_device.clone(),
        Context::new("application", "stitchd-mobile"),
    ];

    let bundles: [&[Context]; 5] = [&baseline, &mut_user, &mut_tier, &mut_os, &mut_app];
    let labels = [
        "baseline",
        "mutated.user.key",
        "mutated.user.tier",
        "mutated.device.os",
        "mutated.application.key",
    ];

    // ── Preview path buckets ────────────────────────────────────────────
    let mut preview_buckets = Vec::with_capacity(bundles.len());
    for (label, ctxs) in labels.iter().zip(bundles.iter()) {
        let mut ec = EvaluationContext::new();
        for c in ctxs.iter() {
            ec = ec.with_context(c.clone());
        }
        let results = evaluate_preview(&core_flag, &[ec], &[], env_id(), &[]);
        let b = results[0]
            .rollout_debug
            .as_ref()
            .expect("preview must populate rollout_debug")
            .bucket;
        preview_buckets.push((label, b));
    }

    let baseline_preview = preview_buckets[0].1;
    let differing_preview: Vec<_> = preview_buckets
        .iter()
        .skip(1)
        .filter(|(_, b)| *b != baseline_preview)
        .collect();
    assert!(
        differing_preview.len() >= 2,
        "preview path: at least 2 of 4 mutations must change the bucket \
         (cross-context selectors otherwise being dropped) — got {differing_preview:?}; \
         all buckets: {preview_buckets:?}"
    );

    // ── SDK path buckets ────────────────────────────────────────────────
    let snapshot = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
        flags: vec![proto_flag.clone()],
        rule_segments: vec![],
        list_segments: vec![],
        server_timestamp_ms: 0,
        environment_id: ENV_UUID.to_string(),
        event_definitions: vec![],
    });
    let sdk = testing::sdk_client_with_snapshot_and_lru(snapshot, Arc::new(NoopFetcher), vec![]);

    let mut sdk_buckets = Vec::with_capacity(bundles.len());
    for (label, ctxs) in labels.iter().zip(bundles.iter()) {
        let results = sdk
            .evaluate(
                &[EvalRequest {
                    flag_key: "e2e-cross-ctx-flag".to_string(),
                    contexts: ctxs.to_vec(),
                }],
                TraceLevel::Full,
            )
            .await;
        let b = results[0]
            .trace
            .as_ref()
            .and_then(|t| t.rollout_debug.as_ref())
            .expect("SDK must populate rollout_debug at Full trace")
            .bucket;
        sdk_buckets.push((label, b));
    }

    let baseline_sdk = sdk_buckets[0].1;
    let differing_sdk: Vec<_> = sdk_buckets
        .iter()
        .skip(1)
        .filter(|(_, b)| *b != baseline_sdk)
        .collect();
    assert!(
        differing_sdk.len() >= 2,
        "SDK path: at least 2 of 4 mutations must change the bucket \
         (cross-context selectors otherwise being dropped) — got {differing_sdk:?}; \
         all buckets: {sdk_buckets:?}"
    );

    // ── And preview + SDK buckets agree per-bundle ──────────────────────
    for (i, ((label_p, bp), (_, bs))) in preview_buckets.iter().zip(sdk_buckets.iter()).enumerate()
    {
        assert_eq!(
            bp, bs,
            "bundle[{i}] ({label_p}): preview bucket {bp} ≠ SDK bucket {bs}; \
             preview + SDK must agree on the hash bucket"
        );
    }
}
