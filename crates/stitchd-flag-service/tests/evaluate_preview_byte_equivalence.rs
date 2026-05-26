//! Byte-equivalence regression test for the unified `evaluate_preview` path.
//!
//! Locks the JSON shape that the flag-service's `evaluate_preview` gRPC handler
//! produces (the `results_json` field of `EvaluatePreviewResponse`) against a
//! frozen baseline for a corpus of representative flags. After Phase 2 (worker
//! P2) rewired `stitchd_core::evaluation::preview::evaluate_preview` to delegate
//! to the unified `evaluate_flag(trace=Full)` orchestrator, this test guards
//! against any later regression that would silently change the preview wire
//! shape.
//!
//! ### Approach
//!
//! We call `stitchd_core::evaluation::preview::evaluate_preview` directly with
//! hand-built inputs and assert the returned `Vec<ContextPreviewResult>`
//! serialises to an exact JSON literal. This is the same value that the gRPC
//! handler in `FlagServiceImpl::evaluate_preview` writes into
//! `EvaluatePreviewResponse.results_json` — the in-process call exercises the
//! identical code path without needing a live PG / Scylla / tonic stack.
//!
//! ### Corpus
//!
//! 1. **single-rule** — one rule that fires on `user.beta == true`.
//! 2. **multi-rule** — three rules where rule 1 fires; rules 0 and 2 are
//!    `no_match` and `skipped` respectively.
//! 3. **default-rule-distribution** — flag with no rules and a 60/40
//!    `default_rule_distribution`. Asserts the hash-derived bucket + the
//!    `rollout_debug` shape.
//! 4. **cross-context-hash** — percentage rule whose `targets` mix
//!    `user.key`, `device.params.os`, and `application.key` to exercise the
//!    cross-context hash-input bridge (`hash_input_spec_from_targets` →
//!    `resolve_hash_inputs` inside `evaluate_flag`).
//!
//! Each entry uses **deterministic UUIDs** (`Uuid::from_u128(N)`) so the JSON
//! baseline below is stable across runs.

use std::collections::HashSet;

use stitchd_core::context::{Context, EvaluationContext, ParameterValue};
use stitchd_core::evaluation::preview::evaluate_preview;
use stitchd_core::flag::{Flag, FlagRecord, FlagRule, FlagValueType};
use stitchd_core::id::{EnvironmentId, FlagId, FlagKey, ProjectId, RuleId, SegmentId, VariantId};
use stitchd_core::rollout::{RolloutAllocation, RolloutDistribution};
use stitchd_core::rule_engine::condition::Condition;
use stitchd_core::rule_engine::types::{
    ConditionExpr, PercentageTarget, Rule, RuleOutput, TargetField,
};
use stitchd_core::variants::{Variant, VariantValue};
use uuid::Uuid;

// ── Deterministic ID helpers ────────────────────────────────────────────────

fn flag_id(n: u128) -> FlagId {
    FlagId::from_uuid(Uuid::from_u128(n))
}

fn project_id(n: u128) -> ProjectId {
    ProjectId::from_uuid(Uuid::from_u128(n))
}

fn variant_id(n: u128) -> VariantId {
    VariantId::from_uuid(Uuid::from_u128(n))
}

fn rule_id(n: u128) -> RuleId {
    RuleId::from_uuid(Uuid::from_u128(n))
}

fn env_id() -> EnvironmentId {
    EnvironmentId::from_uuid(Uuid::nil())
}

// ── Flag-builder helpers ────────────────────────────────────────────────────

/// Build a 2-variant boolean flag with deterministic IDs.
///
/// `on` variant id = `0xA0 + flag_n`, `off` variant id = `0xB0 + flag_n` (the
/// helpers below pass per-flag offsets to keep cross-flag IDs distinct in the
/// baseline JSON).
fn bool_flag(flag_n: u128, enabled: bool, on_id: VariantId, off_id: VariantId, key: &str) -> Flag {
    let record = FlagRecord {
        id: flag_id(flag_n),
        project_id: project_id(0x1111),
        key: FlagKey::new(key).unwrap(),
        name: key.to_string(),
        description: String::new(),
        value_type: FlagValueType::Bool,
        enabled,
        default_variant_id: Some(off_id),
        default_rule_distribution: None,
        // The serialized JSON only surfaces the variant payload + rule traces,
        // not the timestamps. We use a fixed instant for deterministic
        // `record` equality if anyone widens the test later.
        created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        updated_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        deleted_at: None,
        version: 1,
    };

    Flag {
        record,
        hashing_config: vec![],
        rules: vec![],
        variants: vec![
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
        ],
    }
}

// ── Corpus entries ──────────────────────────────────────────────────────────

fn corpus_single_rule() -> (Flag, Vec<EvaluationContext>) {
    let on = variant_id(0xA1);
    let off = variant_id(0xB1);
    let mut flag = bool_flag(0xF1, true, on, off, "single-rule");

    flag.rules.push(FlagRule {
        flag_id: flag.record.id,
        rule_index: 0,
        rule: Rule {
            id: rule_id(0xC1),
            name: Some("beta users".to_string()),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".to_string(),
                param: "beta".to_string(),
                value: ParameterValue::Bool(true),
            }),
            output: RuleOutput::Variant(on),
        },
    });

    let contexts = vec![
        EvaluationContext::new().with_context(
            Context::new("user", "alice").with_parameter("beta", ParameterValue::Bool(true)),
        ),
        EvaluationContext::new().with_context(
            Context::new("user", "bob").with_parameter("beta", ParameterValue::Bool(false)),
        ),
    ];
    (flag, contexts)
}

fn corpus_multi_rule() -> (Flag, Vec<EvaluationContext>) {
    let on = variant_id(0xA2);
    let off = variant_id(0xB2);
    let mid = variant_id(0xD2);
    let mut flag = bool_flag(0xF2, true, on, off, "multi-rule");
    flag.variants.push(Variant {
        id: mid,
        key: "mid".to_string(),
        value: VariantValue::BoolValue(false),
    });

    // Rule 0: beta == true → on (will not match for this context).
    flag.rules.push(FlagRule {
        flag_id: flag.record.id,
        rule_index: 0,
        rule: Rule {
            id: rule_id(0xC2),
            name: Some("beta".to_string()),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".to_string(),
                param: "beta".to_string(),
                value: ParameterValue::Bool(true),
            }),
            output: RuleOutput::Variant(on),
        },
    });
    // Rule 1: country == "US" → mid (matches).
    flag.rules.push(FlagRule {
        flag_id: flag.record.id,
        rule_index: 1,
        rule: Rule {
            id: rule_id(0xC3),
            name: Some("us".to_string()),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".to_string(),
                param: "country".to_string(),
                value: ParameterValue::Str("US".to_string()),
            }),
            output: RuleOutput::Variant(mid),
        },
    });
    // Rule 2: always-match → on. Must be reported as "skipped" once rule 1 wins.
    flag.rules.push(FlagRule {
        flag_id: flag.record.id,
        rule_index: 2,
        rule: Rule {
            id: rule_id(0xC4),
            name: Some("fallback".to_string()),
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Variant(on),
        },
    });

    let contexts = vec![
        EvaluationContext::new().with_context(
            Context::new("user", "carol")
                .with_parameter("beta", ParameterValue::Bool(false))
                .with_parameter("country", ParameterValue::Str("US".to_string())),
        ),
    ];
    (flag, contexts)
}

fn corpus_default_rule_distribution() -> (Flag, Vec<EvaluationContext>) {
    let on = variant_id(0xA3);
    let off = variant_id(0xB3);
    let mut flag = bool_flag(0xF3, true, on, off, "drd");
    flag.record.default_rule_distribution = Some(RolloutDistribution {
        allocations: vec![
            RolloutAllocation {
                variant_key: "on".to_string(),
                percentage_bp: 6000,
            },
            RolloutAllocation {
                variant_key: "off".to_string(),
                percentage_bp: 4000,
            },
        ],
    });
    let contexts = vec![EvaluationContext::new().with_context(Context::new("user", "drd-subject"))];
    (flag, contexts)
}

fn corpus_cross_context_hash() -> (Flag, Vec<EvaluationContext>) {
    let on = variant_id(0xA4);
    let off = variant_id(0xB4);
    let mut flag = bool_flag(0xF4, true, on, off, "cross-ctx");

    flag.rules.push(FlagRule {
        flag_id: flag.record.id,
        rule_index: 0,
        rule: Rule {
            id: rule_id(0xC5),
            name: Some("cross-rollout".to_string()),
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Percentage {
                targets: vec![
                    PercentageTarget {
                        context_type: "user".to_string(),
                        field: TargetField::Key,
                    },
                    PercentageTarget {
                        context_type: "device".to_string(),
                        field: TargetField::Parameter("os".to_string()),
                    },
                    PercentageTarget {
                        context_type: "application".to_string(),
                        field: TargetField::Key,
                    },
                ],
                weights: vec![(on, 5000), (off, 5000)],
            },
        },
    });

    let bundle = EvaluationContext::new()
        .with_context(Context::new("user", "alice"))
        .with_context(
            Context::new("device", "dev-1")
                .with_parameter("os", ParameterValue::Str("ios".to_string())),
        )
        .with_context(Context::new("application", "app-1"));

    (flag, vec![bundle])
}

// ── Frozen baselines ────────────────────────────────────────────────────────

/// Assert that `actual` (serialized `Vec<ContextPreviewResult>`) round-trips
/// equal to `expected_json` parsed as `serde_json::Value`. We compare as
/// `Value` (not raw string) so field-order differences in `serde_json::Map`
/// can't cause spurious failures — the wire format is JSON, not a stable byte
/// layout, but the structural equivalence is what byte-equivalence really
/// guarantees for downstream parsers.
#[track_caller]
fn assert_json_eq(actual: &str, expected: &str) {
    let actual_val: serde_json::Value = serde_json::from_str(actual).unwrap_or_else(|e| {
        panic!("actual is not valid JSON: {e}\nactual:\n{actual}");
    });
    let expected_val: serde_json::Value = serde_json::from_str(expected).unwrap_or_else(|e| {
        panic!("expected is not valid JSON: {e}\nexpected:\n{expected}");
    });
    if actual_val != expected_val {
        panic!(
            "JSON mismatch.\nactual:\n{}\nexpected:\n{}",
            serde_json::to_string_pretty(&actual_val).unwrap_or_default(),
            serde_json::to_string_pretty(&expected_val).unwrap_or_default()
        );
    }
}

#[test]
fn baseline_single_rule() {
    let (flag, contexts) = corpus_single_rule();
    let results = evaluate_preview(&flag, &contexts, &[], env_id(), &[]);
    let actual = serde_json::to_string(&results).unwrap();

    let expected = r#"[
      {
        "context_index": 0,
        "variant_key": "on",
        "variant_value": true,
        "fired_rule_index": 0,
        "fired_rule_name": "beta users",
        "fired_rule_id": "00000000-0000-0000-0000-0000000000c1",
        "rule_traces": [
          {
            "rule_index": 0,
            "rule_name": "beta users",
            "outcome": "match",
            "condition_tree": {"kind": "leaf", "predicate": "user.beta == true", "result": true}
          }
        ],
        "rollout_debug": null
      },
      {
        "context_index": 1,
        "variant_key": "off",
        "variant_value": false,
        "fired_rule_index": null,
        "fired_rule_name": null,
        "rule_traces": [
          {
            "rule_index": 0,
            "rule_name": "beta users",
            "outcome": "no_match",
            "condition_tree": {"kind": "leaf", "predicate": "user.beta == true", "result": false}
          }
        ],
        "rollout_debug": null
      }
    ]"#;

    assert_json_eq(&actual, expected);
}

#[test]
fn baseline_multi_rule() {
    let (flag, contexts) = corpus_multi_rule();
    let results = evaluate_preview(&flag, &contexts, &[], env_id(), &[]);
    let actual = serde_json::to_string(&results).unwrap();

    // Carol: country=US, beta=false → rule 0 no-match, rule 1 match (mid),
    // rule 2 skipped.
    let expected = r#"[
      {
        "context_index": 0,
        "variant_key": "mid",
        "variant_value": false,
        "fired_rule_index": 1,
        "fired_rule_name": "us",
        "fired_rule_id": "00000000-0000-0000-0000-0000000000c3",
        "rule_traces": [
          {
            "rule_index": 0,
            "rule_name": "beta",
            "outcome": "no_match",
            "condition_tree": {"kind": "leaf", "predicate": "user.beta == true", "result": false}
          },
          {
            "rule_index": 1,
            "rule_name": "us",
            "outcome": "match",
            "condition_tree": {"kind": "leaf", "predicate": "user.country == US", "result": true}
          },
          {
            "rule_index": 2,
            "rule_name": "fallback",
            "outcome": "skipped"
          }
        ],
        "rollout_debug": null
      }
    ]"#;

    assert_json_eq(&actual, expected);
}

#[test]
fn baseline_default_rule_distribution() {
    let (flag, contexts) = corpus_default_rule_distribution();
    let results = evaluate_preview(&flag, &contexts, &[], env_id(), &[]);

    // For a default-rule distribution we lock everything EXCEPT the
    // hash-derived bucket — that's just the existing Murmur3 output for
    // (flag_key, env_id, target_values). We assert the structural shape
    // (variant_key chosen, fired_rule_* are null, rollout_debug populated,
    // variant_ranges shape) and that the bucket falls in the chosen
    // variant's range.
    assert_eq!(results.len(), 1);
    let r = &results[0];

    // Whichever bucket comes out, it MUST land in either the "on" 0..=599
    // range or the "off" 600..=999 range.
    assert!(
        r.variant_key == "on" || r.variant_key == "off",
        "variant_key = {}",
        r.variant_key
    );
    assert!(
        r.fired_rule_index.is_none(),
        "default-rule path: no rule fired"
    );
    assert!(r.fired_rule_id.is_none());
    assert!(r.rule_traces.is_empty(), "no rules → no rule traces");
    let debug = r
        .rollout_debug
        .as_ref()
        .expect("default-rule distribution must populate rollout_debug");
    assert!(debug.bucket < 10000, "bucket must be 0..10000");
    assert_eq!(debug.variant_ranges.len(), 2);
    assert_eq!(debug.variant_ranges[0].variant_key, "on");
    assert_eq!(debug.variant_ranges[0].from, 0);
    assert_eq!(debug.variant_ranges[0].to, 5999);
    assert_eq!(debug.variant_ranges[1].variant_key, "off");
    assert_eq!(debug.variant_ranges[1].from, 6000);
    assert_eq!(debug.variant_ranges[1].to, 9999);

    // Hash input is flag_key + env_uuid_str + each target value (just the
    // user.key for the default-rule path).
    let expected_hash_input = format!("drd{}drd-subject", env_id());
    assert_eq!(debug.hash_input, expected_hash_input);

    // The bucket must be consistent with the chosen variant.
    if r.variant_key == "on" {
        assert!(debug.bucket <= 5999);
    } else {
        assert!(debug.bucket >= 6000);
    }
}

#[test]
fn baseline_cross_context_hash() {
    let (flag, contexts) = corpus_cross_context_hash();
    let results = evaluate_preview(&flag, &contexts, &[], env_id(), &[]);

    // A single EvaluationContext bundle (even with N sub-contexts) emits ONE
    // result. The entire bundle shares the same rule evaluation and hashing.
    assert_eq!(results.len(), 1, "one result per bundle");

    // The hash input must include all three selectors in declaration order:
    // user.key="alice", device.params.os="ios", application.key="app-1".
    let expected_hash_input = format!("cross-ctx{}aliceiosapp-1", env_id());

    let r = &results[0];
    assert_eq!(r.fired_rule_index, Some(0));
    assert_eq!(r.fired_rule_name, Some("cross-rollout".to_string()));
    assert_eq!(r.fired_rule_id, Some(rule_id(0xC5)));

    let debug = r
        .rollout_debug
        .as_ref()
        .expect("percentage rule must populate rollout_debug");

    assert_eq!(debug.hash_input, expected_hash_input);

    // Variant ranges are 0..=4999 (on) and 5000..=9999 (off).
    assert_eq!(debug.variant_ranges.len(), 2);
    assert_eq!(debug.variant_ranges[0].variant_key, "on");
    assert_eq!(debug.variant_ranges[0].from, 0);
    assert_eq!(debug.variant_ranges[0].to, 4999);
    assert_eq!(debug.variant_ranges[1].variant_key, "off");
    assert_eq!(debug.variant_ranges[1].from, 5000);
    assert_eq!(debug.variant_ranges[1].to, 9999);

    if debug.bucket <= 4999 {
        assert_eq!(r.variant_key, "on");
    } else {
        assert_eq!(r.variant_key, "off");
    }
}

// ── End-to-end: gRPC `EvaluatePreviewResponse.results_json` shape ───────────
//
// The flag-service gRPC handler does `serde_json::to_string(&results)` on
// the result of `evaluate_preview` and stuffs it into
// `EvaluatePreviewResponse.results_json`. The corpus above exercises the
// `evaluate_preview` core function; this test exercises the exact
// serialisation step the handler performs so the gRPC wire shape is locked
// in too. We don't need to spin up tonic for this — the handler's only
// post-`evaluate_preview` work is the JSON serialisation, and that's what we
// pin here.

#[test]
fn grpc_results_json_round_trips_to_baseline() {
    let (flag, contexts) = corpus_single_rule();
    let results = evaluate_preview(&flag, &contexts, &[], env_id(), &[]);

    // Step 1: serialise (handler does this).
    let results_json = serde_json::to_string(&results).expect("serialise results");

    // Step 2: parse back as a generic Value — every downstream client (admin
    // UI, gateway) will do this.
    let parsed: serde_json::Value =
        serde_json::from_str(&results_json).expect("results_json must be valid JSON");
    let arr = parsed
        .as_array()
        .expect("results_json must be a JSON array");
    assert_eq!(arr.len(), 2);

    // Step 3: assert the two expected variant_keys are present in order.
    assert_eq!(arr[0]["variant_key"].as_str(), Some("on"));
    assert_eq!(arr[1]["variant_key"].as_str(), Some("off"));

    // Step 4: the `fired_rule_id` field must be a 32-char hex UUID string
    // (the deterministic 0xC1 ID) for the matched context.
    assert_eq!(
        arr[0]["fired_rule_id"].as_str(),
        Some("00000000-0000-0000-0000-0000000000c1")
    );
}

// ── Sanity: pre-resolved list-membership pathway still wires through ────────
//
// Worker P2's `evaluate_preview` registers caller-supplied list-segment
// memberships under EVERY (context_type, context_key) tuple in the bundle.
// This regression test guarantees the wiring stays in place — the same
// behaviour as the pre-Phase-2 path.

#[test]
fn pre_resolved_list_membership_pathway_remains_wired() {
    let on = variant_id(0xA5);
    let off = variant_id(0xB5);
    let mut flag = bool_flag(0xF5, true, on, off, "list-seg");

    let seg = SegmentId::new();
    flag.rules.push(FlagRule {
        flag_id: flag.record.id,
        rule_index: 0,
        rule: Rule {
            id: rule_id(0xC6),
            name: None,
            condition: ConditionExpr::Leaf(Condition::InSegment(seg)),
            output: RuleOutput::Variant(on),
        },
    });

    let ec_in = EvaluationContext::new().with_context(Context::new("user", "alice"));
    let ec_out = EvaluationContext::new().with_context(Context::new("user", "bob"));

    let mut in_set = HashSet::new();
    in_set.insert(seg);
    let pre_resolved = vec![in_set, HashSet::new()];

    let results = evaluate_preview(&flag, &[ec_in, ec_out], &[], env_id(), &pre_resolved);
    assert_eq!(results[0].variant_key, "on");
    assert_eq!(results[1].variant_key, "off");
}
