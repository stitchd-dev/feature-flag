//! Experiment start-time prerequisites (Phase 5 Task 2).
//!
//! An experiment may declare prerequisites that must hold for it to be **started**.
//! Both a manual start (a `TransitionExperiment` to ACTIVE issued by an operator)
//! and a scheduled start (the schedule-service issuing the *same*
//! `TransitionExperiment`) flow through the one start path in
//! [`crate::service::ExperimentationServiceImpl::transition_experiment`], so
//! enforcing the gate there covers BOTH. If any prerequisite is unmet at start
//! time the start is rejected with a `FAILED_PRECONDITION` status carrying a clear
//! reason (surfaced as `409` on a gateway path).
//!
//! Two prerequisite kinds (spec C4):
//!   * **flag-variant** — a named flag must currently serve a required variant.
//!   * **experiment-done** — a referenced experiment must be stopped/concluded.
//!
//! Storage is the `experiment_start_prerequisites` table (migration
//! `20260604000002`). New table → runtime `sqlx::query`/`query_as` (no
//! compile-time `query!` macros), matching the rest of the codebase.

use async_trait::async_trait;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use stitchd_db::RepositoryError;

/// A single start-time prerequisite for an experiment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartPrerequisite {
    /// The named flag must currently serve `required_variant_id`.
    FlagVariant {
        /// The prerequisite flag.
        flag_id: Uuid,
        /// The variant the prerequisite flag must currently serve.
        required_variant_id: Uuid,
    },
    /// The referenced experiment must be stopped/concluded.
    ExperimentDone {
        /// The prerequisite experiment.
        experiment_id: Uuid,
    },
}

/// Sentinel prefix encoding the blocking dependents into a
/// `tonic::Status::failed_precondition` message so the gateway can rebuild the
/// structured `409 DEPENDENCY_EXISTS` body. Mirrors
/// `stitchd_flag_service::prerequisites::DEPENDENCY_EXISTS_STATUS_PREFIX`
/// exactly — there is no shared const crate (same convention as the
/// `flag_locked_by_experiment:<uuid>` sentinel).
///
/// Format: `"dependency_exists:<comma-separated dependent experiment ids>"`.
pub const DEPENDENCY_EXISTS_STATUS_PREFIX: &str = "dependency_exists:";

/// Format the `dependency_exists:` sentinel message from blocking dependent
/// experiment ids.
#[must_use]
pub fn dependency_exists_message(dependents: &[Uuid]) -> String {
    let ids = dependents
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("{DEPENDENCY_EXISTS_STATUS_PREFIX}{ids}")
}

/// Loads the start-time prerequisites declared for an experiment.
#[async_trait]
pub trait StartPrerequisiteRepository: Send + Sync {
    /// Return all start-time prerequisites for `experiment_id`, ordered by their
    /// stable display `order`. An experiment with none returns an empty vec.
    async fn list_for_experiment(
        &self,
        experiment_id: Uuid,
    ) -> Result<Vec<StartPrerequisite>, RepositoryError>;

    /// Return the experiments that declare `experiment_id` as an
    /// `experiment_done` start-time prerequisite — i.e. the experiments that
    /// would be blocked from starting if `experiment_id` were deleted. Used by
    /// the delete-block referential-integrity guard (Phase 6). Empty when no
    /// experiment references it.
    async fn dependents_experiment_done(
        &self,
        experiment_id: Uuid,
    ) -> Result<Vec<Uuid>, RepositoryError>;
}

/// Postgres-backed [`StartPrerequisiteRepository`] over
/// `experiment_start_prerequisites`.
pub struct PgStartPrerequisiteRepository {
    pool: PgPool,
}

impl PgStartPrerequisiteRepository {
    /// Construct over a connection pool.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a start-time prerequisite. Convenience for callers / tests; the
    /// admin write path (declaring prerequisites) is otherwise out of this
    /// task's scope.
    ///
    /// # Errors
    /// Returns a [`RepositoryError`] on a database failure or constraint
    /// violation (e.g. a shape that violates `chk_experiment_start_prereq_shape`).
    pub async fn insert(
        &self,
        experiment_id: Uuid,
        prereq: &StartPrerequisite,
        order: i32,
    ) -> Result<Uuid, RepositoryError> {
        let (kind, flag_id, variant_id, prereq_exp_id) = match prereq {
            StartPrerequisite::FlagVariant {
                flag_id,
                required_variant_id,
            } => (
                "flag_variant",
                Some(*flag_id),
                Some(*required_variant_id),
                None,
            ),
            StartPrerequisite::ExperimentDone { experiment_id } => {
                ("experiment_done", None, None, Some(*experiment_id))
            }
        };
        let row = sqlx::query(
            r#"
            INSERT INTO experiment_start_prerequisites
                (experiment_id, kind, prerequisite_flag_id, required_variant_id,
                 prerequisite_experiment_id, "order")
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(experiment_id)
        .bind(kind)
        .bind(flag_id)
        .bind(variant_id)
        .bind(prereq_exp_id)
        .bind(order)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(row.get::<Uuid, _>("id"))
    }
}

#[async_trait]
impl StartPrerequisiteRepository for PgStartPrerequisiteRepository {
    async fn list_for_experiment(
        &self,
        experiment_id: Uuid,
    ) -> Result<Vec<StartPrerequisite>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT kind, prerequisite_flag_id, required_variant_id, prerequisite_experiment_id
            FROM experiment_start_prerequisites
            WHERE experiment_id = $1
            ORDER BY "order", id
            "#,
        )
        .bind(experiment_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let kind: String = row.get("kind");
            match kind.as_str() {
                "flag_variant" => out.push(StartPrerequisite::FlagVariant {
                    flag_id: row.get("prerequisite_flag_id"),
                    required_variant_id: row.get("required_variant_id"),
                }),
                "experiment_done" => out.push(StartPrerequisite::ExperimentDone {
                    experiment_id: row.get("prerequisite_experiment_id"),
                }),
                other => {
                    return Err(RepositoryError::InvalidState {
                        reason: format!("unknown experiment start prerequisite kind '{other}'"),
                    });
                }
            }
        }
        Ok(out)
    }

    async fn dependents_experiment_done(
        &self,
        experiment_id: Uuid,
    ) -> Result<Vec<Uuid>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT DISTINCT experiment_id
            FROM experiment_start_prerequisites
            WHERE kind = 'experiment_done'
              AND prerequisite_experiment_id = $1
            ORDER BY experiment_id
            ",
        )
        .bind(experiment_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(rows.into_iter().map(|r| r.get("experiment_id")).collect())
    }
}

/// The outcome of evaluating one prerequisite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrereqCheck {
    /// The prerequisite is satisfied.
    Met,
    /// The prerequisite is not satisfied; carries a human-readable reason.
    Unmet(String),
}

/// Evaluates whether each kind of start-time prerequisite is currently met.
///
/// Abstracted as a trait so the start gate is unit-testable without a live
/// flag-service or experiment store. The production implementation
/// ([`crate::service`] wires it) resolves `experiment_done` against the experiment
/// repository and `flag_variant` against the Flag Service.
#[async_trait]
pub trait StartPrerequisiteResolver: Send + Sync {
    /// Is the flag-variant prerequisite currently met — i.e. does
    /// `flag_id` currently serve `required_variant_id`?
    async fn flag_serves_variant(
        &self,
        environment_id: &str,
        flag_id: Uuid,
        required_variant_id: Uuid,
    ) -> Result<PrereqCheck, tonic::Status>;

    /// Is the experiment-done prerequisite currently met — i.e. is
    /// `experiment_id` stopped/concluded?
    async fn experiment_is_done(&self, experiment_id: Uuid) -> Result<PrereqCheck, tonic::Status>;
}

/// Evaluate every prerequisite for `experiment_id`. Returns `Ok(None)` when all
/// are met (or none declared), or `Ok(Some(reason))` naming the first unmet
/// prerequisite. The caller turns `Some(reason)` into a `FAILED_PRECONDITION`.
///
/// # Errors
/// Propagates a `tonic::Status` only on an unexpected resolver error (transport /
/// internal) — never for a merely-unmet prerequisite, which is a normal `Some`.
pub async fn evaluate_start_prerequisites(
    repo: &dyn StartPrerequisiteRepository,
    resolver: &dyn StartPrerequisiteResolver,
    environment_id: &str,
    experiment_id: Uuid,
) -> Result<Option<String>, tonic::Status> {
    let prereqs = repo
        .list_for_experiment(experiment_id)
        .await
        .map_err(tonic::Status::from)?;

    for prereq in &prereqs {
        let check = match prereq {
            StartPrerequisite::FlagVariant {
                flag_id,
                required_variant_id,
            } => {
                resolver
                    .flag_serves_variant(environment_id, *flag_id, *required_variant_id)
                    .await?
            }
            StartPrerequisite::ExperimentDone { experiment_id } => {
                resolver.experiment_is_done(*experiment_id).await?
            }
        };
        if let PrereqCheck::Unmet(reason) = check {
            return Ok(Some(reason));
        }
    }
    Ok(None)
}

/// Resolves the variant a flag currently **serves** in an environment, by UUID.
///
/// Abstracted as a trait so [`ServiceStartPrerequisiteResolver`]'s `flag_variant`
/// gate is unit-testable without a live Flag Service. The production
/// implementation ([`FlagServiceServedVariantLookup`]) resolves `flag_id` →
/// `flag_key` via the flag repository, then reads the live `FeatureFlag` from the
/// Flag Service to determine the served variant's UUID — exact comparison
/// (replacing the former fail-closed behaviour, now that the Flag Service exposes
/// variant UUIDs on its read surface).
#[async_trait]
pub trait ServedVariantLookup: Send + Sync {
    /// Return the UUID of the variant `flag_id` currently serves in
    /// `environment_id`, or `Ok(None)` when the flag serves no determinate
    /// variant (disabled, or no default variant configured). `Err` only on an
    /// unexpected transport/internal failure.
    async fn served_variant(
        &self,
        environment_id: &str,
        flag_id: Uuid,
    ) -> Result<Option<Uuid>, tonic::Status>;
}

/// Production [`StartPrerequisiteResolver`].
///
/// * **experiment_done** — resolved against the experiment repository: the
///   prerequisite experiment must be `Stopped`/concluded.
/// * **flag_variant** — resolved exactly via a [`ServedVariantLookup`]: the live
///   variant the flag currently serves is compared by UUID against the stored
///   `required_variant_id`. A match meets the prerequisite (allowing the start);
///   any mismatch (incl. a disabled flag serving no variant) reports `Unmet`.
pub struct ServiceStartPrerequisiteResolver {
    experiment_repo: std::sync::Arc<dyn stitchd_db::ExperimentRepository>,
    served_variant_lookup: std::sync::Arc<dyn ServedVariantLookup>,
}

impl ServiceStartPrerequisiteResolver {
    /// Construct over the experiment repository + a served-variant lookup.
    #[must_use]
    pub fn new(
        experiment_repo: std::sync::Arc<dyn stitchd_db::ExperimentRepository>,
        served_variant_lookup: std::sync::Arc<dyn ServedVariantLookup>,
    ) -> Self {
        Self {
            experiment_repo,
            served_variant_lookup,
        }
    }
}

/// Compare the variant a flag currently serves against the required variant.
///
/// `Met` exactly when the flag serves `required_variant_id`; otherwise `Unmet`
/// with a reason that names the mismatch (or the absence of a served variant —
/// e.g. a disabled flag or one with no default variant).
#[must_use]
pub fn compare_served_variant(
    flag_id: Uuid,
    served: Option<Uuid>,
    required_variant_id: Uuid,
) -> PrereqCheck {
    match served {
        Some(served) if served == required_variant_id => PrereqCheck::Met,
        Some(served) => PrereqCheck::Unmet(format!(
            "flag-variant prerequisite unmet: flag {flag_id} serves variant \
             {served}, requires {required_variant_id}"
        )),
        None => PrereqCheck::Unmet(format!(
            "flag-variant prerequisite unmet: flag {flag_id} serves no variant \
             (disabled or no default), requires {required_variant_id}"
        )),
    }
}

#[async_trait]
impl StartPrerequisiteResolver for ServiceStartPrerequisiteResolver {
    async fn flag_serves_variant(
        &self,
        environment_id: &str,
        flag_id: Uuid,
        required_variant_id: Uuid,
    ) -> Result<PrereqCheck, tonic::Status> {
        // Exact verification: resolve the variant the flag currently serves and
        // compare it to the required variant by UUID.
        let served = self
            .served_variant_lookup
            .served_variant(environment_id, flag_id)
            .await?;
        Ok(compare_served_variant(flag_id, served, required_variant_id))
    }

    async fn experiment_is_done(&self, experiment_id: Uuid) -> Result<PrereqCheck, tonic::Status> {
        let exp_id = stitchd_core::id::ExperimentId::from_uuid(experiment_id);
        let exp = self
            .experiment_repo
            .find_by_id(exp_id)
            .await
            .map_err(tonic::Status::from)?;
        if exp.status == stitchd_core::experimentation::ExperimentStatus::Stopped {
            Ok(PrereqCheck::Met)
        } else {
            Ok(PrereqCheck::Unmet(format!(
                "prerequisite experiment {experiment_id} is {:?}, must be stopped",
                exp.status
            )))
        }
    }
}

/// Production [`ServedVariantLookup`] backed by the flag repository (for
/// `flag_id → flag_key` resolution) and the Flag Service (for the live served
/// variant + its UUID).
///
/// The flag a start-prerequisite references is arbitrary (not necessarily the
/// experiment's bound flag), so its key is resolved via [`stitchd_db::FlagRepository::find_by_id`].
/// The Flag Service's `GetFlag` then returns the live `FeatureFlag` whose
/// `default_variant_key` names the served variant and whose `variants[].id`
/// carries the variant UUID (exposed on the read surface in
/// `flag_lifecycle_20260604`). The served variant is `None` when the flag is
/// disabled or has no default variant configured.
pub struct FlagServiceServedVariantLookup {
    flag_repo: std::sync::Arc<dyn stitchd_db::FlagRepository>,
    flag_client: crate::flag_client::FlagClient,
}

impl FlagServiceServedVariantLookup {
    /// Construct over the flag repository + a Flag Service client.
    #[must_use]
    pub fn new(
        flag_repo: std::sync::Arc<dyn stitchd_db::FlagRepository>,
        flag_client: crate::flag_client::FlagClient,
    ) -> Self {
        Self {
            flag_repo,
            flag_client,
        }
    }
}

/// Fail-closed [`ServedVariantLookup`] used when no Flag Service connection is
/// available at startup. It reports the flag as serving no variant, so any
/// `flag_variant` start-prerequisite is treated as unmet (refusing the start)
/// rather than allowed on an unverifiable assumption.
pub struct UnavailableServedVariantLookup;

#[async_trait]
impl ServedVariantLookup for UnavailableServedVariantLookup {
    async fn served_variant(
        &self,
        _environment_id: &str,
        _flag_id: Uuid,
    ) -> Result<Option<Uuid>, tonic::Status> {
        Ok(None)
    }
}

#[async_trait]
impl ServedVariantLookup for FlagServiceServedVariantLookup {
    async fn served_variant(
        &self,
        environment_id: &str,
        flag_id: Uuid,
    ) -> Result<Option<Uuid>, tonic::Status> {
        let flag_record = self
            .flag_repo
            .find_by_id(stitchd_core::id::FlagId::from_uuid(flag_id))
            .await
            .map_err(tonic::Status::from)?;

        let flag = self
            .flag_client
            .get_flag(environment_id, flag_record.key.as_str())
            .await?;

        // A disabled flag, or one without a default variant, serves no
        // determinate variant for the gate.
        if !flag.enabled || flag.default_variant_key.is_empty() {
            return Ok(None);
        }

        // Find the served (default) variant and return its UUID.
        let served = flag
            .variants
            .iter()
            .find(|v| v.key == flag.default_variant_key)
            .and_then(|v| v.id.parse::<Uuid>().ok());
        Ok(served)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Stub repo returning a fixed prereq list.
    struct StubRepo(Vec<StartPrerequisite>);
    #[async_trait]
    impl StartPrerequisiteRepository for StubRepo {
        async fn list_for_experiment(
            &self,
            _experiment_id: Uuid,
        ) -> Result<Vec<StartPrerequisite>, RepositoryError> {
            Ok(self.0.clone())
        }
        async fn dependents_experiment_done(
            &self,
            _experiment_id: Uuid,
        ) -> Result<Vec<Uuid>, RepositoryError> {
            Ok(vec![])
        }
    }

    /// Stub resolver scripted per-kind.
    struct StubResolver {
        flag: Mutex<PrereqCheck>,
        experiment: Mutex<PrereqCheck>,
        flag_calls: Mutex<u32>,
        experiment_calls: Mutex<u32>,
    }
    impl StubResolver {
        fn new(flag: PrereqCheck, experiment: PrereqCheck) -> Self {
            Self {
                flag: Mutex::new(flag),
                experiment: Mutex::new(experiment),
                flag_calls: Mutex::new(0),
                experiment_calls: Mutex::new(0),
            }
        }
    }
    #[async_trait]
    impl StartPrerequisiteResolver for StubResolver {
        async fn flag_serves_variant(
            &self,
            _environment_id: &str,
            _flag_id: Uuid,
            _required_variant_id: Uuid,
        ) -> Result<PrereqCheck, tonic::Status> {
            *self.flag_calls.lock().unwrap() += 1;
            Ok(self.flag.lock().unwrap().clone())
        }
        async fn experiment_is_done(
            &self,
            _experiment_id: Uuid,
        ) -> Result<PrereqCheck, tonic::Status> {
            *self.experiment_calls.lock().unwrap() += 1;
            Ok(self.experiment.lock().unwrap().clone())
        }
    }

    #[tokio::test]
    async fn no_prerequisites_means_met() {
        let repo = StubRepo(vec![]);
        let resolver = StubResolver::new(PrereqCheck::Met, PrereqCheck::Met);
        let r = evaluate_start_prerequisites(&repo, &resolver, "env", Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(r, None);
    }

    #[tokio::test]
    async fn all_met_returns_none() {
        let repo = StubRepo(vec![
            StartPrerequisite::FlagVariant {
                flag_id: Uuid::new_v4(),
                required_variant_id: Uuid::new_v4(),
            },
            StartPrerequisite::ExperimentDone {
                experiment_id: Uuid::new_v4(),
            },
        ]);
        let resolver = StubResolver::new(PrereqCheck::Met, PrereqCheck::Met);
        let r = evaluate_start_prerequisites(&repo, &resolver, "env", Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(r, None);
    }

    #[tokio::test]
    async fn unmet_flag_variant_refuses_start_with_reason() {
        let repo = StubRepo(vec![StartPrerequisite::FlagVariant {
            flag_id: Uuid::new_v4(),
            required_variant_id: Uuid::new_v4(),
        }]);
        let resolver = StubResolver::new(
            PrereqCheck::Unmet("flag 'kill-switch' serves 'off', requires 'on'".to_string()),
            PrereqCheck::Met,
        );
        let r = evaluate_start_prerequisites(&repo, &resolver, "env", Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(
            r,
            Some("flag 'kill-switch' serves 'off', requires 'on'".to_string())
        );
    }

    #[tokio::test]
    async fn unmet_experiment_done_refuses_start() {
        let repo = StubRepo(vec![StartPrerequisite::ExperimentDone {
            experiment_id: Uuid::new_v4(),
        }]);
        let resolver = StubResolver::new(
            PrereqCheck::Met,
            PrereqCheck::Unmet("prerequisite experiment is still running".to_string()),
        );
        let r = evaluate_start_prerequisites(&repo, &resolver, "env", Uuid::new_v4())
            .await
            .unwrap();
        assert!(r.unwrap().contains("still running"));
    }

    // ── PG repo round-trip (#[sqlx::test], live DB) ───────────────────────────

    /// Seed org → project → env → flag → experiment, returning the experiment id.
    #[cfg(test)]
    async fn seed_experiment(pool: &PgPool) -> Uuid {
        let org = Uuid::new_v4();
        let project = Uuid::new_v4();
        let env = Uuid::new_v4();
        let flag = Uuid::new_v4();
        let exp = Uuid::new_v4();
        sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
            .bind(org)
            .bind(format!("org-{org}"))
            .execute(pool)
            .await
            .expect("seed org");
        sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
            .bind(project)
            .bind(org)
            .bind(format!("proj-{project}"))
            .execute(pool)
            .await
            .expect("seed project");
        sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
            .bind(env)
            .bind(project)
            .bind("dev")
            .execute(pool)
            .await
            .expect("seed env");
        sqlx::query(
            "INSERT INTO feature_flags (id, project_id, key, value_type, enabled) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(flag)
        .bind(project)
        .bind(format!("flag-{flag}"))
        .bind("bool")
        .bind(true)
        .execute(pool)
        .await
        .expect("seed flag");
        sqlx::query(
            "INSERT INTO experiments \
                (id, env_id, flag_id, name, status, traffic_allocation, \
                 targets_default_rule, unit_context_types) \
             VALUES ($1, $2, $3, $4, $5, $6::numeric, $7, $8)",
        )
        .bind(exp)
        .bind(env)
        .bind(flag)
        .bind(format!("exp-{exp}"))
        .bind("draft")
        .bind(100.0_f64)
        .bind(true)
        .bind(vec!["user".to_string()])
        .execute(pool)
        .await
        .expect("seed experiment");
        exp
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn pg_repo_roundtrips_both_prereq_kinds_in_order(pool: PgPool) {
        let exp = seed_experiment(&pool).await;
        let other_exp = seed_experiment(&pool).await;
        let repo = PgStartPrerequisiteRepository::new(pool);

        let flag_id = Uuid::new_v4();
        let variant_id = Uuid::new_v4();
        // Insert experiment_done first with a LATER order, flag_variant with an
        // earlier order — list must return flag_variant first.
        repo.insert(
            exp,
            &StartPrerequisite::ExperimentDone {
                experiment_id: other_exp,
            },
            5,
        )
        .await
        .expect("insert experiment_done");
        repo.insert(
            exp,
            &StartPrerequisite::FlagVariant {
                flag_id,
                required_variant_id: variant_id,
            },
            1,
        )
        .await
        .expect("insert flag_variant");

        let prereqs = repo.list_for_experiment(exp).await.expect("list");
        assert_eq!(prereqs.len(), 2);
        assert_eq!(
            prereqs[0],
            StartPrerequisite::FlagVariant {
                flag_id,
                required_variant_id: variant_id,
            }
        );
        assert_eq!(
            prereqs[1],
            StartPrerequisite::ExperimentDone {
                experiment_id: other_exp,
            }
        );
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn pg_repo_dependents_experiment_done(pool: PgPool) {
        // exp_a is referenced by exp_b's experiment_done prerequisite; exp_c is
        // referenced by nobody. dependents_experiment_done(exp_a) must return
        // [exp_b]; dependents_experiment_done(exp_c) must be empty.
        let exp_a = seed_experiment(&pool).await;
        let exp_b = seed_experiment(&pool).await;
        let exp_c = seed_experiment(&pool).await;
        let repo = PgStartPrerequisiteRepository::new(pool);

        repo.insert(
            exp_b,
            &StartPrerequisite::ExperimentDone {
                experiment_id: exp_a,
            },
            0,
        )
        .await
        .expect("insert experiment_done");

        let deps_a = repo
            .dependents_experiment_done(exp_a)
            .await
            .expect("dependents a");
        assert_eq!(deps_a, vec![exp_b]);

        let deps_c = repo
            .dependents_experiment_done(exp_c)
            .await
            .expect("dependents c");
        assert!(deps_c.is_empty());
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn pg_repo_experiment_with_no_prereqs_is_empty(pool: PgPool) {
        let exp = seed_experiment(&pool).await;
        let repo = PgStartPrerequisiteRepository::new(pool);
        assert!(
            repo.list_for_experiment(exp)
                .await
                .expect("list")
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn pg_repo_shape_constraint_rejects_mismatched_columns(pool: PgPool) {
        // The CHECK constraint forbids a flag_variant row that also names an
        // experiment — exercised by binding the wrong columns directly.
        let exp = seed_experiment(&pool).await;
        let res = sqlx::query(
            r#"INSERT INTO experiment_start_prerequisites
                   (experiment_id, kind, prerequisite_flag_id, required_variant_id,
                    prerequisite_experiment_id, "order")
               VALUES ($1, 'flag_variant', $2, $3, $4, 0)"#,
        )
        .bind(exp)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4()) // experiment id set on a flag_variant → violates shape
        .execute(&pool)
        .await;
        assert!(
            res.is_err(),
            "shape constraint must reject mismatched columns"
        );
    }

    // ── compare_served_variant — exact flag-variant verification ──────────────

    #[test]
    fn compare_served_variant_met_when_served_equals_required() {
        let required = Uuid::new_v4();
        assert_eq!(
            compare_served_variant(Uuid::new_v4(), Some(required), required),
            PrereqCheck::Met
        );
    }

    #[test]
    fn compare_served_variant_unmet_when_served_differs() {
        let r = compare_served_variant(Uuid::new_v4(), Some(Uuid::new_v4()), Uuid::new_v4());
        assert!(matches!(r, PrereqCheck::Unmet(reason) if reason.contains("serves variant")));
    }

    #[test]
    fn compare_served_variant_unmet_when_flag_serves_no_variant() {
        let r = compare_served_variant(Uuid::new_v4(), None, Uuid::new_v4());
        assert!(matches!(r, PrereqCheck::Unmet(reason) if reason.contains("no variant")));
    }

    #[tokio::test]
    async fn first_unmet_short_circuits() {
        // The flag prereq is unmet → the experiment prereq is never evaluated.
        let repo = StubRepo(vec![
            StartPrerequisite::FlagVariant {
                flag_id: Uuid::new_v4(),
                required_variant_id: Uuid::new_v4(),
            },
            StartPrerequisite::ExperimentDone {
                experiment_id: Uuid::new_v4(),
            },
        ]);
        let resolver = StubResolver::new(
            PrereqCheck::Unmet("flag unmet".to_string()),
            PrereqCheck::Met,
        );
        let r = evaluate_start_prerequisites(&repo, &resolver, "env", Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(r, Some("flag unmet".to_string()));
        assert_eq!(*resolver.flag_calls.lock().unwrap(), 1);
        assert_eq!(*resolver.experiment_calls.lock().unwrap(), 0);
    }
}
