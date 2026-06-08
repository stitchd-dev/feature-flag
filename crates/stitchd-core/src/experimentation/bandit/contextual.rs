//! Pure contextual-bandit linear reward model.
//!
//! A contextual bandit conditions each variant's predicted reward on the
//! evaluation context's *feature values*. This module holds:
//!
//! * the snapshot-resident model shape ([`FeatureSpec`], [`FeatureEncoding`],
//!   [`VariantCoefficients`], [`ContextualModel`]),
//! * the pure feature-vector encoder ([`encode_features`]),
//! * the pure predictor ([`predict`]) — a dot product of coefficients with the
//!   encoded feature vector (intercept-first),
//! * the pure per-context sampler ([`sample_contextual_variant`]) — a
//!   Thompson-on-linear draw (predicted reward + an exploration perturbation
//!   scaled by the LinUCB variance term, drawn from the SAME seeded LCG the rest
//!   of the stats core uses) picking the goal-directed argmax, and
//! * the closed-form ridge fitter ([`fit_ridge`]) the stats-service uses to fit
//!   one coefficient vector per variant from `(feature_vector, reward)` rows.
//!
//! ## Purity
//!
//! Every function here is pure synchronous math: no async, no I/O, no clock
//! reads, no logging. [`sample_contextual_variant`] reuses the shared
//! [`Lcg`](crate::experimentation::stats::bayesian) + standard-normal sampler so
//! the evaluator can call it without violating the `evaluation` purity contract.
//!
//! ## Determinism
//!
//! The draw is fully determined by `(model, feature_vector, seed)`. The evaluator
//! derives `seed` the same way the non-contextual realtime path does
//! ([`context_seed`](super::realtime::context_seed)) so a context resolves
//! identically in preview and in the SDK.
//!
//! ## Privacy
//!
//! Feature *values* never leave this module — only the model coefficients and
//! the encoded numeric vector circulate, and the evaluator surfaces feature
//! *names* only. No `privateParameters` value is ever traced.

use crate::experimentation::stats::bayesian::{Lcg, sample_standard_normal};
use crate::rule_engine::types::BanditGoal;
use serde::{Deserialize, Serialize};

/// How a single context feature is turned into one-or-more numeric slots in the
/// design vector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FeatureEncoding {
    /// Pass the parameter through as a single numeric slot. A non-numeric /
    /// missing value encodes to `0.0`.
    Numeric,
    /// One-hot encode a categorical parameter into one slot per declared
    /// category (in declaration order). A value matching `categories[i]` sets
    /// slot `i` to `1.0`; an unknown value (or a missing parameter) leaves every
    /// category slot `0.0` (the implicit "other" baseline).
    OneHot {
        /// The categories to encode, in declaration order. One slot each.
        categories: Vec<String>,
    },
}

impl FeatureEncoding {
    /// The number of design-vector slots this encoding contributes.
    #[must_use]
    pub fn width(&self) -> usize {
        match self {
            FeatureEncoding::Numeric => 1,
            FeatureEncoding::OneHot { categories } => categories.len(),
        }
    }
}

/// One context feature the reward model conditions on: which context type +
/// parameter to read, and how to encode it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FeatureSpec {
    /// The context type whose parameter is read (e.g. `"user"`).
    pub context_type: String,
    /// The parameter name within that context (e.g. `"plan"`).
    pub parameter: String,
    /// How the parameter value is encoded into the design vector.
    pub encoding: FeatureEncoding,
}

/// The fitted linear reward coefficients for one variant.
///
/// `coeffs[0]` is the intercept; `coeffs[1..]` align slot-for-slot with the
/// concatenated [`FeatureSpec`] encodings (in `ContextualModel.features` order).
/// `a_inv` is an OPTIONAL flattened `d×d` row-major inverse design matrix
/// (`(XᵀX + λI)⁻¹`) carrying the LinUCB / posterior covariance; when present it
/// drives the per-context exploration perturbation, when absent the sampler
/// falls back to a small fixed exploration variance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VariantCoefficients {
    /// The variant key these coefficients predict reward for.
    pub variant_key: String,
    /// Intercept-first coefficient vector (`len == 1 + Σ feature widths`).
    pub coeffs: Vec<f64>,
    /// Optional flattened `d×d` row-major `(XᵀX + λI)⁻¹` for the LinUCB
    /// exploration term (`d == coeffs.len()`). Absent → fixed exploration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_inv: Option<Vec<f64>>,
}

/// A snapshot-resident contextual reward model: a shared feature set + one
/// coefficient vector per assignable variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ContextualModel {
    /// The features every variant's coefficients are defined over, in order.
    pub features: Vec<FeatureSpec>,
    /// Per-variant coefficient vectors.
    pub variants: Vec<VariantCoefficients>,
}

impl ContextualModel {
    /// The design-vector dimension: `1` (intercept) + the sum of feature widths.
    #[must_use]
    pub fn dim(&self) -> usize {
        1 + self
            .features
            .iter()
            .map(|f| f.encoding.width())
            .sum::<usize>()
    }
}

/// A resolved feature value for the encoder: a `(context_type, parameter)` lookup
/// result. `None` is a missing parameter (encodes to the baseline / `0.0`).
///
/// Kept as a borrowed-string closure input so the evaluator can resolve from its
/// in-memory context map without allocating, and the fitter can resolve from
/// event properties.
pub trait FeatureResolver {
    /// Resolve the raw string value of `(context_type, parameter)`, if present.
    fn resolve(&self, context_type: &str, parameter: &str) -> Option<&str>;
}

/// Encode a context (via a [`FeatureResolver`]) into the intercept-first design
/// vector for `features`. Pure.
///
/// Layout: `[1.0, <feat0 slots>, <feat1 slots>, ...]`. A `Numeric` feature
/// contributes the parsed `f64` (missing / unparsable → `0.0`); a `OneHot`
/// feature contributes one `1.0` in the matching category slot (unknown /
/// missing → all `0.0`).
#[must_use]
pub fn encode_features(features: &[FeatureSpec], resolver: &dyn FeatureResolver) -> Vec<f64> {
    let mut v = Vec::with_capacity(1 + features.iter().map(|f| f.encoding.width()).sum::<usize>());
    v.push(1.0); // intercept
    for spec in features {
        let raw = resolver.resolve(&spec.context_type, &spec.parameter);
        match &spec.encoding {
            FeatureEncoding::Numeric => {
                let x = raw.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                v.push(x);
            }
            FeatureEncoding::OneHot { categories } => {
                for cat in categories {
                    let hit = raw.is_some_and(|s| s == cat.as_str());
                    v.push(if hit { 1.0 } else { 0.0 });
                }
            }
        }
    }
    v
}

/// Predict a variant's reward as the dot product of its coefficients with the
/// encoded feature vector. The intercept rides in `coeffs[0]` × `feature[0]`
/// (which [`encode_features`] sets to `1.0`). Length mismatch is tolerated by
/// truncating to the shorter length (degenerate but safe). Pure.
#[must_use]
pub fn predict(coeffs: &[f64], feature_vector: &[f64]) -> f64 {
    coeffs
        .iter()
        .zip(feature_vector.iter())
        .map(|(c, x)| c * x)
        .sum()
}

/// The LinUCB exploration standard deviation for one variant at `feature_vector`:
/// `sqrt(xᵀ A⁻¹ x)` when `a_inv` is present and yields a non-negative quadratic
/// form, else a small fixed floor so exploration never fully collapses. Pure.
fn exploration_sd(coeffs: &VariantCoefficients, feature_vector: &[f64]) -> f64 {
    const FIXED_SD: f64 = 0.1;
    let d = feature_vector.len();
    let Some(a_inv) = coeffs.a_inv.as_ref() else {
        return FIXED_SD;
    };
    if a_inv.len() != d * d {
        return FIXED_SD;
    }
    // quad = xᵀ A⁻¹ x
    let mut quad = 0.0;
    for i in 0..d {
        let mut row = 0.0;
        for j in 0..d {
            row += a_inv[i * d + j] * feature_vector[j];
        }
        quad += feature_vector[i] * row;
    }
    if quad > 0.0 { quad.sqrt() } else { FIXED_SD }
}

/// Draw a per-context variant from a contextual model: for each variant predict
/// reward, add a Thompson exploration perturbation `sd · Z` (`Z` standard normal
/// from the seeded LCG; `sd` is the LinUCB term or a fixed floor), then pick the
/// goal-directed argmax (Increase) / argmin (Decrease). First wins ties.
///
/// Returns `None` when the model has no variants. Deterministic in
/// `(model, feature_vector, seed)`: variants are perturbed in declaration order
/// from one shared LCG stream. Pure: no I/O, no clock.
#[must_use]
pub fn sample_contextual_variant(
    model: &ContextualModel,
    feature_vector: &[f64],
    goal: BanditGoal,
    seed: u64,
) -> Option<String> {
    if model.variants.is_empty() {
        return None;
    }
    let mut rng = Lcg::new(seed);
    let mut best_idx = 0usize;
    let mut best_score = f64::NAN;
    for (i, vc) in model.variants.iter().enumerate() {
        let mean = predict(&vc.coeffs, feature_vector);
        let sd = exploration_sd(vc, feature_vector);
        let score = mean + sd * sample_standard_normal(&mut rng);
        if i == 0 {
            best_score = score;
            best_idx = 0;
            continue;
        }
        let better = match goal {
            BanditGoal::Increase => score > best_score,
            BanditGoal::Decrease => score < best_score,
        };
        if better {
            best_score = score;
            best_idx = i;
        }
    }
    Some(model.variants[best_idx].variant_key.clone())
}

// ── Ridge regression (closed-form normal equations) ──────────────────────────

/// Closed-form ridge regression: solve `(XᵀX + λI) β = Xᵀy` for `β`, returning
/// the coefficient vector. `rows` are `(feature_vector, reward)` design rows; all
/// feature vectors must share length `d` (the first row's length sets `d`;
/// shorter/longer rows are skipped). The intercept is expected to ride as a
/// constant `1.0` slot in each feature vector (as [`encode_features`] produces),
/// so it is regularised like any other coefficient — callers wanting an
/// unpenalised intercept can use `lambda = 0` or accept the (small) shrinkage.
///
/// Returns a zero vector of length `d` when `rows` is empty. Pure linear algebra:
/// a hand-rolled Gauss-Jordan solve over small fixed dims (no new deps).
#[must_use]
pub fn fit_ridge(rows: &[(Vec<f64>, f64)], lambda: f64) -> Vec<f64> {
    let Some(d) = rows.first().map(|(x, _)| x.len()) else {
        return Vec::new();
    };
    if d == 0 {
        return Vec::new();
    }
    // Accumulate A = XᵀX + λI and b = Xᵀy.
    let mut a = vec![0.0f64; d * d];
    let mut b = vec![0.0f64; d];
    for (x, y) in rows {
        if x.len() != d {
            continue;
        }
        for i in 0..d {
            b[i] += x[i] * y;
            for j in 0..d {
                a[i * d + j] += x[i] * x[j];
            }
        }
    }
    for i in 0..d {
        a[i * d + i] += lambda;
    }
    solve_linear_system(&mut a, &mut b, d).unwrap_or_else(|| vec![0.0; d])
}

/// Compute the inverse of the ridge design matrix `A = XᵀX + λI` as a flattened
/// row-major `d×d` vector (the LinUCB `A⁻¹`), or `None` if singular. Pure.
#[must_use]
pub fn fit_design_inverse(rows: &[(Vec<f64>, f64)], lambda: f64) -> Option<Vec<f64>> {
    let d = rows.first().map(|(x, _)| x.len())?;
    if d == 0 {
        return None;
    }
    let mut a = vec![0.0f64; d * d];
    for (x, _) in rows {
        if x.len() != d {
            continue;
        }
        for i in 0..d {
            for j in 0..d {
                a[i * d + j] += x[i] * x[j];
            }
        }
    }
    for i in 0..d {
        a[i * d + i] += lambda;
    }
    invert_matrix(&a, d)
}

/// Gauss-Jordan solve of `A x = b` for `x` (in place on `a`/`b`). Returns the
/// solution, or `None` if a pivot is ~0 (singular). Pure.
fn solve_linear_system(a: &mut [f64], b: &mut [f64], d: usize) -> Option<Vec<f64>> {
    for col in 0..d {
        // Partial pivot: find the row with the largest |a[row][col]|.
        let mut pivot = col;
        let mut max = a[col * d + col].abs();
        for row in (col + 1)..d {
            let v = a[row * d + col].abs();
            if v > max {
                max = v;
                pivot = row;
            }
        }
        if max < 1e-12 {
            return None;
        }
        if pivot != col {
            for k in 0..d {
                a.swap(col * d + k, pivot * d + k);
            }
            b.swap(col, pivot);
        }
        // Normalise pivot row.
        let p = a[col * d + col];
        for k in 0..d {
            a[col * d + k] /= p;
        }
        b[col] /= p;
        // Eliminate other rows.
        for row in 0..d {
            if row == col {
                continue;
            }
            let factor = a[row * d + col];
            if factor == 0.0 {
                continue;
            }
            for k in 0..d {
                a[row * d + k] -= factor * a[col * d + k];
            }
            b[row] -= factor * b[col];
        }
    }
    Some(b.to_vec())
}

/// Invert a `d×d` row-major matrix via Gauss-Jordan, returning the flattened
/// inverse, or `None` if singular. Pure.
fn invert_matrix(src: &[f64], d: usize) -> Option<Vec<f64>> {
    let mut a = src.to_vec();
    // Identity that becomes the inverse.
    let mut inv = vec![0.0f64; d * d];
    for i in 0..d {
        inv[i * d + i] = 1.0;
    }
    for col in 0..d {
        let mut pivot = col;
        let mut max = a[col * d + col].abs();
        for row in (col + 1)..d {
            let v = a[row * d + col].abs();
            if v > max {
                max = v;
                pivot = row;
            }
        }
        if max < 1e-12 {
            return None;
        }
        if pivot != col {
            for k in 0..d {
                a.swap(col * d + k, pivot * d + k);
                inv.swap(col * d + k, pivot * d + k);
            }
        }
        let p = a[col * d + col];
        for k in 0..d {
            a[col * d + k] /= p;
            inv[col * d + k] /= p;
        }
        for row in 0..d {
            if row == col {
                continue;
            }
            let factor = a[row * d + col];
            if factor == 0.0 {
                continue;
            }
            for k in 0..d {
                a[row * d + k] -= factor * a[col * d + k];
                inv[row * d + k] -= factor * inv[col * d + k];
            }
        }
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A simple map-backed resolver: keys are `"{context_type}.{parameter}"`.
    struct MapResolver(HashMap<String, String>);
    impl FeatureResolver for MapResolver {
        fn resolve(&self, ct: &str, p: &str) -> Option<&str> {
            self.0.get(&format!("{ct}.{p}")).map(String::as_str)
        }
    }
    fn resolver(pairs: &[(&str, &str)]) -> MapResolver {
        MapResolver(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        )
    }

    fn numeric_feat(ct: &str, p: &str) -> FeatureSpec {
        FeatureSpec {
            context_type: ct.into(),
            parameter: p.into(),
            encoding: FeatureEncoding::Numeric,
        }
    }
    fn onehot_feat(ct: &str, p: &str, cats: &[&str]) -> FeatureSpec {
        FeatureSpec {
            context_type: ct.into(),
            parameter: p.into(),
            encoding: FeatureEncoding::OneHot {
                categories: cats.iter().map(|c| (*c).to_string()).collect(),
            },
        }
    }

    // ── encode_features ──────────────────────────────────────────────────────

    #[test]
    fn encode_intercept_first_numeric() {
        let feats = vec![numeric_feat("user", "age")];
        let r = resolver(&[("user.age", "42")]);
        assert_eq!(encode_features(&feats, &r), vec![1.0, 42.0]);
    }

    #[test]
    fn encode_missing_numeric_is_zero() {
        let feats = vec![numeric_feat("user", "age")];
        let r = resolver(&[]);
        assert_eq!(encode_features(&feats, &r), vec![1.0, 0.0]);
    }

    #[test]
    fn encode_unparsable_numeric_is_zero() {
        let feats = vec![numeric_feat("user", "age")];
        let r = resolver(&[("user.age", "not-a-number")]);
        assert_eq!(encode_features(&feats, &r), vec![1.0, 0.0]);
    }

    #[test]
    fn encode_one_hot_sets_matching_slot() {
        let feats = vec![onehot_feat("user", "plan", &["free", "pro", "enterprise"])];
        let r = resolver(&[("user.plan", "pro")]);
        assert_eq!(encode_features(&feats, &r), vec![1.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn encode_one_hot_unknown_is_all_zero_baseline() {
        let feats = vec![onehot_feat("user", "plan", &["free", "pro"])];
        let r = resolver(&[("user.plan", "mystery")]);
        assert_eq!(encode_features(&feats, &r), vec![1.0, 0.0, 0.0]);
    }

    #[test]
    fn encode_mixed_features_in_order() {
        let feats = vec![
            numeric_feat("user", "age"),
            onehot_feat("user", "plan", &["free", "pro"]),
        ];
        let r = resolver(&[("user.age", "30"), ("user.plan", "free")]);
        assert_eq!(encode_features(&feats, &r), vec![1.0, 30.0, 1.0, 0.0]);
    }

    // ── predict ──────────────────────────────────────────────────────────────

    #[test]
    fn predict_is_dot_product_with_intercept() {
        // coeffs = [intercept=2, age=0.5], x = [1, 10] → 2 + 5 = 7
        assert_eq!(predict(&[2.0, 0.5], &[1.0, 10.0]), 7.0);
    }

    // ── fit_ridge golden vectors ─────────────────────────────────────────────

    #[test]
    fn fit_ridge_recovers_known_line_no_reg() {
        // y = 3 + 2x exactly; with lambda≈0 ridge ≈ OLS recovers [3, 2].
        let rows: Vec<(Vec<f64>, f64)> = (0..20)
            .map(|i| {
                let x = i as f64;
                (vec![1.0, x], 3.0 + 2.0 * x)
            })
            .collect();
        let beta = fit_ridge(&rows, 1e-9);
        assert!((beta[0] - 3.0).abs() < 1e-4, "intercept {}", beta[0]);
        assert!((beta[1] - 2.0).abs() < 1e-4, "slope {}", beta[1]);
    }

    #[test]
    fn fit_ridge_two_features_known_plane() {
        // y = 1 + 2*x1 - 3*x2
        let mut rows = Vec::new();
        for x1 in 0..5 {
            for x2 in 0..5 {
                let (a, b) = (x1 as f64, x2 as f64);
                rows.push((vec![1.0, a, b], 1.0 + 2.0 * a - 3.0 * b));
            }
        }
        let beta = fit_ridge(&rows, 1e-9);
        assert!((beta[0] - 1.0).abs() < 1e-3, "{beta:?}");
        assert!((beta[1] - 2.0).abs() < 1e-3, "{beta:?}");
        assert!((beta[2] - (-3.0)).abs() < 1e-3, "{beta:?}");
    }

    #[test]
    fn fit_ridge_empty_is_empty() {
        assert!(fit_ridge(&[], 1.0).is_empty());
    }

    #[test]
    fn fit_ridge_regularization_shrinks_toward_zero() {
        // Single feature, strong signal; heavy lambda shrinks the slope.
        let rows: Vec<(Vec<f64>, f64)> = (0..10)
            .map(|i| (vec![1.0, i as f64], 5.0 * i as f64))
            .collect();
        let light = fit_ridge(&rows, 1e-6);
        let heavy = fit_ridge(&rows, 1000.0);
        assert!(
            heavy[1].abs() < light[1].abs(),
            "heavy reg should shrink slope: light={} heavy={}",
            light[1],
            heavy[1]
        );
    }

    // ── sample_contextual_variant ────────────────────────────────────────────

    fn model_two_variants() -> ContextualModel {
        ContextualModel {
            features: vec![numeric_feat("user", "x")],
            variants: vec![
                // variant "a": reward = 0 + 1*x
                VariantCoefficients {
                    variant_key: "a".into(),
                    coeffs: vec![0.0, 1.0],
                    a_inv: None,
                },
                // variant "b": reward = 0 - 1*x
                VariantCoefficients {
                    variant_key: "b".into(),
                    coeffs: vec![0.0, -1.0],
                    a_inv: None,
                },
            ],
        }
    }

    #[test]
    fn sample_is_deterministic_in_seed() {
        let m = model_two_variants();
        let x = vec![1.0, 5.0];
        let r1 = sample_contextual_variant(&m, &x, BanditGoal::Increase, 99);
        let r2 = sample_contextual_variant(&m, &x, BanditGoal::Increase, 99);
        assert_eq!(r1, r2);
        assert!(r1.is_some());
    }

    #[test]
    fn sample_empty_model_is_none() {
        let m = ContextualModel {
            features: vec![],
            variants: vec![],
        };
        assert!(sample_contextual_variant(&m, &[1.0], BanditGoal::Increase, 1).is_none());
    }

    #[test]
    fn better_context_feature_wins_for_increase() {
        // For x>0, variant "a" (reward = +x) dominates "b" (reward = -x); a
        // large positive x should make "a" win the vast majority of contexts.
        let m = model_two_variants();
        let x = vec![1.0, 10.0];
        let mut a = 0;
        let n = 2000;
        for seed in 0..n {
            if sample_contextual_variant(&m, &x, BanditGoal::Increase, seed).as_deref() == Some("a")
            {
                a += 1;
            }
        }
        assert!(a as f64 / n as f64 > 0.95, "a should dominate, got {a}/{n}");
    }

    #[test]
    fn feature_value_flips_winner() {
        // Same model: for x>0 "a" wins; for x<0 "b" wins (reward = -x > +x).
        let m = model_two_variants();
        let pos = vec![1.0, 10.0];
        let neg = vec![1.0, -10.0];
        let mut a_pos = 0;
        let mut b_neg = 0;
        let n = 1000;
        for seed in 0..n {
            if sample_contextual_variant(&m, &pos, BanditGoal::Increase, seed).as_deref()
                == Some("a")
            {
                a_pos += 1;
            }
            if sample_contextual_variant(&m, &neg, BanditGoal::Increase, seed).as_deref()
                == Some("b")
            {
                b_neg += 1;
            }
        }
        assert!(a_pos > 950, "a wins for x>0: {a_pos}/{n}");
        assert!(b_neg > 950, "b wins for x<0: {b_neg}/{n}");
    }

    #[test]
    fn decrease_goal_prefers_lower_prediction() {
        // Under Decrease, the lower-reward variant should win. For x>0 "b" gives
        // -x (lower), so "b" wins most contexts.
        let m = model_two_variants();
        let x = vec![1.0, 10.0];
        let mut b = 0;
        let n = 1000;
        for seed in 0..n {
            if sample_contextual_variant(&m, &x, BanditGoal::Decrease, seed).as_deref() == Some("b")
            {
                b += 1;
            }
        }
        assert!(
            b > 950,
            "b (lower reward) should win under Decrease: {b}/{n}"
        );
    }

    #[test]
    fn linucb_variance_drives_exploration() {
        // Variant "b" has a LOWER mean (-0.5) than "a" (0.0) but a large a_inv
        // (high uncertainty). A pure-exploitation rule would NEVER pick "b";
        // LinUCB exploration should let "b" win a meaningful share of contexts
        // because its wide posterior occasionally over-draws "a". A zero-variance
        // "b" with the same lower mean would win ~0%. d=2 (intercept + feature).
        let model = ContextualModel {
            features: vec![numeric_feat("user", "x")],
            variants: vec![
                VariantCoefficients {
                    variant_key: "a".into(),
                    coeffs: vec![0.0, 0.0],
                    a_inv: Some(vec![1e-8, 0.0, 0.0, 1e-8]),
                },
                VariantCoefficients {
                    variant_key: "b".into(),
                    coeffs: vec![-0.5, 0.0],
                    a_inv: Some(vec![25.0, 0.0, 0.0, 25.0]),
                },
            ],
        };
        let x = vec![1.0, 1.0];
        let mut b = 0;
        let n = 2000;
        for seed in 0..n {
            if sample_contextual_variant(&model, &x, BanditGoal::Increase, seed).as_deref()
                == Some("b")
            {
                b += 1;
            }
        }
        // The high-uncertainty, lower-mean arm still gets explored a real share
        // of the time (a deterministic argmax would give it 0%).
        let frac = b as f64 / n as f64;
        assert!(
            (0.2..0.8).contains(&frac),
            "high-variance lower-mean arm should be explored, not dominate: {b}/{n}"
        );
    }

    // ── fit_design_inverse ───────────────────────────────────────────────────

    #[test]
    fn design_inverse_is_inverse_of_a() {
        let rows: Vec<(Vec<f64>, f64)> = (0..5).map(|i| (vec![1.0, i as f64], 0.0)).collect();
        let lambda = 0.5;
        let inv = fit_design_inverse(&rows, lambda).expect("invertible");
        // Reconstruct A and check A * inv ≈ I.
        let d = 2;
        let mut a = vec![0.0; d * d];
        for (x, _) in &rows {
            for i in 0..d {
                for j in 0..d {
                    a[i * d + j] += x[i] * x[j];
                }
            }
        }
        for i in 0..d {
            a[i * d + i] += lambda;
        }
        for i in 0..d {
            for j in 0..d {
                let mut s = 0.0;
                for k in 0..d {
                    s += a[i * d + k] * inv[k * d + j];
                }
                let expect = if i == j { 1.0 } else { 0.0 };
                assert!((s - expect).abs() < 1e-6, "A*inv[{i}][{j}]={s}");
            }
        }
    }

    // ── serde round-trip ─────────────────────────────────────────────────────

    #[test]
    fn model_serde_round_trips() {
        let m = ContextualModel {
            features: vec![
                numeric_feat("user", "age"),
                onehot_feat("user", "plan", &["free", "pro"]),
            ],
            variants: vec![VariantCoefficients {
                variant_key: "a".into(),
                coeffs: vec![1.0, 2.0, 3.0, 4.0],
                a_inv: Some(vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            }],
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: ContextualModel = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
        assert_eq!(m.dim(), 4);
    }

    #[test]
    fn a_inv_omitted_when_none() {
        let vc = VariantCoefficients {
            variant_key: "a".into(),
            coeffs: vec![1.0],
            a_inv: None,
        };
        let json = serde_json::to_string(&vc).unwrap();
        assert!(!json.contains("a_inv"), "{json}");
    }
}
