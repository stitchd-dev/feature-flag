//! Exclusion-group eval-gating parity test — Phase 2 of `xexp_interaction`.
//!
//! Proves the exclusion gate carried on a `RuleOutput::Percentage` survives the
//! proto → core → SDK snapshot path and produces the SAME gated outcome on the
//! SDK's local-evaluation path as the core engine (which both the preview and
//! SDK call through `evaluate_flag`).
//!
//! ## What's asserted
//!
//! For a single always-match percentage rule that allocates 100% to `on` and
//! carries an `ExclusionGate { context_type: "user", group_salt, [lo, hi) }`:
//!
//! 1. **In-range context enrolls** — a `user` whose exclusion-group bucket
//!    falls in `[lo, hi)` resolves to `on` (the rule's distribution), on BOTH
//!    the core `evaluate_flag` path and the SDK `evaluate` path.
//! 2. **Out-of-range context held out** — a `user` whose bucket falls outside
//!    `[lo, hi)` is NOT enrolled: the rule behaves as a non-match and the flag
//!    falls through to its default variant `off`. Same on core + SDK.
//! 3. **Missing randomization-unit held out** — a bundle lacking the gate's
//!    `context_type` cannot be bucketed → held out → `off`. Same on core + SDK.
//! 4. **No network call** — the SDK reads only its in-memory `ArcSwap`
//!    snapshot. The membership fetcher panics if invoked; the poll fetcher is a
//!    no-op. The gated flag has no segments, so neither is touched during
//!    `evaluate`.

use std::sync::Arc;

use async_trait::async_trait;

use stitchd_core::context::Context;
use stitchd_core::context::EvaluationContext;
use stitchd_core::evaluation::TraceLevel;
use stitchd_core::evaluation::exclusion::group_bucket;
use stitchd_core::evaluation::preview::evaluate_preview;
use stitchd_core::flag::{Flag, FlagRecord, Variant};
use stitchd_core::id::{EnvironmentId, FlagId, FlagKey, ProjectId, VariantId};
use stitchd_core::rule_engine::types::{
    ConditionExpr, ExclusionGate, PercentageTarget, Rule, RuleOutput, TargetField,
};
use stitchd_core::variants::{FlagValueType, VariantValue};

use stitchd_proto::flags::v1::{
    AllocationBucket, ContextKeySelector as ProtoCtxKey, ExclusionGate as ProtoExclusionGate,
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

const ENV_UUID: &str = "00000000-0000-0000-0000-0000000000e2";
const GATE_SALT: &str = "phase2-exclusion-salt";
const FLAG_KEY: &str = "exclusion-gated-flag";

fn env_id() -> EnvironmentId {
    EnvironmentId::from_uuid(uuid::Uuid::parse_str(ENV_UUID).unwrap())
}

/// A `MembershipBatchFetcher` that panics if called — proves `evaluate` makes
/// no network call (it reads only the ArcSwap snapshot).
struct PanicFetcher;

#[async_trait]
impl MembershipBatchFetcher for PanicFetcher {
    async fn fetch(
        &self,
        _contexts: Vec<(String, String)>,
        _segment_ids: Vec<String>,
    ) -> Result<Vec<MembershipMap>, SdkError> {
        panic!("evaluate must not trigger a membership network fetch for a segment-free flag");
    }
}

/// Find a `user-N` key whose exclusion-group bucket (under `GATE_SALT`) lands
/// in `[lo, hi)`.
fn key_in_bucket_range(lo: u16, hi: u16) -> String {
    for i in 0..200_000u32 {
        let key = format!("user-{i}");
        let b = group_bucket(&key, GATE_SALT);
        if b >= lo && b < hi {
            return key;
        }
    }
    panic!("no user key found with bucket in [{lo}, {hi})");
}

/// Build the proto FeatureFlag with a single always-match percentage rule that
/// allocates 100% to `on` and carries the given exclusion gate.
fn build_proto_flag(gate: &ExclusionGate) -> FeatureFlag {
    let alloc = PercentageAllocation {
        buckets: vec![
            AllocationBucket {
                variant_key: "on".to_string(),
                weight_bp: 10000,
            },
            AllocationBucket {
                variant_key: "off".to_string(),
                weight_bp: 0,
            },
        ],
        hash_inputs: vec![ProtoHashSelector {
            selector: Some(ProtoSelectorOneof::ContextKey(ProtoCtxKey {
                context_type: "user".to_string(),
            })),
        }],
        exclusion_gate: Some(ProtoExclusionGate {
            group_salt: gate.group_salt.clone(),
            bucket_lo: u32::from(gate.bucket_lo),
            bucket_hi: u32::from(gate.bucket_hi),
            context_type: gate.context_type.clone(),
        }),
        realtime_bandit: None,
    };

    let condition = ConditionExpr::And(vec![]); // universal match
    let proto_rule = ProtoFlagRule {
        rule_payload: serde_json::to_vec(&condition).unwrap(),
        output: Some(ProtoOutput::Allocation(alloc)),
        name: "gated-rollout".to_string(),
        rule_id: String::new(),
    };

    FeatureFlag {
        key: FLAG_KEY.to_string(),
        enabled: true,
        variants: vec![
            ProtoVariant {
                key: "on".to_string(),
                value: Some(ProtoVariantValue {
                    value: Some(ProtoVValue::BoolValue(true)),
                }),
                id: String::new(),
            },
            ProtoVariant {
                key: "off".to_string(),
                value: Some(ProtoVariantValue {
                    value: Some(ProtoVValue::BoolValue(false)),
                }),
                id: String::new(),
            },
        ],
        default_variant_key: "off".to_string(),
        rules: vec![proto_rule],
        ..Default::default()
    }
}

/// Build the matching core `Flag` directly (mirrors what the flag-service
/// mapping produces) so the core/preview path can be compared with the SDK.
fn build_core_flag(gate: &ExclusionGate) -> Flag {
    let on_id = VariantId::new();
    let off_id = VariantId::new();
    let flag_id = FlagId::new();

    let record = FlagRecord {
        id: flag_id,
        project_id: ProjectId::new(),
        key: FlagKey::new(FLAG_KEY).unwrap(),
        name: String::new(),
        description: String::new(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: Some(off_id),
        default_rule_distribution: None,
        created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        updated_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        deleted_at: None,
        version: 1,
    };

    let variants = vec![
        Variant {
            id: on_id,
            key: "on".to_string(),
            value: VariantValue::BoolValue(true),
        },
        Variant {
            id: off_id,
            key: "off".to_string(),
            value: VariantValue::BoolValue(false),
        },
    ];

    let rule = Rule {
        id: stitchd_core::id::RuleId::new(),
        name: Some("gated-rollout".to_string()),
        condition: ConditionExpr::And(vec![]),
        output: RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Key,
            }],
            weights: vec![(on_id, 10000), (off_id, 0)],
            exclusion_gate: Some(gate.clone()),
            realtime_bandit: None,
        },
    };

    Flag {
        record,
        hashing_config: vec![],
        rules: vec![stitchd_core::flag::FlagRule {
            flag_id,
            rule_index: 0,
            rule,
        }],
        variants,
        prerequisites: stitchd_core::prerequisite::PrerequisiteGate::default(),
    }
}

/// Variant the core preview path assigns for `ctx` against `core_flag`.
fn preview_variant(core_flag: &Flag, ctx: &Context) -> String {
    let ec = EvaluationContext::new().with_context(ctx.clone());
    let results = evaluate_preview(core_flag, &[ec], &[], env_id(), &[]);
    results[0].variant_key.clone()
}

/// Build an SDK client over an in-memory snapshot of `proto_flag`, with a
/// membership fetcher that panics if any network fetch is attempted.
fn sdk_for(proto_flag: &FeatureFlag) -> Arc<stitchd_sdk_rust::SdkClient> {
    let snapshot = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
        flags: vec![proto_flag.clone()],
        rule_segments: vec![],
        list_segments: vec![],
        server_timestamp_ms: 0,
        environment_id: ENV_UUID.to_string(),
        event_definitions: vec![],
    });
    testing::sdk_client_with_snapshot_and_lru(snapshot, Arc::new(PanicFetcher), vec![])
}

async fn sdk_variant(sdk: &stitchd_sdk_rust::SdkClient, ctx: Context) -> String {
    let results = sdk
        .evaluate(&[EvalRequest::single(FLAG_KEY, ctx)], TraceLevel::Off)
        .await;
    results[0].variant_key.clone()
}

#[tokio::test]
async fn exclusion_in_range_context_enrolls_on_sdk_and_core() {
    let gate = ExclusionGate {
        group_salt: GATE_SALT.to_string(),
        context_type: "user".to_string(),
        bucket_lo: 0,
        bucket_hi: 5000,
    };
    let core_flag = build_core_flag(&gate);
    let proto_flag = build_proto_flag(&gate);
    let sdk = sdk_for(&proto_flag);

    let key = key_in_bucket_range(0, 5000);
    let ctx = Context::new("user", &key);

    // Core/preview enrolls → "on".
    assert_eq!(preview_variant(&core_flag, &ctx), "on");
    // SDK agrees, reading only its snapshot.
    assert_eq!(sdk_variant(&sdk, ctx).await, "on");
}

#[tokio::test]
async fn exclusion_out_of_range_context_held_out_on_sdk_and_core() {
    let gate = ExclusionGate {
        group_salt: GATE_SALT.to_string(),
        context_type: "user".to_string(),
        bucket_lo: 0,
        bucket_hi: 5000,
    };
    let core_flag = build_core_flag(&gate);
    let proto_flag = build_proto_flag(&gate);
    let sdk = sdk_for(&proto_flag);

    // Bucket >= 5000 → outside [0, 5000) → held out → default "off".
    let key = key_in_bucket_range(5000, 10000);
    let ctx = Context::new("user", &key);

    assert_eq!(preview_variant(&core_flag, &ctx), "off");
    assert_eq!(sdk_variant(&sdk, ctx).await, "off");
}

#[tokio::test]
async fn exclusion_missing_unit_context_held_out_on_sdk_and_core() {
    // Full range — only an absent randomization unit can hold out.
    let gate = ExclusionGate {
        group_salt: GATE_SALT.to_string(),
        context_type: "user".to_string(),
        bucket_lo: 0,
        bucket_hi: 10000,
    };
    let core_flag = build_core_flag(&gate);
    let proto_flag = build_proto_flag(&gate);
    let sdk = sdk_for(&proto_flag);

    // Bundle has no "user" context → cannot bucket → held out → "off".
    let ctx = Context::new("device", "d1");

    assert_eq!(preview_variant(&core_flag, &ctx), "off");
    assert_eq!(sdk_variant(&sdk, ctx).await, "off");
}
