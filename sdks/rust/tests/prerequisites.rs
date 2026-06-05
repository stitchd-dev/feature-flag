//! Flag-prerequisite SDK integration tests — Phase 7 of `flag_lifecycle_20260604`.
//!
//! The SDK holds every flag definition in its local `DefinitionSnapshot`
//! (definition-sync), so it can resolve flag prerequisites **locally and
//! transitively** without any extra round-trips. These tests synthesize proto
//! `FeatureFlag`s carrying `prerequisites` (by key) + `fallback_variant_key`,
//! build a snapshot, and assert `SdkClient::evaluate(...)` returns the
//! configured fallback variant whenever a prerequisite is unmet — identically
//! to the preview path (Phase 4).
//!
//! Coverage:
//! - (a) an UNMET prerequisite ⇒ the configured fallback variant.
//! - (b) a MET prerequisite ⇒ normal evaluation proceeds.
//! - (c) a TRANSITIVE chain A→B→C; C unmet ⇒ A falls back.
//! - (d) a DISABLED prerequisite flag ⇒ unmet ⇒ fallback.
//! - (e) a prerequisite flag ABSENT from the snapshot ⇒ unmet ⇒ fallback.
//!
//! Run with:
//! ```
//! cargo test --features test-util -p stitchd-sdk-rust --test prerequisites
//! ```

use stitchd_core::context::Context;
use stitchd_core::evaluation::TraceLevel;

use stitchd_proto::flags::v1::{
    FeatureFlag, FlagPrerequisite, FlagRule as ProtoFlagRule, Variant as ProtoVariant,
    VariantValue as ProtoVariantValue, flag_rule::Output as ProtoOutput,
    variant_value::Value as VVal,
};
use stitchd_proto::sdk::v1::SyncDefinitionsResponse;

use stitchd_core::rule_engine::types::ConditionExpr;

use stitchd_sdk_rust::client::testing;
use stitchd_sdk_rust::snapshot::DefinitionSnapshot;
use stitchd_sdk_rust::{EvalOutcome, EvalRequest};

const ENV_UUID: &str = "00000000-0000-0000-0000-000000000001";

// ── Builders ─────────────────────────────────────────────────────────────────

fn string_variant(key: &str, value: &str) -> ProtoVariant {
    ProtoVariant {
        key: key.to_string(),
        value: Some(ProtoVariantValue {
            value: Some(VVal::StringValue(value.to_string())),
        }),
    }
}

/// An always-true rule (empty AND tree) emitting `variant_key`.
fn always_rule(variant_key: &str) -> ProtoFlagRule {
    let condition = ConditionExpr::And(vec![]);
    ProtoFlagRule {
        rule_payload: serde_json::to_vec(&condition).unwrap(),
        output: Some(ProtoOutput::VariantKey(variant_key.to_string())),
        name: String::new(),
        rule_id: String::new(),
    }
}

/// A flag with three string variants `on` / `off` / `fallback`, an always-on
/// rule selecting `on`, no prerequisites. `flag_id` is a stable UUID derived
/// from `key` so the snapshot's cross-flag IDs are deterministic.
fn flag(key: &str, flag_id: &str) -> FeatureFlag {
    FeatureFlag {
        key: key.to_string(),
        enabled: true,
        flag_id: flag_id.to_string(),
        variants: vec![
            string_variant("on", "ON"),
            string_variant("off", "OFF"),
            string_variant("fallback", "FB"),
        ],
        rules: vec![always_rule("on")],
        default_variant_key: "off".to_string(),
        ..Default::default()
    }
}

/// Like [`flag`] but resolves to `off` (its rule selects `off`), so it does
/// NOT satisfy a prerequisite that requires `on`.
fn flag_resolving_off(key: &str, flag_id: &str) -> FeatureFlag {
    let mut f = flag(key, flag_id);
    f.rules = vec![always_rule("off")];
    f
}

fn snapshot(flags: Vec<FeatureFlag>) -> DefinitionSnapshot {
    DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
        flags,
        rule_segments: vec![],
        list_segments: vec![],
        event_definitions: vec![],
        server_timestamp_ms: 0,
        environment_id: ENV_UUID.to_string(),
    })
}

fn ctx() -> Context {
    Context::new("user", "u-1")
}

// ── (a) Unmet prerequisite ⇒ fallback ────────────────────────────────────────

#[tokio::test]
async fn unmet_prerequisite_returns_fallback_variant() {
    // dependent requires prereq=on, but prereq resolves to `off`.
    let prereq = flag_resolving_off("prereq", "00000000-0000-0000-0000-0000000000a1");
    let mut dependent = flag("dependent", "00000000-0000-0000-0000-0000000000a2");
    dependent.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "prereq".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    dependent.fallback_variant_key = "fallback".to_string();

    let client = testing::sdk_client_simple(snapshot(vec![prereq, dependent]));
    let results = client
        .evaluate(
            &[EvalRequest::single("dependent", ctx())],
            TraceLevel::Full,
        )
        .await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, EvalOutcome::PrerequisiteFailed);
    assert_eq!(results[0].variant_key, "fallback");
}

// ── (b) Met prerequisite ⇒ normal evaluation ─────────────────────────────────

#[tokio::test]
async fn met_prerequisite_allows_normal_evaluation() {
    // prereq resolves to `on` (its rule), which satisfies the gate.
    let prereq = flag("prereq", "00000000-0000-0000-0000-0000000000b1");
    let mut dependent = flag("dependent", "00000000-0000-0000-0000-0000000000b2");
    dependent.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "prereq".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    dependent.fallback_variant_key = "fallback".to_string();

    let client = testing::sdk_client_simple(snapshot(vec![prereq, dependent]));
    let results = client
        .evaluate(
            &[EvalRequest::single("dependent", ctx())],
            TraceLevel::Full,
        )
        .await;

    assert_eq!(results.len(), 1);
    // Gate passed → the dependent's own always-on rule fired → `on`.
    assert_eq!(results[0].variant_key, "on");
    assert!(matches!(results[0].outcome, EvalOutcome::Matched { .. }));
}

// ── (c) Transitive chain A→B→C, C unmet ⇒ A falls back ────────────────────────

#[tokio::test]
async fn transitive_unmet_prerequisite_falls_back() {
    // C resolves to `off`; B requires C=on (so B is unmet → B falls back, not
    // resolving to `on`); A requires B=on → A is unmet → A returns fallback.
    let c = flag_resolving_off("c", "00000000-0000-0000-0000-0000000000c3");

    let mut b = flag("b", "00000000-0000-0000-0000-0000000000c2");
    b.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "c".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    b.fallback_variant_key = "fallback".to_string();

    let mut a = flag("a", "00000000-0000-0000-0000-0000000000c1");
    a.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "b".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    a.fallback_variant_key = "fallback".to_string();

    let client = testing::sdk_client_simple(snapshot(vec![a, b, c]));
    let results = client
        .evaluate(&[EvalRequest::single("a", ctx())], TraceLevel::Full)
        .await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, EvalOutcome::PrerequisiteFailed);
    assert_eq!(results[0].variant_key, "fallback");
}

// ── (c2) Transitive chain, all met ⇒ A proceeds ──────────────────────────────

#[tokio::test]
async fn transitive_met_prerequisite_proceeds() {
    // C=on, B requires C=on (met → B=on), A requires B=on (met → A proceeds).
    let c = flag("c", "00000000-0000-0000-0000-0000000000d3");

    let mut b = flag("b", "00000000-0000-0000-0000-0000000000d2");
    b.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "c".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    b.fallback_variant_key = "fallback".to_string();

    let mut a = flag("a", "00000000-0000-0000-0000-0000000000d1");
    a.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "b".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    a.fallback_variant_key = "fallback".to_string();

    let client = testing::sdk_client_simple(snapshot(vec![a, b, c]));
    let results = client
        .evaluate(&[EvalRequest::single("a", ctx())], TraceLevel::Full)
        .await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].variant_key, "on");
    assert!(matches!(results[0].outcome, EvalOutcome::Matched { .. }));
}

// ── (d) Disabled prerequisite flag ⇒ unmet ⇒ fallback ────────────────────────

#[tokio::test]
async fn disabled_prerequisite_flag_falls_back() {
    // prereq is disabled → resolves to no variant → gate unmet → fallback.
    let mut prereq = flag("prereq", "00000000-0000-0000-0000-0000000000e1");
    prereq.enabled = false;

    let mut dependent = flag("dependent", "00000000-0000-0000-0000-0000000000e2");
    dependent.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "prereq".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    dependent.fallback_variant_key = "fallback".to_string();

    let client = testing::sdk_client_simple(snapshot(vec![prereq, dependent]));
    let results = client
        .evaluate(
            &[EvalRequest::single("dependent", ctx())],
            TraceLevel::Full,
        )
        .await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, EvalOutcome::PrerequisiteFailed);
    assert_eq!(results[0].variant_key, "fallback");
}

// ── (e) Prerequisite flag absent from snapshot ⇒ unmet ⇒ fallback ────────────

#[tokio::test]
async fn missing_prerequisite_flag_falls_back() {
    // The prerequisite "ghost" flag is NOT in the snapshot at all → unmet.
    let mut dependent = flag("dependent", "00000000-0000-0000-0000-0000000000f2");
    dependent.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "ghost".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    dependent.fallback_variant_key = "fallback".to_string();

    let client = testing::sdk_client_simple(snapshot(vec![dependent]));
    let results = client
        .evaluate(
            &[EvalRequest::single("dependent", ctx())],
            TraceLevel::Full,
        )
        .await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, EvalOutcome::PrerequisiteFailed);
    assert_eq!(results[0].variant_key, "fallback");
}

// ── (f) Empty fallback_variant_key ⇒ falls back to default/off variant ────────

#[tokio::test]
async fn empty_fallback_uses_off_default_variant() {
    // No explicit fallback key → the gate uses the flag's default/off variant.
    let prereq = flag_resolving_off("prereq", "00000000-0000-0000-0000-0000000000aa");
    let mut dependent = flag("dependent", "00000000-0000-0000-0000-0000000000ab");
    dependent.prerequisites = vec![FlagPrerequisite {
        prerequisite_flag_id: String::new(),
        prerequisite_flag_key: "prereq".to_string(),
        required_variant_id: String::new(),
        required_variant_key: "on".to_string(),
    }];
    dependent.fallback_variant_key = String::new();

    let client = testing::sdk_client_simple(snapshot(vec![prereq, dependent]));
    let results = client
        .evaluate(
            &[EvalRequest::single("dependent", ctx())],
            TraceLevel::Full,
        )
        .await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, EvalOutcome::PrerequisiteFailed);
    // Falls back to the flag's default variant (`off`).
    assert_eq!(results[0].variant_key, "off");
}
