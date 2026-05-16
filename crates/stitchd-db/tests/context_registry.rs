use chrono::{Duration, Utc};
use stitchd_core::{context::InferredType, id::EnvironmentId};
use stitchd_db::{ContextRegistryRepository, repository::pg::PgContextRegistryRepository};

#[sqlx::test(migrations = "./migrations")]
async fn upsert_context_type_creates_and_is_idempotent(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool);
    let env_id = EnvironmentId::new();

    repo.upsert_context_type(env_id, "user").await.unwrap();
    repo.upsert_context_type(env_id, "user").await.unwrap(); // idempotent

    let types = repo.list_types(env_id).await.unwrap();
    assert_eq!(types.len(), 1);
    assert_eq!(types[0].context_type, "user");
    assert_eq!(types[0].env_id, env_id);
}

#[sqlx::test(migrations = "./migrations")]
async fn upsert_param_creates_and_updates(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool);
    let env_id = EnvironmentId::new();

    repo.upsert_context_type(env_id, "user").await.unwrap();
    repo.upsert_param(env_id, "user", "email", InferredType::Str, true)
        .await
        .unwrap();
    // update: is_private flips to false
    repo.upsert_param(env_id, "user", "email", InferredType::Str, false)
        .await
        .unwrap();

    let params = repo.list_params(env_id, "user").await.unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].param_key, "email");
    assert_eq!(params[0].inferred_type, InferredType::Str);
    assert!(!params[0].is_private);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_types_applies_90_day_filter(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool.clone());
    let env_id = EnvironmentId::new();

    // Insert a stale entry directly — last_seen_at older than 90 days
    let stale_ts = Utc::now() - Duration::days(91);
    sqlx::query(
        r#"INSERT INTO context_type_registry (env_id, context_type, first_seen_at, last_seen_at)
           VALUES ($1, $2, $3, $3)"#,
    )
    .bind(env_id.as_uuid())
    .bind("stale_type")
    .bind(stale_ts)
    .execute(&pool)
    .await
    .unwrap();

    repo.upsert_context_type(env_id, "fresh_type").await.unwrap();

    let types = repo.list_types(env_id).await.unwrap();
    assert_eq!(types.len(), 1);
    assert_eq!(types[0].context_type, "fresh_type");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_params_applies_90_day_filter(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool.clone());
    let env_id = EnvironmentId::new();

    repo.upsert_context_type(env_id, "user").await.unwrap();

    let stale_ts = Utc::now() - Duration::days(91);
    sqlx::query(
        r#"INSERT INTO context_param_registry
               (env_id, context_type, param_key, inferred_type, is_private,
                first_seen_at, last_seen_at)
           VALUES ($1, $2, $3, $4, $5, $6, $6)"#,
    )
    .bind(env_id.as_uuid())
    .bind("user")
    .bind("stale_key")
    .bind("str")
    .bind(false)
    .bind(stale_ts)
    .execute(&pool)
    .await
    .unwrap();

    repo.upsert_param(env_id, "user", "fresh_key", InferredType::Int, false)
        .await
        .unwrap();

    let params = repo.list_params(env_id, "user").await.unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].param_key, "fresh_key");
    assert_eq!(params[0].inferred_type, InferredType::Int);
}

#[sqlx::test(migrations = "./migrations")]
async fn purge_stale_removes_old_entries(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool.clone());
    let env_id = EnvironmentId::new();

    let stale_ts = Utc::now() - Duration::days(91);
    sqlx::query(
        r#"INSERT INTO context_type_registry (env_id, context_type, first_seen_at, last_seen_at)
           VALUES ($1, $2, $3, $3)"#,
    )
    .bind(env_id.as_uuid())
    .bind("stale_type")
    .bind(stale_ts)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO context_param_registry
               (env_id, context_type, param_key, inferred_type, is_private,
                first_seen_at, last_seen_at)
           VALUES ($1, $2, $3, $4, $5, $6, $6)"#,
    )
    .bind(env_id.as_uuid())
    .bind("stale_type")
    .bind("stale_key")
    .bind("str")
    .bind(false)
    .bind(stale_ts)
    .execute(&pool)
    .await
    .unwrap();

    repo.upsert_context_type(env_id, "fresh_type").await.unwrap();

    let cutoff = Utc::now() - Duration::days(90);
    repo.purge_stale(cutoff).await.unwrap();

    // list_types with the 90-day filter still excludes the stale entry by timestamp,
    // but purge_stale removes rows entirely — verify via direct count
    let count: (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM context_type_registry WHERE env_id = $1"#)
            .bind(env_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    // Only "fresh_type" survives
    assert_eq!(count.0, 1);

    let types = repo.list_types(env_id).await.unwrap();
    assert_eq!(types[0].context_type, "fresh_type");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_params_all_inferred_types_round_trip(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool);
    let env_id = EnvironmentId::new();

    repo.upsert_context_type(env_id, "ctx").await.unwrap();

    let cases = [
        ("str_key", InferredType::Str),
        ("int_key", InferredType::Int),
        ("double_key", InferredType::Double),
        ("bool_key", InferredType::Bool),
        ("semver_key", InferredType::SemVer),
        ("unknown_key", InferredType::Unknown),
    ];

    for (key, ty) in &cases {
        repo.upsert_param(env_id, "ctx", key, *ty, false)
            .await
            .unwrap();
    }

    let params = repo.list_params(env_id, "ctx").await.unwrap();
    assert_eq!(params.len(), cases.len());

    for param in &params {
        let (_, expected_ty) = cases.iter().find(|(k, _)| *k == param.param_key).unwrap();
        assert_eq!(param.inferred_type, *expected_ty, "type mismatch for {}", param.param_key);
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn list_params_returns_only_matching_context_type(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool);
    let env_id = EnvironmentId::new();

    repo.upsert_context_type(env_id, "user").await.unwrap();
    repo.upsert_context_type(env_id, "session").await.unwrap();
    repo.upsert_param(env_id, "user", "email", InferredType::Str, false)
        .await
        .unwrap();
    repo.upsert_param(env_id, "session", "token", InferredType::Str, true)
        .await
        .unwrap();

    let user_params = repo.list_params(env_id, "user").await.unwrap();
    assert_eq!(user_params.len(), 1);
    assert_eq!(user_params[0].param_key, "email");

    let session_params = repo.list_params(env_id, "session").await.unwrap();
    assert_eq!(session_params.len(), 1);
    assert_eq!(session_params[0].param_key, "token");
    assert!(session_params[0].is_private);
}
