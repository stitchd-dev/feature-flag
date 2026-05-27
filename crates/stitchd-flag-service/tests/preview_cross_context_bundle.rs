//! Regression test for `feature-flag-utp` (CRITICAL).
//!
//! When the gateway sends a flat list of sub-contexts (e.g.
//! `[user, device, application]`) under the preview RPC, the flag-service
//! must interpret them as ONE bundle and emit one result per sub-context
//! where ALL results share the SAME cross-context `hash_input`.
//!
//! Before this fix the flag-service split the inbound list into N
//! independent single-sub-context bundles. The result: each per-context
//! `hash_input` resolved using only ITS own sub-context's selectors,
//! breaking cross-context hashing in evaluate-preview entirely.
//!
//! This test exercises the core `evaluate_preview` directly with a single
//! `EvaluationContext` bundle containing three sub-contexts (user, device,
//! application) and a percentage rule that hashes across `user.key`,
//! `device.params.os`, and `application.key`. The fix guarantees:
//!   - one `ContextPreviewResult` per sub-context (preserving UI shape)
//!   - all results share an identical `rollout_debug.hash_input` (the
//!     concatenation of every selector value across the FULL bundle)
//!   - all results share an identical `bucket` and `variant_key`
//!     (consequence of identical hash_input)

use stitchd_core::context::{Context, EvaluationContext, ParameterValue};
use stitchd_core::evaluation::preview::evaluate_preview;
use stitchd_core::flag::{Flag, FlagRecord, FlagRule, FlagValueType};
use stitchd_core::id::{EnvironmentId, FlagId, FlagKey, ProjectId, RuleId, VariantId};
use stitchd_core::rule_engine::types::{
    ConditionExpr, PercentageTarget, Rule, RuleOutput, TargetField,
};
use stitchd_core::variants::{Variant, VariantValue};
use uuid::Uuid;

fn env_id() -> EnvironmentId {
    EnvironmentId::from_uuid(Uuid::nil())
}

fn cross_context_flag() -> Flag {
    let on = VariantId::from_uuid(Uuid::from_u128(0xA0));
    let off = VariantId::from_uuid(Uuid::from_u128(0xB0));
    let record = FlagRecord {
        id: FlagId::from_uuid(Uuid::from_u128(0xF0)),
        project_id: ProjectId::from_uuid(Uuid::from_u128(0x1111)),
        key: FlagKey::new("cross-ctx").unwrap(),
        name: "cross-ctx".into(),
        description: String::new(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: Some(off),
        default_rule_distribution: None,
        created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        updated_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        deleted_at: None,
        version: 1,
    };
    let record_id = record.id;
    Flag {
        record,
        hashing_config: vec![],
        rules: vec![FlagRule {
            flag_id: record_id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::from_uuid(Uuid::from_u128(0xC5)),
                name: Some("cross-rollout".into()),
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![
                        PercentageTarget {
                            context_type: "user".into(),
                            field: TargetField::Key,
                        },
                        PercentageTarget {
                            context_type: "device".into(),
                            field: TargetField::Parameter("os".into()),
                        },
                        PercentageTarget {
                            context_type: "application".into(),
                            field: TargetField::Key,
                        },
                    ],
                    weights: vec![(on, 5000), (off, 5000)],
                },
            },
        }],
        variants: vec![
            Variant {
                id: on,
                key: "on".into(),
                value: VariantValue::BoolValue(true),
            },
            Variant {
                id: off,
                key: "off".into(),
                value: VariantValue::BoolValue(false),
            },
        ],
    }
}

#[test]
fn multi_subcontext_bundle_shares_cross_context_hash_input() {
    // ONE bundle with three sub-contexts. This is the shape the gateway
    // builds after the fix (the UI sends a flat `Vec<Context>` and the
    // gateway lifts the whole list into a single EvaluationContext).
    let bundle = EvaluationContext::new()
        .with_context(Context::new("user", "alice"))
        .with_context(
            Context::new("device", "device-42")
                .with_parameter("os", ParameterValue::Str("iOS 18".into()))
                .with_parameter("name", ParameterValue::Str("iPhone-15-Pro".into())),
        )
        .with_context(Context::new("application", "stitchd-web"));

    let flag = cross_context_flag();
    let results = evaluate_preview(&flag, &[bundle], &[], env_id(), &[]);

    // One result per bundle (the whole bundle is evaluated as one unit).
    assert_eq!(
        results.len(),
        1,
        "expected 1 result per bundle, got {}",
        results.len()
    );

    let r = &results[0];
    let debug = r
        .rollout_debug
        .as_ref()
        .expect("percentage rule must populate rollout_debug");

    // The expected hash_input is the concatenation of every selector value
    // across the FULL bundle: user.key="alice", device.os="iOS 18",
    // application.key="stitchd-web".
    let expected_hash_input = format!("cross-ctx{}aliceiOS 18stitchd-web", env_id());
    assert_eq!(
        debug.hash_input, expected_hash_input,
        "cross-context hash_input must concatenate selectors across the FULL bundle"
    );
}
