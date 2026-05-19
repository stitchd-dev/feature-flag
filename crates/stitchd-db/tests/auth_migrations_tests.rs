/// Integration tests that verify the auth schema migration (20260421000001_auth_schema.sql)
/// applied correctly. Each test uses an isolated database spun up by `#[sqlx::test]`.
///
/// Tests only require a running Postgres; if none is available the test runner will
/// skip them. CI provides the database.

#[sqlx::test(migrations = "./migrations")]
async fn test_auth_tables_exist(pool: sqlx::PgPool) {
    // Verify every auth table is reachable with a simple SELECT.
    let tables = [
        "users",
        "org_memberships",
        "user_project_roles",
        "user_env_roles",
        "refresh_tokens",
        "auth_providers",
        "invites",
        "mfa_challenges",
        "mfa_recovery_codes",
        "password_reset_otps",
    ];

    for table in tables {
        let query = format!("SELECT 1 FROM {table} LIMIT 0");
        sqlx::query(&query)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("table `{table}` not found or inaccessible: {e}"));
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn test_users_email_unique_constraint(pool: sqlx::PgPool) {
    // Insert a user, then attempt to insert another with the same email.
    // The second insert must fail with a unique-violation error.
    sqlx::query("INSERT INTO users (email, display_name) VALUES ('alice@example.com', 'Alice')")
        .execute(&pool)
        .await
        .expect("first user insert should succeed");

    let result = sqlx::query(
        "INSERT INTO users (email, display_name) VALUES ('alice@example.com', 'Alice Duplicate')",
    )
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "duplicate email insert should fail due to UNIQUE constraint on users.email"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_org_memberships_primary_key(pool: sqlx::PgPool) {
    // Set up prerequisite rows.
    let org_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO organisations (name) VALUES ('Test Org') RETURNING id")
            .fetch_one(&pool)
            .await
            .expect("org insert should succeed");

    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, display_name) VALUES ('bob@example.com', 'Bob') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("user insert should succeed");

    // First membership insert should succeed.
    sqlx::query(
        "INSERT INTO org_memberships (user_id, org_id, role) VALUES ($1, $2, 'org_member')",
    )
    .bind(user_id)
    .bind(org_id)
    .execute(&pool)
    .await
    .expect("first org_membership insert should succeed");

    // Second insert with the same (user_id, org_id) PK must fail.
    let result = sqlx::query(
        "INSERT INTO org_memberships (user_id, org_id, role) VALUES ($1, $2, 'org_admin')",
    )
    .bind(user_id)
    .bind(org_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "duplicate (user_id, org_id) insert should fail due to PRIMARY KEY on org_memberships"
    );
}
