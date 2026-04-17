use chrono::Utc;
use std::collections::HashSet;
use std::time::Instant;
use stitchd_core::{
    context::{Context, EvaluationContext, ParameterValue},
    evaluation::FlagEvaluator,
    flag::{Flag, FlagRecord, FlagRule, FlagValueType, Variant, VariantValue},
    id::{FlagId, FlagKey, ProjectId, RuleId, VariantId},
    rule_engine::condition::Condition,
    rule_engine::types::{ConditionExpr, Rule, RuleOutput},
};

fn setup_benchmark_flag() -> Flag {
    let flag_id = FlagId::new();
    let project_id = ProjectId::new();
    let v1_id = VariantId::new();
    let v2_id = VariantId::new();

    let record = FlagRecord {
        id: flag_id,
        project_id,
        key: FlagKey::new("perf-flag").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: Some(v2_id),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };

    let variants = vec![
        Variant {
            id: v1_id,
            key: "on".to_string(),
            value: VariantValue::BoolValue(true),
        },
        Variant {
            id: v2_id,
            key: "off".to_string(),
            value: VariantValue::BoolValue(false),
        },
    ];

    // Simple rule that matches
    let rules = vec![FlagRule {
        flag_id,
        rule_index: 0,
        rule: Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".to_string(),
                param: "tier".to_string(),
                value: ParameterValue::Str("gold".to_string()),
            }),
            output: RuleOutput::Variant(v1_id),
        },
    }];

    Flag {
        record,
        hashing_config: vec![],
        rules,
        variants,
    }
}

#[test]
fn benchmark_evaluation_throughput() {
    let flag = setup_benchmark_flag();
    let context = EvaluationContext::new().with_context(
        Context::new("user", "u1").with_parameter("tier", ParameterValue::Str("gold".to_string())),
    );
    let segments = HashSet::new();
    let env_id = stitchd_core::id::EnvironmentId::from_uuid(uuid::Uuid::nil());

    let iterations = 100_000;
    let start = Instant::now();

    for _ in 0..iterations {
        let _ = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
    }

    let duration = start.elapsed();
    let nanos_per_eval = duration.as_nanos() as f64 / iterations as f64;
    let evals_per_sec = iterations as f64 / duration.as_secs_f64();

    println!("\n--- Evaluation Benchmark ---");
    println!("Iterations: {}", iterations);
    println!("Total Time: {:?}", duration);
    println!("Average time per evaluation: {:.2}ns", nanos_per_eval);
    println!("Throughput: {:.2} evals/sec", evals_per_sec);

    // Goal: < 10,000ns (10us) per evaluation for simple rules
    assert!(
        nanos_per_eval < 50_000.0,
        "Evaluation is too slow: {:.2}ns",
        nanos_per_eval
    );
}
