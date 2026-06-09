//! Integration tests for `PgMetricRepository`.
//!
//! Uses `#[sqlx::test(migrations = "./migrations")]` for isolated DB
//! per test — each gets a fresh temp database with all migrations
//! applied. The seed_env helper provisions the parent org → project →
//! environment chain that every metric needs.

use std::sync::Arc;

use chrono::Timelike;
use stitchd_core::{
    id::{EnvironmentId, MetricId, OrganisationId, ProjectId},
    metric::{
        AggregationConfig, AggregationOperator, FunnelConfig, FunnelStep, GoalDirection,
        MetricDefinition, MetricKind, RatioConfig,
    },
    tenant::{Environment, Organisation, Project},
};

use stitchd_db::{
    EnvironmentRepository, MetricRepository, OrganisationRepository, ProjectRepository,
    RepositoryError,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgMetricRepository, PgOrganisationRepository,
        PgProjectRepository,
    },
};

// ── Setup ────────────────────────────────────────────────────────────────────

/// Current UTC timestamp truncated to microsecond precision.
///
/// PostgreSQL `TIMESTAMP WITH TIME ZONE` columns store at most
/// microsecond resolution (6 fractional digits), but `chrono::Utc::now()`
/// returns nanoseconds. Without this truncation, a round-trip through
/// `INSERT … RETURNING *` (or insert → fetch) loses the sub-microsecond
/// digits and `assert_eq!(fetched, original)` panics with a confusing
/// "left == right" diff that only differs in the last 3 ns digits. CI
/// (Linux) reliably trips on this; macOS local runs sometimes don't
/// because chrono's nanosecond source has lower entropy there. Using
/// this helper everywhere in the fixture bodies makes the tests
/// deterministic across platforms.
fn now_micros() -> chrono::DateTime<chrono::Utc> {
    let now = chrono::Utc::now();
    let nanos = now.timestamp_subsec_nanos();
    let micros_only = (nanos / 1_000) * 1_000;
    now.with_nanosecond(micros_only)
        .expect("micros derived from nanos is always in range")
}

async fn seed_env(pool: &sqlx::PgPool) -> (PgMetricRepository, EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let metric_repo = PgMetricRepository::new(pool.clone(), audit);

    let org = Organisation {
        id: OrganisationId::new(),
        name: "Org".into(),
        created_at: now_micros(),
        updated_at: now_micros(),
        deleted_at: None,
        version: 1,
        is_system: false,
    };
    org_repo.create(&org).await.unwrap();
    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Proj".into(),
        created_at: now_micros(),
        updated_at: now_micros(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();
    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "Env".into(),
        created_at: now_micros(),
        updated_at: now_micros(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();
    (metric_repo, env.id)
}

fn make_aggregation(env_id: EnvironmentId, key: &str) -> MetricDefinition {
    MetricDefinition {
        id: MetricId::new(),
        environment_id: env_id,
        key: key.into(),
        name: format!("Metric {key}"),
        description: Some(format!("Test metric `{key}`")),
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now_micros(),
        updated_at: now_micros(),
        deleted_at: None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn create_and_find_by_id(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m = make_aggregation(env, "checkout_completion_count");
    repo.create(&m).await.unwrap();
    let fetched = repo.find_by_id(m.id).await.unwrap();
    assert_eq!(fetched, m);
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_id_returns_not_found_for_missing(pool: sqlx::PgPool) {
    let (repo, _env) = seed_env(&pool).await;
    let err = repo.find_by_id(MetricId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_key_returns_live_metric(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m = make_aggregation(env, "checkout_rate");
    repo.create(&m).await.unwrap();
    let fetched = repo.find_by_key("checkout_rate", env).await.unwrap();
    assert_eq!(fetched.id, m.id);
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_key_returns_not_found_for_missing(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let err = repo.find_by_key("nope", env).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn duplicate_key_in_env_violates_unique(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m1 = make_aggregation(env, "dup");
    let mut m2 = make_aggregation(env, "dup");
    m2.id = MetricId::new();
    repo.create(&m1).await.unwrap();
    let err = repo.create(&m2).await.unwrap_err();
    assert!(matches!(err, RepositoryError::UniqueViolation { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn soft_delete_allows_recreating_same_key(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m1 = make_aggregation(env, "reusable_key");
    repo.create(&m1).await.unwrap();
    repo.soft_delete(m1.id).await.unwrap();

    // After soft-delete, the partial UNIQUE index should allow a new
    // metric with the same key.
    let mut m2 = make_aggregation(env, "reusable_key");
    m2.id = MetricId::new();
    repo.create(&m2).await.unwrap();

    // find_by_key returns the live one, not the deleted one.
    let fetched = repo.find_by_key("reusable_key", env).await.unwrap();
    assert_eq!(fetched.id, m2.id);
}

#[sqlx::test(migrations = "./migrations")]
async fn soft_delete_missing_returns_not_found(pool: sqlx::PgPool) {
    let (repo, _env) = seed_env(&pool).await;
    let err = repo.soft_delete(MetricId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_returns_only_live(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m1 = make_aggregation(env, "alpha");
    let m2 = make_aggregation(env, "beta");
    let m3 = make_aggregation(env, "gamma");
    repo.create(&m1).await.unwrap();
    repo.create(&m2).await.unwrap();
    repo.create(&m3).await.unwrap();
    repo.soft_delete(m2.id).await.unwrap();

    let listed = repo.list_by_environment(env).await.unwrap();
    let keys: Vec<_> = listed.iter().map(|m| m.key.as_str()).collect();
    assert!(keys.contains(&"alpha"));
    assert!(keys.contains(&"gamma"));
    assert!(!keys.contains(&"beta"));
    assert_eq!(listed.len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_first_page_and_next_cursor(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    for i in 0..5 {
        repo.create(&make_aggregation(env, &format!("m{i}")))
            .await
            .unwrap();
    }
    let (page, next) = repo.list_by_environment_keyset(env, None, 3).await.unwrap();
    assert_eq!(page.len(), 3, "first page returns limit items");
    assert!(next.is_some(), "more rows remain ⇒ a next cursor");

    let (page2, next2) = repo
        .list_by_environment_keyset(env, None, 50)
        .await
        .unwrap();
    assert_eq!(page2.len(), 5);
    assert!(next2.is_none(), "all rows on one page ⇒ no next cursor");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_empty(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let (page, next) = repo
        .list_by_environment_keyset(env, None, 50)
        .await
        .unwrap();
    assert!(page.is_empty());
    assert!(next.is_none());
}

/// Rigorous correctness: paging through with the returned cursor visits EVERY
/// row exactly once, in (created_at, id) order, with no duplicates or gaps.
#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_pages_through_all_rows_exactly_once(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    const N: usize = 23;
    for i in 0..N {
        repo.create(&make_aggregation(env, &format!("m{i:03}")))
            .await
            .unwrap();
    }

    // Walk pages of 7 (so the last page is partial: 23 = 7+7+7+2).
    let mut seen: Vec<uuid::Uuid> = Vec::new();
    let mut cursor: Option<stitchd_db::KeysetCursor> = None;
    let mut pages = 0;
    loop {
        let (items, next) = repo
            .list_by_environment_keyset(env, cursor, 7)
            .await
            .unwrap();
        pages += 1;
        assert!(items.len() <= 7, "never more than the limit per page");
        for m in &items {
            seen.push(m.id.as_uuid());
        }
        match next {
            Some(tok) => cursor = Some(stitchd_db::KeysetCursor::decode(&tok).unwrap()),
            None => break,
        }
        assert!(pages <= N + 1, "must terminate");
    }

    assert_eq!(
        seen.len(),
        N,
        "every row visited exactly once — no gaps/dupes"
    );
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), N, "no duplicates across pages");
    assert_eq!(pages, 4, "23 rows / 7 per page = 4 pages (7+7+7+2)");
}

#[sqlx::test(migrations = "./migrations")]
async fn update_increments_version_and_persists(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let mut m = make_aggregation(env, "mutable");
    repo.create(&m).await.unwrap();

    m.name = "Updated Name".into();
    m.description = Some("Updated".into());
    m.goal_direction = GoalDirection::Decrease;
    let updated = repo.update(&m).await.unwrap();
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(updated.description.as_deref(), Some("Updated"));
    assert_eq!(updated.goal_direction, GoalDirection::Decrease);
    assert_eq!(updated.version, m.version + 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn update_stale_version_returns_conflict(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let mut m = make_aggregation(env, "raced");
    repo.create(&m).await.unwrap();
    // First update bumps version to 2.
    let _ = repo.update(&m).await.unwrap();
    // Caller still holds version 1 → conflict.
    m.name = "Stale".into();
    let err = repo.update(&m).await.unwrap_err();
    match err {
        RepositoryError::VersionConflict { expected, actual } => {
            assert_eq!(expected, 1);
            assert_eq!(actual, 2);
        }
        other => panic!("expected VersionConflict, got {other:?}"),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn update_missing_returns_not_found(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m = make_aggregation(env, "absent");
    let err = repo.update(&m).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn ratio_metric_round_trips(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let num = make_aggregation(env, "num");
    let den = make_aggregation(env, "den");
    repo.create(&num).await.unwrap();
    repo.create(&den).await.unwrap();

    let ratio = MetricDefinition {
        id: MetricId::new(),
        environment_id: env,
        key: "ratio_metric".into(),
        name: "Conversion Rate".into(),
        description: None,
        kind: MetricKind::Ratio(RatioConfig {
            numerator_metric_id: num.id,
            denominator_metric_id: den.id,
            min_denominator: 100,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now_micros(),
        updated_at: now_micros(),
        deleted_at: None,
    };
    repo.create(&ratio).await.unwrap();
    let fetched = repo.find_by_id(ratio.id).await.unwrap();
    assert_eq!(fetched, ratio);
}

#[sqlx::test(migrations = "./migrations")]
async fn funnel_metric_round_trips_with_steps(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let funnel = MetricDefinition {
        id: MetricId::new(),
        environment_id: env,
        key: "checkout_funnel".into(),
        name: "Checkout Funnel".into(),
        description: Some("3-step purchase funnel".into()),
        kind: MetricKind::Funnel(FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "view_product".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "add_to_cart".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "checkout_completed".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 86_400,
            count_repeats: false,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now_micros(),
        updated_at: now_micros(),
        deleted_at: None,
    };
    repo.create(&funnel).await.unwrap();
    let fetched = repo.find_by_id(funnel.id).await.unwrap();
    assert_eq!(fetched, funnel);
    if let MetricKind::Funnel(c) = &fetched.kind {
        assert_eq!(c.steps.len(), 3);
        assert_eq!(c.window_seconds, 86_400);
    } else {
        panic!("expected Funnel variant");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn find_batch_by_ids_returns_only_live(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m1 = make_aggregation(env, "b1");
    let m2 = make_aggregation(env, "b2");
    let m3 = make_aggregation(env, "b3");
    repo.create(&m1).await.unwrap();
    repo.create(&m2).await.unwrap();
    repo.create(&m3).await.unwrap();
    repo.soft_delete(m2.id).await.unwrap();

    let fetched = repo
        .find_batch_by_ids(&[m1.id, m2.id, m3.id, MetricId::new()])
        .await
        .unwrap();
    let ids: Vec<_> = fetched.iter().map(|m| m.id).collect();
    assert!(ids.contains(&m1.id));
    assert!(ids.contains(&m3.id));
    assert!(!ids.contains(&m2.id));
    assert_eq!(fetched.len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn find_batch_by_ids_empty_input_returns_empty(pool: sqlx::PgPool) {
    let (repo, _env) = seed_env(&pool).await;
    let fetched = repo.find_batch_by_ids(&[]).await.unwrap();
    assert!(fetched.is_empty());
}

// ── list_referencing_event ──────────────────────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn list_referencing_event_matches_aggregation_event_key(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    // `make_aggregation` uses event_key="checkout_completed"
    let m = make_aggregation(env, "agg_on_checkout");
    repo.create(&m).await.unwrap();

    let hits = repo
        .list_referencing_event(env, "checkout_completed")
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, m.id);

    let none = repo
        .list_referencing_event(env, "does_not_exist")
        .await
        .unwrap();
    assert!(none.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn list_referencing_event_matches_funnel_step_event_key(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let funnel = MetricDefinition {
        id: MetricId::new(),
        environment_id: env,
        key: "purchase_funnel".into(),
        name: "Purchase Funnel".into(),
        description: None,
        kind: MetricKind::Funnel(FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "view_item".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "add_to_cart".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "checkout_completed".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 3600,
            count_repeats: false,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now_micros(),
        updated_at: now_micros(),
        deleted_at: None,
    };
    repo.create(&funnel).await.unwrap();

    // Matches via any step's event_key (mid-funnel).
    let hits = repo
        .list_referencing_event(env, "add_to_cart")
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, funnel.id);

    // Last-step event matches too.
    let last = repo
        .list_referencing_event(env, "checkout_completed")
        .await
        .unwrap();
    assert_eq!(last.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_referencing_event_skips_soft_deleted(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m = make_aggregation(env, "to_be_deleted");
    repo.create(&m).await.unwrap();
    // Before delete: 1 hit.
    let before = repo
        .list_referencing_event(env, "checkout_completed")
        .await
        .unwrap();
    assert_eq!(before.len(), 1);

    repo.soft_delete(m.id).await.unwrap();

    // After delete: deleted_at IS NOT NULL filter excludes it.
    let after = repo
        .list_referencing_event(env, "checkout_completed")
        .await
        .unwrap();
    assert!(after.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn list_referencing_event_excludes_ratio_metrics(pool: sqlx::PgPool) {
    // Ratio metrics have no direct event_key — they reference other metrics.
    // The repo intentionally skips them (caller does transitive resolution).
    let (repo, env) = seed_env(&pool).await;
    let num = make_aggregation(env, "num_metric");
    let denom_evt_key = "denom_event"; // use a different event_key for denom
    let mut denom = make_aggregation(env, "denom_metric");
    denom.kind = MetricKind::Aggregation(AggregationConfig {
        event_key: denom_evt_key.into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    });
    repo.create(&num).await.unwrap();
    repo.create(&denom).await.unwrap();

    let ratio = MetricDefinition {
        id: MetricId::new(),
        environment_id: env,
        key: "checkout_rate".into(),
        name: "Checkout Rate".into(),
        description: None,
        kind: MetricKind::Ratio(RatioConfig {
            numerator_metric_id: num.id,
            denominator_metric_id: denom.id,
            min_denominator: 30,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now_micros(),
        updated_at: now_micros(),
        deleted_at: None,
    };
    repo.create(&ratio).await.unwrap();

    // Searching for the numerator's event_key returns ONLY the underlying
    // aggregation, not the ratio that wraps it.
    let hits = repo
        .list_referencing_event(env, "checkout_completed")
        .await
        .unwrap();
    let ids: Vec<_> = hits.iter().map(|m| m.id).collect();
    assert!(ids.contains(&num.id));
    assert!(!ids.contains(&ratio.id));
}
