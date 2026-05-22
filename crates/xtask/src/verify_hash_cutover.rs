//! `cargo xtask verify-hash-cutover`
//!
//! Phase 3 task 3.5 of `flag_eval_unify_20260522`. Reads a frozen corpus
//! mirroring the schema-test fixtures, converts each legacy
//! `context_hash_specs` map into the canonical-sorted new `hash_inputs` list,
//! re-hashes both shapes against the bundled sample contexts using
//! `stitchd_core::hashing::calculate_allocation`, and prints a summary
//! report.
//!
//! Bucket-identical pairs are the steady-state expectation; pairs where the
//! buckets differ are flagged for operator review (those rules need explicit
//! sign-off before the legacy `context_hash_specs` field is retired in Phase
//! 5/6).
//!
//! Exit code is always 0 — this is a *report* binary, not a gate. The track's
//! NFR-4 (hash-stability invariant) is enforced by the migration test, not
//! this binary.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use stitchd_core::hashing::calculate_allocation;

/// Top-level entry — invoked by `main.rs` when `cargo xtask verify-hash-cutover`
/// is the requested task.
pub fn run() -> Result<()> {
    let fixture_path = locate_fixture()?;
    println!("Reading cutover corpus → {}", fixture_path.display());

    let src = std::fs::read_to_string(&fixture_path)
        .with_context(|| format!("failed to read {}", fixture_path.display()))?;
    let corpus: Corpus = serde_json::from_str(&src)
        .with_context(|| format!("failed to parse {}", fixture_path.display()))?;

    let mut report = Report::default();

    for pair in &corpus.pairs {
        let new_hash_inputs = canonical_sort(&pair.legacy_context_hash_specs, &pair.legacy_insertion_order);
        if new_hash_inputs != pair.expected_new_hash_inputs {
            report.fixture_drift.push(pair.name.clone());
            eprintln!(
                "  [FIXTURE-DRIFT] {} — canonical sort emitted {} selectors but the corpus expected {}",
                pair.name,
                new_hash_inputs.len(),
                pair.expected_new_hash_inputs.len()
            );
            continue;
        }

        for ctx in &pair.sample_contexts {
            let legacy_values = target_values_legacy(
                &pair.legacy_context_hash_specs,
                &pair.legacy_insertion_order,
                ctx,
            );
            let new_values = target_values_new(&new_hash_inputs, ctx);

            let legacy_bucket = bucket_for(&corpus.flag_key, &corpus.env_id, &legacy_values);
            let new_bucket = bucket_for(&corpus.flag_key, &corpus.env_id, &new_values);

            if legacy_bucket == new_bucket {
                report.identical += 1;
            } else {
                report.operator_review.push(OperatorReviewRow {
                    pair_name: pair.name.clone(),
                    context_key: ctx.key.clone(),
                    legacy_bucket,
                    new_bucket,
                });
            }
        }
    }

    print_summary(&report);
    Ok(())
}

// ── Fixture types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Corpus {
    flag_key: String,
    env_id: String,
    pairs: Vec<CutoverPair>,
}

#[derive(Debug, Deserialize)]
struct CutoverPair {
    name: String,
    /// `context_type` → `{ parameter_names: [...] }`. JSON object; iteration
    /// order is non-deterministic, hence we also accept an explicit
    /// `legacy_insertion_order` field to reproduce the pre-migration ordering.
    legacy_context_hash_specs: BTreeMap<String, LegacyContextHashSpec>,
    /// Explicit `context_type` ordering as it would have been recorded by
    /// upstream code paths before the migration existed.
    legacy_insertion_order: Vec<String>,
    expected_new_hash_inputs: Vec<HashSelectorJson>,
    sample_contexts: Vec<SampleContext>,
}

#[derive(Debug, Deserialize)]
struct LegacyContextHashSpec {
    parameter_names: Vec<String>,
}

#[derive(Debug, PartialEq, Deserialize, Clone)]
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

#[derive(Debug, Deserialize)]
struct SampleContext {
    context_type: String,
    key: String,
    parameters: BTreeMap<String, String>,
}

// ── Canonical sort ───────────────────────────────────────────────────────────

fn canonical_sort(
    specs: &BTreeMap<String, LegacyContextHashSpec>,
    _insertion_order: &[String],
) -> Vec<HashSelectorJson> {
    // BTreeMap already iterates in context_type ASC, which is the canonical
    // sort. The explicit `_insertion_order` is only used by the
    // *legacy-shape* hashing path below — the migration's canonical sort
    // ignores it and re-emits the ordering deterministically.
    let mut out = Vec::new();
    for (ctx_type, spec) in specs {
        if spec.parameter_names.is_empty() {
            out.push(HashSelectorJson::ContextKey {
                context_type: ctx_type.clone(),
            });
        } else {
            let mut params = spec.parameter_names.clone();
            params.sort();
            for p in params {
                out.push(HashSelectorJson::ContextParameter {
                    context_type: ctx_type.clone(),
                    parameter: p,
                });
            }
        }
    }
    out
}

// ── Per-shape target_values builders ─────────────────────────────────────────

fn target_values_legacy(
    specs: &BTreeMap<String, LegacyContextHashSpec>,
    insertion_order: &[String],
    ctx: &SampleContext,
) -> Vec<String> {
    let mut out = Vec::new();
    for ct in insertion_order {
        let Some(spec) = specs.get(ct) else {
            continue;
        };
        if spec.parameter_names.is_empty() {
            // Hash the context's key — only contributes if the sample's
            // context_type matches.
            if ctx.context_type == *ct {
                out.push(ctx.key.clone());
            } else {
                out.push(String::new());
            }
        } else {
            for p in &spec.parameter_names {
                if ctx.context_type == *ct {
                    out.push(ctx.parameters.get(p).cloned().unwrap_or_default());
                } else {
                    out.push(String::new());
                }
            }
        }
    }
    out
}

fn target_values_new(hash_inputs: &[HashSelectorJson], ctx: &SampleContext) -> Vec<String> {
    let mut out = Vec::new();
    for sel in hash_inputs {
        match sel {
            HashSelectorJson::ContextKey { context_type } => {
                if ctx.context_type == *context_type {
                    out.push(ctx.key.clone());
                } else {
                    out.push(String::new());
                }
            }
            HashSelectorJson::ContextParameter {
                context_type,
                parameter,
            } => {
                if ctx.context_type == *context_type {
                    out.push(ctx.parameters.get(parameter).cloned().unwrap_or_default());
                } else {
                    out.push(String::new());
                }
            }
        }
    }
    out
}

fn bucket_for(flag_key: &str, env_id: &str, target_values: &[String]) -> u32 {
    let percentage = calculate_allocation(flag_key, env_id, target_values);
    ((percentage * 10.0).floor() as u32).min(999)
}

// ── Report ───────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Report {
    identical: usize,
    operator_review: Vec<OperatorReviewRow>,
    fixture_drift: Vec<String>,
}

struct OperatorReviewRow {
    pair_name: String,
    context_key: String,
    legacy_bucket: u32,
    new_bucket: u32,
}

fn print_summary(report: &Report) {
    println!();
    println!("─── verify-hash-cutover summary ───");
    println!("  identical buckets:        {}", report.identical);
    println!("  operator-review entries:  {}", report.operator_review.len());
    println!("  fixture-drift pairs:      {}", report.fixture_drift.len());

    if !report.operator_review.is_empty() {
        println!();
        println!("Operator-review rows (legacy ≠ new bucket; needs sign-off):");
        for row in &report.operator_review {
            println!(
                "  • pair={:<35} ctx_key={:<12} legacy_bucket={:<3} new_bucket={:<3}",
                row.pair_name, row.context_key, row.legacy_bucket, row.new_bucket
            );
        }
    }

    if !report.fixture_drift.is_empty() {
        println!();
        println!("Fixture-drift pairs (corpus `expected_new_hash_inputs` no longer matches the canonical-sort emitter):");
        for name in &report.fixture_drift {
            println!("  • {name}");
        }
        println!("  → re-generate the fixture or update the corpus by hand.");
    }
}

// ── Locating the fixture file ────────────────────────────────────────────────

fn locate_fixture() -> Result<PathBuf> {
    // 1) explicit arg override: `cargo xtask verify-hash-cutover <path>`.
    if let Some(arg) = std::env::args().nth(2) {
        return Ok(PathBuf::from(arg));
    }
    // 2) default: workspace-relative `crates/xtask/fixtures/...`.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set — invoke via `cargo xtask`")?;
    Ok(PathBuf::from(manifest_dir).join("fixtures/hash_cutover_corpus.json"))
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_sort_emits_btreemap_order() {
        let mut specs = BTreeMap::new();
        specs.insert(
            "device".into(),
            LegacyContextHashSpec {
                parameter_names: vec!["os".into(), "browser".into()],
            },
        );
        specs.insert(
            "user".into(),
            LegacyContextHashSpec {
                parameter_names: vec![],
            },
        );

        let out = canonical_sort(&specs, &["device".into(), "user".into()]);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], HashSelectorJson::ContextParameter { ref context_type, ref parameter } if context_type == "device" && parameter == "browser"));
        assert!(matches!(out[1], HashSelectorJson::ContextParameter { ref context_type, ref parameter } if context_type == "device" && parameter == "os"));
        assert!(matches!(out[2], HashSelectorJson::ContextKey { ref context_type } if context_type == "user"));
    }

    #[test]
    fn target_values_legacy_uses_insertion_order() {
        let mut specs = BTreeMap::new();
        specs.insert(
            "user".into(),
            LegacyContextHashSpec {
                parameter_names: vec![],
            },
        );
        specs.insert(
            "device".into(),
            LegacyContextHashSpec {
                parameter_names: vec!["os".into()],
            },
        );

        // Insertion order: device-first, user-second.
        let order = vec!["device".to_string(), "user".to_string()];
        let ctx = SampleContext {
            context_type: "user".into(),
            key: "alice".into(),
            parameters: BTreeMap::new(),
        };

        let values = target_values_legacy(&specs, &order, &ctx);
        // First selector = device.os — ctx is user, so empty.
        // Second selector = user.key — ctx is user, so "alice".
        assert_eq!(values, vec!["".to_string(), "alice".to_string()]);
    }

    #[test]
    fn target_values_new_uses_canonical_order() {
        let new_inputs = vec![
            HashSelectorJson::ContextParameter {
                context_type: "device".into(),
                parameter: "os".into(),
            },
            HashSelectorJson::ContextKey {
                context_type: "user".into(),
            },
        ];
        let ctx = SampleContext {
            context_type: "user".into(),
            key: "alice".into(),
            parameters: BTreeMap::new(),
        };

        let values = target_values_new(&new_inputs, &ctx);
        assert_eq!(values, vec!["".to_string(), "alice".to_string()]);
    }
}
