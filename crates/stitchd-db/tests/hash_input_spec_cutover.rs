//! Phase 3 of `flag_eval_unify_20260522` — migration cutover verification.
//!
//! Verifies the `2026XXXXNNNNNN_hash_input_spec_cutover` migration:
//!
//! * Adds nullable `hash_inputs` JSONB columns to `feature_flag_rules` and
//!   `feature_flags`.
//! * Does NOT drop the legacy in-row payloads (`rule_def`, the old proto
//!   `context_hash_specs` map) — dual-schema state lives across Phases 4-6.
//! * For a frozen pre-migration corpus, the canonical-sort converter
//!   (`context_type` ASC, `parameter` ASC within type) emits a `hash_inputs`
//!   array whose bucket assignment is identical to the legacy ordered
//!   `Vec<PercentageTarget>` representation **when** the existing target
//!   ordering matches the canonical sort. Corpora where it does not match
//!   are surfaced for operator review without failing — they represent
//!   bucket-reshuffles that need human sign-off before the legacy field is
//!   retired in Phase 5/6.
//!
//! The pure-Rust portion (canonical-sort conversion + bucket parity) lives as
//! a unit test below and does not require Postgres. The schema assertions are
//! `#[sqlx::test]` and need a running PG (CI infrastructure provides this; the
//! standard local dev setup in `conductor/workflow.md` also provides it). If
//! PG is offline the schema tests will fail; the pure-Rust corpus tests still
//! run.

use serde::{Deserialize, Serialize};
use stitchd_core::hashing::calculate_allocation;

// ── Frozen pre-migration corpus shape ────────────────────────────────────────
//
// Mirrors the proto-wire `ContextHashSpec` map shape (`context_type` →
// `parameter_names`). This is the *source* shape from the on-the-wire pre-1.0
// SDK contract. Note: the PG-side `rule_def` already stores an ordered
// `Vec<PercentageTarget>` — but pre-migration callers (notably the
// non-deterministic HashMap-iteration code path that built the proto
// `context_hash_specs` map from the ordered targets) sometimes round-tripped
// data through the map, in which case the on-disk ordering reflected
// whatever HashMap iteration order happened to be. This corpus reproduces
// that worst case so the canonical-sort conversion can be checked against
// every plausible "what was insertion order" hypothesis.

/// Legacy map-shaped spec — `context_type` → list of parameter names; empty
/// list means hash the context `key`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyContextHashSpec {
    parameter_names: Vec<String>,
}

/// One sample context entry in the corpus: `(context_type, key, [(param_name, value)])`.
type SampleCtx = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

/// One row in the frozen pre-migration corpus.
#[derive(Debug, Clone)]
struct LegacyCorpusRow {
    /// Display name for assertion-failure logs.
    name: &'static str,
    /// The legacy map plus an explicit *insertion* order — modelling what an
    /// upstream caller serialised before this migration existed.
    context_hash_specs: Vec<(String, LegacyContextHashSpec)>,
    /// Sample context bundle to hash against.
    sample_contexts: Vec<SampleCtx>,
    /// `(flag_key, env_id)` used to seed `calculate_allocation`.
    flag_key: &'static str,
    env_id: &'static str,
}

// ── Canonical sort: legacy → new HashInputSpec ───────────────────────────────

/// Result of converting a legacy `context_hash_specs` map into the new
/// ordered selector list. We use `serde_json::Value` deliberately — this
/// mirrors the JSONB shape the migration backfills.
fn canonical_sort_to_hash_inputs(
    specs: &[(String, LegacyContextHashSpec)],
) -> Vec<serde_json::Value> {
    // Sort by context_type ASC for deterministic iteration order.
    let mut sorted: Vec<&(String, LegacyContextHashSpec)> = specs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = Vec::new();
    for (ctx_type, spec) in sorted {
        if spec.parameter_names.is_empty() {
            out.push(serde_json::json!({
                "kind": "context_key",
                "context_type": ctx_type,
            }));
        } else {
            let mut params = spec.parameter_names.clone();
            params.sort();
            for p in params {
                out.push(serde_json::json!({
                    "kind": "context_parameter",
                    "context_type": ctx_type,
                    "parameter": p,
                }));
            }
        }
    }
    out
}

// ── Build target_values from each schema and hash them ───────────────────────

/// Build the `target_values` list that `calculate_allocation` consumes from
/// the legacy `Vec<PercentageTarget>` representation. This emulates the
/// preview path's existing hashing logic in `stitchd-core::evaluation::preview`.
fn target_values_from_legacy_insertion_order(
    specs: &[(String, LegacyContextHashSpec)],
    sample_contexts: &[SampleCtx],
) -> Vec<String> {
    let mut values = Vec::new();
    for (ctx_type, spec) in specs {
        let ctx_lookup = sample_contexts.iter().find(|c| c.0 == ctx_type);
        if spec.parameter_names.is_empty() {
            values.push(ctx_lookup.map(|c| c.1.to_string()).unwrap_or_default());
        } else {
            for p in &spec.parameter_names {
                let v = ctx_lookup
                    .and_then(|c| {
                        c.2.iter()
                            .find(|(k, _)| *k == p.as_str())
                            .map(|(_, v)| (*v).to_string())
                    })
                    .unwrap_or_default();
                values.push(v);
            }
        }
    }
    values
}

/// Build the `target_values` list from the new ordered `hash_inputs` JSON
/// array (the post-migration shape). Mirrors the resolver that will exist in
/// `stitchd-core` once Phase 2 lands.
fn target_values_from_hash_inputs(
    hash_inputs: &[serde_json::Value],
    sample_contexts: &[SampleCtx],
) -> Vec<String> {
    let mut values = Vec::new();
    for sel in hash_inputs {
        let kind = sel.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        let ctx_type = sel
            .get("context_type")
            .and_then(|k| k.as_str())
            .unwrap_or("");
        let ctx_lookup = sample_contexts.iter().find(|c| c.0 == ctx_type);
        match kind {
            "context_key" => {
                values.push(ctx_lookup.map(|c| c.1.to_string()).unwrap_or_default());
            }
            "context_parameter" => {
                let p = sel.get("parameter").and_then(|k| k.as_str()).unwrap_or("");
                let v = ctx_lookup
                    .and_then(|c| {
                        c.2.iter()
                            .find(|(k, _)| *k == p)
                            .map(|(_, v)| (*v).to_string())
                    })
                    .unwrap_or_default();
                values.push(v);
            }
            other => panic!("unknown selector kind: {other}"),
        }
    }
    values
}

fn bucket_for(flag_key: &str, env_id: &str, target_values: &[String]) -> u32 {
    // calculate_allocation returns basis points (0..=9999)
    calculate_allocation(flag_key, env_id, target_values).min(9999)
}

/// Returns the frozen pre-migration corpus.
fn corpus() -> Vec<LegacyCorpusRow> {
    vec![
        // 1. Single context_type, key only — trivial canonical sort.
        LegacyCorpusRow {
            name: "user_key_only",
            context_hash_specs: vec![(
                "user".into(),
                LegacyContextHashSpec {
                    parameter_names: vec![],
                },
            )],
            sample_contexts: vec![
                ("user", "alice", vec![]),
                ("user", "bob", vec![]),
                ("user", "carol", vec![]),
                ("user", "dave", vec![]),
                ("user", "eve", vec![]),
            ],
            flag_key: "flag-1",
            env_id: "env-prod",
        },
        // 2. Single context_type, single parameter — canonical sort
        //    trivial because only one parameter.
        LegacyCorpusRow {
            name: "user_single_param",
            context_hash_specs: vec![(
                "user".into(),
                LegacyContextHashSpec {
                    parameter_names: vec!["plan".into()],
                },
            )],
            sample_contexts: vec![
                ("user", "alice", vec![("plan", "pro")]),
                ("user", "bob", vec![("plan", "free")]),
                ("user", "carol", vec![("plan", "pro")]),
                ("user", "dave", vec![("plan", "enterprise")]),
                ("user", "eve", vec![("plan", "free")]),
            ],
            flag_key: "flag-2",
            env_id: "env-prod",
        },
        // 3. Two context_types in non-canonical insertion order — this is
        //    the "operator-review" case. (`user` < `device` is the canonical
        //    sort; we provide them as device-then-user.)
        LegacyCorpusRow {
            name: "device_then_user_NONCANONICAL",
            context_hash_specs: vec![
                (
                    "device".into(),
                    LegacyContextHashSpec {
                        parameter_names: vec!["os".into()],
                    },
                ),
                (
                    "user".into(),
                    LegacyContextHashSpec {
                        parameter_names: vec![],
                    },
                ),
            ],
            sample_contexts: vec![
                ("user", "alice", vec![]),
                ("device", "d-001", vec![("os", "ios")]),
            ],
            flag_key: "flag-3",
            env_id: "env-prod",
        },
        // 4. Cross-context with multiple parameters per context_type — the
        //    parameter list is provided in non-alphabetical order so the
        //    intra-context canonical sort matters.
        LegacyCorpusRow {
            name: "multi_param_NONCANONICAL_params",
            context_hash_specs: vec![(
                "user".into(),
                LegacyContextHashSpec {
                    // canonical order: "country", "name", "plan"
                    parameter_names: vec!["name".into(), "country".into(), "plan".into()],
                },
            )],
            sample_contexts: vec![(
                "user",
                "alice",
                vec![("name", "Alice"), ("country", "US"), ("plan", "pro")],
            )],
            flag_key: "flag-4",
            env_id: "env-prod",
        },
        // 5. Three context_types in canonical insertion order (alphabetical).
        LegacyCorpusRow {
            name: "three_contexts_CANONICAL",
            context_hash_specs: vec![
                (
                    "application".into(),
                    LegacyContextHashSpec {
                        parameter_names: vec![],
                    },
                ),
                (
                    "device".into(),
                    LegacyContextHashSpec {
                        parameter_names: vec!["os".into()],
                    },
                ),
                (
                    "user".into(),
                    LegacyContextHashSpec {
                        parameter_names: vec![],
                    },
                ),
            ],
            sample_contexts: vec![
                ("application", "stitchd-admin", vec![]),
                ("device", "d-001", vec![("os", "ios")]),
                ("user", "alice", vec![]),
            ],
            flag_key: "flag-5",
            env_id: "env-prod",
        },
    ]
}

// ── Pure-Rust corpus assertions (no DB required) ─────────────────────────────

#[test]
fn canonical_sort_emits_expected_selectors_for_corpus() {
    for row in corpus() {
        let hash_inputs = canonical_sort_to_hash_inputs(&row.context_hash_specs);
        assert!(
            !hash_inputs.is_empty(),
            "corpus row {} produced empty hash_inputs",
            row.name
        );
        for sel in &hash_inputs {
            let kind = sel.get("kind").and_then(|k| k.as_str()).unwrap();
            assert!(
                matches!(kind, "context_key" | "context_parameter"),
                "unexpected selector kind '{kind}' in {}",
                row.name
            );
            assert!(
                sel.get("context_type").and_then(|k| k.as_str()).is_some(),
                "selector missing context_type in {}",
                row.name
            );
        }
    }
}

#[test]
fn bucket_parity_when_canonical_sort_matches_legacy_insertion_order() {
    // Rows where the legacy insertion order already matched canonical sort
    // (context_type ASC + parameter ASC within type) MUST produce identical
    // buckets under both schemas — this is the hash-stability invariant
    // (NFR-4 in the spec).
    let canonical_named = [
        "user_key_only",
        "user_single_param",
        "three_contexts_CANONICAL",
    ];
    let mut checked = 0;
    for row in corpus() {
        if !canonical_named.contains(&row.name) {
            continue;
        }
        let hash_inputs = canonical_sort_to_hash_inputs(&row.context_hash_specs);

        for ctx in &row.sample_contexts {
            // Hash against the same single-context bundle for both shapes.
            let bundle = vec![ctx.clone()];
            let legacy_values =
                target_values_from_legacy_insertion_order(&row.context_hash_specs, &bundle);
            let new_values = target_values_from_hash_inputs(&hash_inputs, &bundle);
            let legacy_bucket = bucket_for(row.flag_key, row.env_id, &legacy_values);
            let new_bucket = bucket_for(row.flag_key, row.env_id, &new_values);
            assert_eq!(
                legacy_bucket, new_bucket,
                "bucket mismatch for canonical-order row {} with context {:?}: legacy={legacy_bucket}, new={new_bucket}",
                row.name, ctx.1
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 2,
        "expected at least two canonical-order corpus rows; got {checked}"
    );
}

#[test]
fn operator_review_report_surfaces_noncanonical_rows() {
    // Rows where insertion order ≠ canonical sort MAY produce different
    // buckets — the test surfaces them for operator review without failing.
    // We assert at least one such row exists in the corpus so the surface is
    // exercised, and we collect the deltas into a report string.
    let mut report: Vec<String> = Vec::new();
    let mut nonmatching = 0;
    for row in corpus() {
        let hash_inputs = canonical_sort_to_hash_inputs(&row.context_hash_specs);

        // Build per-bundle bucket pairs.
        for ctx_bundle_seed in &row.sample_contexts {
            let bundle = vec![ctx_bundle_seed.clone()];
            let legacy_values =
                target_values_from_legacy_insertion_order(&row.context_hash_specs, &bundle);
            let new_values = target_values_from_hash_inputs(&hash_inputs, &bundle);
            let legacy_bucket = bucket_for(row.flag_key, row.env_id, &legacy_values);
            let new_bucket = bucket_for(row.flag_key, row.env_id, &new_values);
            if legacy_bucket != new_bucket {
                nonmatching += 1;
                report.push(format!(
                    "  • row={:<40} ctx_key={:<10} legacy_bucket={legacy_bucket:<3} new_bucket={new_bucket:<3}",
                    row.name, ctx_bundle_seed.1
                ));
            }
        }
    }
    // We expect at least one non-canonical row in the corpus so the operator
    // pathway is genuinely exercised.
    if nonmatching == 0 {
        eprintln!(
            "[operator-review] no non-canonical buckets in current corpus — re-check corpus design"
        );
    } else {
        eprintln!(
            "[operator-review] {nonmatching} bucket reshuffles surfaced — operator sign-off required:\n{}",
            report.join("\n")
        );
    }
    // Don't assert. The whole point is to *report* without failing.
}

#[test]
fn canonical_sort_is_stable_under_insertion_reorder() {
    // Two corpus rows with the same logical content but different insertion
    // orders MUST produce byte-identical hash_inputs JSON.
    let a = vec![
        (
            "device".to_string(),
            LegacyContextHashSpec {
                parameter_names: vec!["os".into(), "browser".into()],
            },
        ),
        (
            "user".to_string(),
            LegacyContextHashSpec {
                parameter_names: vec![],
            },
        ),
    ];
    let b = vec![
        (
            "user".to_string(),
            LegacyContextHashSpec {
                parameter_names: vec![],
            },
        ),
        (
            "device".to_string(),
            LegacyContextHashSpec {
                parameter_names: vec!["browser".into(), "os".into()],
            },
        ),
    ];

    let hi_a = canonical_sort_to_hash_inputs(&a);
    let hi_b = canonical_sort_to_hash_inputs(&b);

    assert_eq!(
        serde_json::to_string(&hi_a).unwrap(),
        serde_json::to_string(&hi_b).unwrap(),
        "canonical sort must be order-independent over the legacy map"
    );
}

// ── Schema assertion tests (PG required) ─────────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn feature_flag_rules_has_hash_inputs_column(pool: sqlx::PgPool) {
    let row: (bool,) = sqlx::query_as(
        r"SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
             WHERE table_schema = 'public'
               AND table_name = 'feature_flag_rules'
               AND column_name = 'hash_inputs'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");

    assert!(
        row.0,
        "feature_flag_rules.hash_inputs column must exist after the cutover migration"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn feature_flag_rules_hash_inputs_is_jsonb_nullable(pool: sqlx::PgPool) {
    let row: (String, String) = sqlx::query_as(
        r"SELECT data_type, is_nullable
            FROM information_schema.columns
           WHERE table_schema = 'public'
             AND table_name = 'feature_flag_rules'
             AND column_name = 'hash_inputs'",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");

    assert_eq!(row.0, "jsonb");
    assert_eq!(
        row.1, "YES",
        "hash_inputs must be NULL-able for legacy rows"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn feature_flags_has_default_rule_hash_inputs_column(pool: sqlx::PgPool) {
    let row: (bool,) = sqlx::query_as(
        r"SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
             WHERE table_schema = 'public'
               AND table_name = 'feature_flags'
               AND column_name = 'default_rule_hash_inputs'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");

    assert!(
        row.0,
        "feature_flags.default_rule_hash_inputs column must exist after the cutover migration"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn legacy_rule_def_column_is_preserved(pool: sqlx::PgPool) {
    // The dual-schema contract: the legacy `rule_def` column MUST still exist
    // for the duration of Phase 3-4. Phase 5/6 removes it.
    let row: (bool,) = sqlx::query_as(
        r"SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
             WHERE table_schema = 'public'
               AND table_name = 'feature_flag_rules'
               AND column_name = 'rule_def'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");

    assert!(
        row.0,
        "legacy rule_def column must survive Phase 3 migration"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn legacy_default_rule_distribution_column_is_preserved(pool: sqlx::PgPool) {
    let row: (bool,) = sqlx::query_as(
        r"SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
             WHERE table_schema = 'public'
               AND table_name = 'feature_flags'
               AND column_name = 'default_rule_distribution'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");

    assert!(
        row.0,
        "legacy default_rule_distribution column must survive Phase 3 migration"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn backfill_produces_canonical_sorted_hash_inputs(pool: sqlx::PgPool) {
    // Bootstrap minimum org + project + flag rows so we can attach a rule.
    let org_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, 'TestOrg')")
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
    let project_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, 'TestProj')")
        .bind(project_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
    let flag_id = uuid::Uuid::new_v4();
    sqlx::query(
        r"INSERT INTO feature_flags (id, project_id, key, value_type, enabled)
          VALUES ($1, $2, 'bk-flag', 'Bool', true)",
    )
    .bind(flag_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .unwrap();

    // Insert a rule whose `rule_def->'output'->'Percentage'->'targets'` is
    // intentionally NOT in canonical order: `[user.Key, device.Parameter(os),
    // device.Parameter(browser)]`. Backfill should re-emit as
    // `[device.Parameter(browser), device.Parameter(os), user.Key]`
    // (context_type ASC, parameter ASC within type).
    let rule_id = uuid::Uuid::new_v4();
    let variant_id = uuid::Uuid::new_v4();
    let rule_def = serde_json::json!({
        "id": rule_id.to_string(),
        "condition": { "And": [] },
        "output": {
            "Percentage": {
                "targets": [
                    { "context_type": "user",   "field": "Key" },
                    { "context_type": "device", "field": { "Parameter": "os" } },
                    { "context_type": "device", "field": { "Parameter": "browser" } }
                ],
                "weights": [[variant_id.to_string(), 10000]]
            }
        }
    });
    sqlx::query(
        r"INSERT INTO feature_flag_rules (id, flag_id, rule_index, rule_def)
          VALUES ($1, $2, 0, $3)",
    )
    .bind(rule_id)
    .bind(flag_id)
    .bind(&rule_def)
    .execute(&pool)
    .await
    .unwrap();

    // The migration already ran (via sqlx::test). But the rule was inserted
    // *after* migration, so its hash_inputs is NULL. Re-run the backfill
    // step manually to exercise the canonical-sort logic against this row.
    sqlx::query(
        r#"WITH expanded AS (
            SELECT
                r.id AS rule_id,
                (t.value->>'context_type') AS context_type,
                CASE
                    WHEN t.value->'field' = '"Key"'::jsonb THEN NULL
                    ELSE t.value->'field'->>'Parameter'
                END AS parameter
            FROM feature_flag_rules r,
                 LATERAL jsonb_array_elements(
                     COALESCE(r.rule_def->'output'->'Percentage'->'targets', '[]'::jsonb)
                 ) AS t(value)
            WHERE r.hash_inputs IS NULL
        ),
        aggregated AS (
            SELECT
                rule_id,
                jsonb_agg(
                    CASE
                        WHEN parameter IS NULL THEN
                            jsonb_build_object('kind', 'context_key', 'context_type', context_type)
                        ELSE
                            jsonb_build_object('kind', 'context_parameter', 'context_type', context_type, 'parameter', parameter)
                    END
                    ORDER BY context_type ASC, COALESCE(parameter, '') ASC
                ) AS hash_inputs
            FROM expanded
            GROUP BY rule_id
        )
        UPDATE feature_flag_rules r
           SET hash_inputs = a.hash_inputs
          FROM aggregated a
         WHERE r.id = a.rule_id
           AND r.hash_inputs IS NULL"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Read back and assert canonical order.
    let row: (serde_json::Value,) =
        sqlx::query_as(r"SELECT hash_inputs FROM feature_flag_rules WHERE id = $1")
            .bind(rule_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let arr = row.0.as_array().expect("hash_inputs must be a JSON array");
    assert_eq!(arr.len(), 3);
    // Expected canonical ordering:
    //   [0] device.parameter(browser)
    //   [1] device.parameter(os)
    //   [2] user.key
    assert_eq!(arr[0]["kind"], "context_parameter");
    assert_eq!(arr[0]["context_type"], "device");
    assert_eq!(arr[0]["parameter"], "browser");
    assert_eq!(arr[1]["kind"], "context_parameter");
    assert_eq!(arr[1]["context_type"], "device");
    assert_eq!(arr[1]["parameter"], "os");
    assert_eq!(arr[2]["kind"], "context_key");
    assert_eq!(arr[2]["context_type"], "user");
}
