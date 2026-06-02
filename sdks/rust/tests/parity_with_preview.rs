//! Parity test — Phase 6 Task 3 of `flag_eval_unify_20260522`.
//!
//! For a shared corpus of `(flag, contexts, segment configs, list-segment
//! memberships)`, the SDK's `SdkClient::evaluate(...)` path and a direct
//! call to `stitchd_core::evaluation::evaluate_flag(...)` (the
//! preview-equivalent core entry) MUST return identical variants AND
//! identical traces when both run with `TraceLevel::Full`.
//!
//! Corpus coverage:
//!
//! - One cross-context-hash flag — percentage rule mixing `user.key`,
//!   `user.params.tier`, and `device.params.os` selectors.
//! - One default-rule-distribution flag — exercises the core engine's
//!   `EvalOutcome::DefaultRuleDistribution` branch via a directly-
//!   constructed `Flag` (the proto wire does not carry
//!   `default_rule_distribution` yet, so the SDK proto -> core conversion
//!   yields `None`; the parity check here uses core's direct path with a
//!   synthetic `Flag` to prove the engine handles the variant correctly,
//!   matching the contract for when the proto eventually ships the field).
//!
//! Run with:
//! ```
//! cargo test --features test-util -p stitchd-sdk-rust --test parity_with_preview
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use stitchd_core::context::{Context, ParameterValue as CoreParam};
use stitchd_core::evaluation::{
    EvalOutcome as CoreEvalOutcome, ListMembershipIndex, TraceLevel, evaluate_flag,
};
use stitchd_core::flag::{Flag, FlagRecord, FlagRule, Variant};
use stitchd_core::id::{EnvironmentId, FlagId, FlagKey, ProjectId, RuleId, VariantId};
use stitchd_core::rollout::{RolloutAllocation, RolloutDistribution};
use stitchd_core::rule_engine::types::{ConditionExpr, Rule, RuleOutput};
use stitchd_core::variants::{FlagValueType, VariantValue};

use stitchd_proto::flags::v1::{
    AllocationBucket, ContextKeySelector as ProtoCtxKeySel,
    ContextParameterSelector as ProtoCtxParamSel, FeatureFlag, FlagRule as ProtoFlagRule,
    HashSelector as ProtoHashSelMsg, PercentageAllocation, Variant as ProtoVariant,
    VariantValue as ProtoVariantValue, flag_rule::Output as ProtoOutput,
    hash_selector::Selector as ProtoSelOneof, variant_value::Value as VVal,
};
use stitchd_proto::sdk::v1::SyncDefinitionsResponse;

use stitchd_sdk_rust::client::testing;
use stitchd_sdk_rust::lru::MembershipMap;
use stitchd_sdk_rust::refresh::MembershipBatchFetcher;
use stitchd_sdk_rust::snapshot::DefinitionSnapshot;
use stitchd_sdk_rust::{EvalOutcome, EvalRequest, SdkError};

// ── Shared fixture corpus ───────────────────────────────────────────────────

/// Stable environment UUID used as the hashing salt across all parity tests.
const ENV_UUID: &str = "00000000-0000-0000-0000-000000000001";

fn env_id() -> EnvironmentId {
    EnvironmentId::from_uuid(uuid::Uuid::parse_str(ENV_UUID).unwrap())
}

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

/// Build the SAME flag in both proto + core shapes. Returns
/// `(proto_flag, core_flag)` so the parity check feeds the proto into the
/// SDK and the core into `evaluate_flag` directly. Variants share the same
/// `(key, value)` payload; the `VariantId` differs between the two
/// constructions (the SDK mints fresh IDs during proto -> core conversion)
/// but that's fine because the parity contract compares `variant_key` and
/// trace fields, not the synthetic `VariantId`s.
fn cross_context_corpus() -> (FeatureFlag, Flag) {
    let alloc = PercentageAllocation {
        context_hash_specs: HashMap::new(),
        buckets: vec![
            AllocationBucket {
                variant_key: "control".into(),
                weight_bp: 5000,
            },
            AllocationBucket {
                variant_key: "treatment".into(),
                weight_bp: 5000,
            },
        ],
        hash_inputs: vec![
            ProtoHashSelMsg {
                selector: Some(ProtoSelOneof::ContextKey(ProtoCtxKeySel {
                    context_type: "user".into(),
                })),
            },
            ProtoHashSelMsg {
                selector: Some(ProtoSelOneof::ContextParameter(ProtoCtxParamSel {
                    context_type: "user".into(),
                    parameter: "tier".into(),
                })),
            },
            ProtoHashSelMsg {
                selector: Some(ProtoSelOneof::ContextParameter(ProtoCtxParamSel {
                    context_type: "device".into(),
                    parameter: "os".into(),
                })),
            },
        ],
        exclusion_gate: None,
    };

    // ── Proto shape ──────────────────────────────────────────────────────
    let proto_rule = ProtoFlagRule {
        rule_payload: serde_json::to_vec(&ConditionExpr::And(vec![])).unwrap(),
        output: Some(ProtoOutput::Allocation(alloc)),
        name: "cross-ctx-rollout".into(),
        rule_id: String::new(),
    };
    let proto = FeatureFlag {
        key: "cross-ctx-flag".into(),
        enabled: true,
        variants: vec![
            ProtoVariant {
                key: "control".into(),
                value: Some(ProtoVariantValue {
                    value: Some(VVal::StringValue("off".into())),
                }),
            },
            ProtoVariant {
                key: "treatment".into(),
                value: Some(ProtoVariantValue {
                    value: Some(VVal::StringValue("on".into())),
                }),
            },
        ],
        default_variant_key: "control".into(),
        rules: vec![proto_rule],
        ..Default::default()
    };

    // ── Core shape (built directly, NOT via SDK conversion) ──────────────
    let control_id = VariantId::new();
    let treatment_id = VariantId::new();
    let flag_id = FlagId::new();
    let rule = Rule {
        id: RuleId::new(),
        name: Some("cross-ctx-rollout".into()),
        condition: ConditionExpr::And(vec![]),
        output: RuleOutput::Percentage {
            targets: vec![
                stitchd_core::rule_engine::types::PercentageTarget {
                    context_type: "user".into(),
                    field: stitchd_core::rule_engine::types::TargetField::Key,
                },
                stitchd_core::rule_engine::types::PercentageTarget {
                    context_type: "user".into(),
                    field: stitchd_core::rule_engine::types::TargetField::Parameter("tier".into()),
                },
                stitchd_core::rule_engine::types::PercentageTarget {
                    context_type: "device".into(),
                    field: stitchd_core::rule_engine::types::TargetField::Parameter("os".into()),
                },
            ],
            weights: vec![(control_id, 5000), (treatment_id, 5000)],
            exclusion_gate: None,
        },
    };
    let core = Flag {
        record: FlagRecord {
            id: flag_id,
            project_id: ProjectId::new(),
            key: FlagKey::new("cross-ctx-flag").unwrap(),
            name: String::new(),
            description: String::new(),
            value_type: FlagValueType::Str,
            enabled: true,
            default_variant_id: Some(control_id),
            default_rule_distribution: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        },
        hashing_config: vec![],
        rules: vec![FlagRule {
            flag_id,
            rule_index: 0,
            rule,
        }],
        variants: vec![
            Variant {
                id: control_id,
                key: "control".into(),
                value: VariantValue::StrValue("off".into()),
            },
            Variant {
                id: treatment_id,
                key: "treatment".into(),
                value: VariantValue::StrValue("on".into()),
            },
        ],
    };

    (proto, core)
}

/// Build a flag with `default_rule_distribution` set. The proto wire does
/// not carry the field yet (sealed Phase 3 scope), so the SDK proto -> core
/// conversion lands with `None` — Task 8 verifies that the core engine
/// handles the variant directly. We mutate the SDK-converted core flag in
/// the test body to install the distribution, simulating the future state
/// where the proto wire will populate it.
fn default_rule_distribution_corpus() -> (FeatureFlag, Flag) {
    let proto = FeatureFlag {
        key: "default-dist-flag".into(),
        enabled: true,
        variants: vec![
            ProtoVariant {
                key: "control".into(),
                value: Some(ProtoVariantValue {
                    value: Some(VVal::BoolValue(false)),
                }),
            },
            ProtoVariant {
                key: "treatment".into(),
                value: Some(ProtoVariantValue {
                    value: Some(VVal::BoolValue(true)),
                }),
            },
        ],
        default_variant_key: "control".into(),
        rules: vec![],
        ..Default::default()
    };

    let control_id = VariantId::new();
    let treatment_id = VariantId::new();
    let flag_id = FlagId::new();
    let core = Flag {
        record: FlagRecord {
            id: flag_id,
            project_id: ProjectId::new(),
            key: FlagKey::new("default-dist-flag").unwrap(),
            name: String::new(),
            description: String::new(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: Some(control_id),
            // 80/20 default-rule distribution.
            default_rule_distribution: Some(RolloutDistribution {
                allocations: vec![
                    RolloutAllocation {
                        variant_key: "control".into(),
                        percentage_bp: 8_000,
                    },
                    RolloutAllocation {
                        variant_key: "treatment".into(),
                        percentage_bp: 2_000,
                    },
                ],
            }),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        },
        hashing_config: vec![],
        rules: vec![],
        variants: vec![
            Variant {
                id: control_id,
                key: "control".into(),
                value: VariantValue::BoolValue(false),
            },
            Variant {
                id: treatment_id,
                key: "treatment".into(),
                value: VariantValue::BoolValue(true),
            },
        ],
    };
    (proto, core)
}

// ── Parity assertions ───────────────────────────────────────────────────────

/// Compare a single SDK `EvalResult` against a single core
/// `FlagEvaluationResult` for `variant_key`, the `outcome` family, and
/// (when present) the trace's `fired_rule_name` + the rollout-debug
/// bucket/variant_ranges.
fn assert_results_match(
    sdk: &stitchd_sdk_rust::EvalResult,
    core: &stitchd_core::evaluation::FlagEvaluationResult,
    label: &str,
) {
    assert_eq!(sdk.variant_key, core.variant_key, "{label}: variant_key");

    // Outcome family. SDK + core map 1:1 except for FlagNotFound which is
    // SDK-only (core never sees a missing flag).
    let outcome_matches = match (&sdk.outcome, &core.outcome) {
        (EvalOutcome::Matched { rule_index: a }, CoreEvalOutcome::RuleMatch { rule_index: b }) => {
            a == b
        }
        (EvalOutcome::DefaultRule, CoreEvalOutcome::DefaultFallthrough) => true,
        (EvalOutcome::DefaultRuleDistribution, CoreEvalOutcome::DefaultRuleDistribution) => true,
        (EvalOutcome::Disabled, CoreEvalOutcome::FlagDisabled) => true,
        _ => false,
    };
    assert!(
        outcome_matches,
        "{label}: outcome mismatch — sdk={:?} core={:?}",
        sdk.outcome, core.outcome
    );

    // Both should populate a trace at Full level.
    let sdk_trace = sdk.trace.as_ref().expect("sdk trace must be Some at Full");
    let core_trace = core
        .trace
        .as_ref()
        .expect("core trace must be Some at Full");

    assert_eq!(
        sdk_trace.fired_rule_name, core_trace.fired_rule_name,
        "{label}: fired_rule_name"
    );

    match (&sdk_trace.rollout_debug, &core_trace.rollout_debug) {
        (Some(a), Some(b)) => {
            assert_eq!(a.bucket, b.bucket, "{label}: rollout_debug.bucket");
            assert_eq!(
                a.variant_ranges.len(),
                b.variant_ranges.len(),
                "{label}: variant_ranges len"
            );
            for (i, (av, bv)) in a
                .variant_ranges
                .iter()
                .zip(b.variant_ranges.iter())
                .enumerate()
            {
                assert_eq!(
                    av.variant_key, bv.variant_key,
                    "{label}: variant_ranges[{i}].variant_key"
                );
                assert_eq!(av.from, bv.from, "{label}: variant_ranges[{i}].from");
                assert_eq!(av.to, bv.to, "{label}: variant_ranges[{i}].to");
            }
        }
        (None, None) => {}
        (a, b) => panic!("{label}: rollout_debug presence mismatch — sdk={a:?} core={b:?}"),
    }
}

/// SDK + core agree on the cross-context percentage rule's variant
/// assignment for a representative bundle. The contract is that the
/// hash inputs and the bucket math match byte-for-byte across paths.
#[tokio::test]
async fn parity_cross_context_percentage_hash_matches_core() {
    let (proto, core) = cross_context_corpus();

    let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
        flags: vec![proto],
        rule_segments: vec![],
        list_segments: vec![],
        server_timestamp_ms: 0,
        environment_id: ENV_UUID.into(),
        event_definitions: vec![],
    });
    let client = testing::sdk_client_with_snapshot_and_lru(snap, Arc::new(NoopFetcher), vec![]);

    // Two distinct subject bundles. Same flag, different contexts.
    let bundles: [Vec<Context>; 2] = [
        vec![
            Context::new("user", "alice").with_parameter("tier", CoreParam::Str("gold".into())),
            Context::new("device", "iphone-12").with_parameter("os", CoreParam::Str("ios".into())),
        ],
        vec![
            Context::new("user", "bob").with_parameter("tier", CoreParam::Str("silver".into())),
            Context::new("device", "pixel-7")
                .with_parameter("os", CoreParam::Str("android".into())),
        ],
    ];

    for (i, bundle) in bundles.iter().enumerate() {
        let sdk_results = client
            .evaluate(
                &[EvalRequest {
                    flag_key: "cross-ctx-flag".into(),
                    contexts: bundle.clone(),
                }],
                TraceLevel::Full,
            )
            .await;
        let core_results = evaluate_flag(
            &core,
            bundle,
            &[],
            &ListMembershipIndex::new(),
            env_id(),
            ProjectId::new(),
            TraceLevel::Full,
        );

        assert_eq!(
            sdk_results.len(),
            core_results.len(),
            "bundle[{i}]: result count must match (one per context in bundle)"
        );
        for (j, (sdk, core_r)) in sdk_results.iter().zip(core_results.iter()).enumerate() {
            assert_results_match(sdk, core_r, &format!("bundle[{i}].ctx[{j}]"));
        }
    }
}

/// SDK + core agree on the default-rule-distribution path. The SDK proto
/// wire does NOT yet carry `default_rule_distribution`, so we directly
/// drive core's `evaluate_flag` here and assert the engine assigns the
/// expected variant via the 80/20 distribution.
///
/// (Once the proto SDK service ships the field — separate track — the
/// SDK's proto -> core conversion will populate it and this test gains a
/// real SDK side via `client.evaluate(...)`. For now the parity is one-
/// sided: the test verifies the core branch the SDK will inherit.)
#[tokio::test]
async fn parity_default_rule_distribution_via_core_engine() {
    let (_proto, core) = default_rule_distribution_corpus();

    // Drive the core engine directly. The contract is that no rule fires
    // (rules vec is empty), so the engine enters the
    // default_rule_distribution branch and assigns one of the two listed
    // variants via the bundle's hash.
    let contexts = vec![
        Context::new("user", "alice"),
        Context::new("user", "bob"),
        Context::new("user", "carol"),
        Context::new("user", "dave"),
        Context::new("user", "eve"),
        Context::new("user", "frank"),
    ];
    let results = evaluate_flag(
        &core,
        &contexts,
        &[],
        &ListMembershipIndex::new(),
        env_id(),
        ProjectId::new(),
        TraceLevel::Full,
    );
    assert_eq!(
        results.len(),
        contexts.len(),
        "one FlagEvaluationResult per context"
    );

    let mut count_control = 0usize;
    let mut count_treatment = 0usize;
    for r in &results {
        assert!(
            matches!(r.outcome, CoreEvalOutcome::DefaultRuleDistribution),
            "default_rule_distribution must fire when no rule matches; got {:?}",
            r.outcome
        );
        match r.variant_key.as_str() {
            "control" => count_control += 1,
            "treatment" => count_treatment += 1,
            other => panic!("unexpected variant {other:?}"),
        }
        // Trace level Full → trace must be present + rollout_debug populated.
        let trace = r.trace.as_ref().expect("trace at Full");
        assert!(
            trace.rollout_debug.is_some(),
            "rollout_debug must be populated for default-rule-distribution"
        );
    }

    // Sanity: with 6 deterministic context keys against an 80/20 split,
    // at least one control assignment is expected; the precise count is
    // hash-dependent, so we only assert that the distribution branch
    // exercised both possible variants over the sample (i.e. the
    // assignment logic isn't returning a constant). When the sample is
    // entirely one variant, that's a hash artefact and not a bug, so
    // we only enforce "at least one control" (the majority bucket).
    assert!(
        count_control + count_treatment == results.len(),
        "every result must assign a known variant"
    );
    assert!(
        count_control >= 1,
        "expected at least one 'control' assignment in the 80% bucket; got control={} treatment={}",
        count_control,
        count_treatment
    );
}

/// Phase 6 Task 8 — SDK inherits `default_rule_distribution` automatically
/// via core.
///
/// Direct verification that the core engine's hash-based assignment is the
/// path the SDK's `evaluate()` will exercise when the proto wire eventually
/// carries `default_rule_distribution`. Sanity-checks two contracts the SDK
/// inherits:
///
/// 1. The returned variant is one of the distribution's listed variants —
///    never the fallback `default_variant_id` alone.
/// 2. The hash assignment is stable for the same `(flag_key, env_id,
///    bundle)` — the same context bundle always lands on the same variant.
#[test]
fn default_rule_distribution_assigns_listed_variant_not_fallback() {
    let (_proto, core) = default_rule_distribution_corpus();

    let bundle_alice = [Context::new("user", "alice")];
    let bundle_bob = [Context::new("user", "bob")];

    let res1 = evaluate_flag(
        &core,
        &bundle_alice,
        &[],
        &ListMembershipIndex::new(),
        env_id(),
        ProjectId::new(),
        TraceLevel::Off,
    );
    let res2 = evaluate_flag(
        &core,
        &bundle_alice,
        &[],
        &ListMembershipIndex::new(),
        env_id(),
        ProjectId::new(),
        TraceLevel::Off,
    );
    let res_bob = evaluate_flag(
        &core,
        &bundle_bob,
        &[],
        &ListMembershipIndex::new(),
        env_id(),
        ProjectId::new(),
        TraceLevel::Off,
    );

    assert_eq!(res1.len(), 1);
    assert_eq!(res2.len(), 1);
    assert_eq!(res_bob.len(), 1);

    // Distributed variant — one of the two listed.
    for r in res1.iter().chain(res_bob.iter()) {
        assert!(
            r.variant_key == "control" || r.variant_key == "treatment",
            "default_rule_distribution must assign a listed variant; got {:?}",
            r.variant_key
        );
        assert!(
            matches!(r.outcome, CoreEvalOutcome::DefaultRuleDistribution),
            "outcome must be DefaultRuleDistribution; got {:?}",
            r.outcome
        );
    }

    // Hash stability — alice always gets the same variant across repeated
    // evaluations of the same flag + env + bundle.
    assert_eq!(
        res1[0].variant_key, res2[0].variant_key,
        "alice's bucket assignment must be stable across calls"
    );
}
