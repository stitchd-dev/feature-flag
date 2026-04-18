//! Postgres implementation for the `role` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    id::{ProjectId, RoleId},
    user::{Action, Permission, ResourceType, Role},
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, RoleRepository},
};

/// Postgres-backed implementation of [`RoleRepository`].
pub struct PgRoleRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgRoleRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Fetch all permissions for a role from the `permissions` table.
async fn fetch_permissions(
    pool: &PgPool,
    role_id: RoleId,
) -> Result<Vec<Permission>, RepositoryError> {
    sqlx::query!(
        r#"
        SELECT resource_type, resource_pattern, action
        FROM permissions
        WHERE role_id = $1
        "#,
        role_id as RoleId
    )
    .fetch_all(pool)
    .await
    .map_err(RepositoryError::Database)?
    .into_iter()
    .map(|row| {
        let resource_type = parse_resource_type(&row.resource_type)?;
        let action = parse_action(&row.action)?;
        Ok(Permission {
            resource_type,
            resource_pattern: row.resource_pattern,
            action,
        })
    })
    .collect()
}

/// Delete all permissions for `role_id` then re-insert `permissions`.
async fn replace_permissions(
    pool: &PgPool,
    role_id: RoleId,
    permissions: &[Permission],
) -> Result<(), RepositoryError> {
    sqlx::query!(
        "DELETE FROM permissions WHERE role_id = $1",
        role_id as RoleId
    )
    .execute(pool)
    .await
    .map_err(RepositoryError::Database)?;

    for perm in permissions {
        sqlx::query!(
            r#"
            INSERT INTO permissions (role_id, resource_type, resource_pattern, action)
            VALUES ($1, $2, $3, $4)
            "#,
            role_id as RoleId,
            resource_type_to_str(perm.resource_type),
            perm.resource_pattern,
            action_to_str(perm.action),
        )
        .execute(pool)
        .await
        .map_err(RepositoryError::Database)?;
    }

    Ok(())
}

const fn resource_type_to_str(rt: ResourceType) -> &'static str {
    match rt {
        ResourceType::Environment => "environment",
        ResourceType::Flag => "flag",
        ResourceType::Segment => "segment",
    }
}

const fn action_to_str(a: Action) -> &'static str {
    match a {
        Action::Read => "read",
        Action::Write => "write",
        Action::Publish => "publish",
        Action::Admin => "admin",
    }
}

fn parse_resource_type(s: &str) -> Result<ResourceType, RepositoryError> {
    match s {
        "environment" => Ok(ResourceType::Environment),
        "flag" => Ok(ResourceType::Flag),
        "segment" => Ok(ResourceType::Segment),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown resource_type: {other}"
        ))),
    }
}

fn parse_action(s: &str) -> Result<Action, RepositoryError> {
    match s {
        "read" => Ok(Action::Read),
        "write" => Ok(Action::Write),
        "publish" => Ok(Action::Publish),
        "admin" => Ok(Action::Admin),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown action: {other}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Unit tests for pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn resource_type_to_str_all_variants() {
        assert_eq!(
            resource_type_to_str(ResourceType::Environment),
            "environment"
        );
        assert_eq!(resource_type_to_str(ResourceType::Flag), "flag");
        assert_eq!(resource_type_to_str(ResourceType::Segment), "segment");
    }

    #[test]
    fn action_to_str_all_variants() {
        assert_eq!(action_to_str(Action::Read), "read");
        assert_eq!(action_to_str(Action::Write), "write");
        assert_eq!(action_to_str(Action::Publish), "publish");
        assert_eq!(action_to_str(Action::Admin), "admin");
    }

    #[test]
    fn parse_resource_type_all_variants() {
        assert!(matches!(
            parse_resource_type("environment").unwrap(),
            ResourceType::Environment
        ));
        assert!(matches!(
            parse_resource_type("flag").unwrap(),
            ResourceType::Flag
        ));
        assert!(matches!(
            parse_resource_type("segment").unwrap(),
            ResourceType::Segment
        ));
    }

    #[test]
    fn parse_resource_type_unknown_returns_error() {
        let err = parse_resource_type("other").unwrap_err();
        assert!(err.to_string().contains("unknown resource_type: other"));
    }

    #[test]
    fn parse_action_all_variants() {
        assert!(matches!(parse_action("read").unwrap(), Action::Read));
        assert!(matches!(parse_action("write").unwrap(), Action::Write));
        assert!(matches!(parse_action("publish").unwrap(), Action::Publish));
        assert!(matches!(parse_action("admin").unwrap(), Action::Admin));
    }

    #[test]
    fn parse_action_unknown_returns_error() {
        let err = parse_action("delete").unwrap_err();
        assert!(err.to_string().contains("unknown action: delete"));
    }

    #[test]
    fn resource_type_roundtrips() {
        for rt in [
            ResourceType::Environment,
            ResourceType::Flag,
            ResourceType::Segment,
        ] {
            let s = resource_type_to_str(rt);
            let parsed = parse_resource_type(s).unwrap();
            assert_eq!(parsed, rt, "roundtrip failed for {s}");
        }
    }

    #[test]
    fn action_roundtrips() {
        for action in [Action::Read, Action::Write, Action::Publish, Action::Admin] {
            let s = action_to_str(action);
            let parsed = parse_action(s).unwrap();
            assert_eq!(parsed, action, "roundtrip failed for {s}");
        }
    }
}

// ---------------------------------------------------------------------------
// RoleRepository impl
// ---------------------------------------------------------------------------

#[async_trait]
impl RoleRepository for PgRoleRepository {
    async fn find_by_id(&self, id: RoleId) -> Result<Role, RepositoryError> {
        let row = sqlx::query!(
            r#"
            SELECT
                id         AS "id: RoleId",
                project_id AS "project_id: ProjectId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            FROM roles
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as RoleId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })?;

        let permissions = fetch_permissions(&self.pool, id).await?;
        Ok(Role {
            id: row.id,
            project_id: row.project_id,
            name: row.name,
            permissions,
            created_at: row.created_at,
            updated_at: row.updated_at,
            deleted_at: row.deleted_at,
            version: row.version,
        })
    }

    async fn list_by_project(&self, project_id: ProjectId) -> Result<Vec<Role>, RepositoryError> {
        let rows = sqlx::query!(
            r#"
            SELECT
                id         AS "id: RoleId",
                project_id AS "project_id: ProjectId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            FROM roles
            WHERE project_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            project_id as ProjectId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut roles = Vec::with_capacity(rows.len());
        for row in rows {
            let permissions = fetch_permissions(&self.pool, row.id).await?;
            roles.push(Role {
                id: row.id,
                project_id: row.project_id,
                name: row.name,
                permissions,
                created_at: row.created_at,
                updated_at: row.updated_at,
                deleted_at: row.deleted_at,
                version: row.version,
            });
        }
        Ok(roles)
    }

    async fn create(&self, role: &Role) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO roles
                (id, project_id, name, created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
            role.id as RoleId,
            role.project_id as ProjectId,
            role.name,
            role.created_at,
            role.updated_at,
            role.deleted_at,
            role.version,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref dbe) = e {
                if let Some(constraint) = dbe.constraint() {
                    return RepositoryError::UniqueViolation {
                        field: constraint.to_string(),
                    };
                }
            }
            RepositoryError::Database(e)
        })?;

        replace_permissions(&self.pool, role.id, &role.permissions).await?;

        self.audit
            .log(
                None,
                "role",
                role.id.as_uuid(),
                "create",
                serde_json::json!({
                    "name": role.name,
                    "project_id": role.project_id.to_string(),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, role: &Role) -> Result<Role, RepositoryError> {
        let new_version = role.version + 1;
        let result = sqlx::query!(
            r#"
            UPDATE roles
            SET name = $1, updated_at = NOW(), version = $2
            WHERE id = $3 AND version = $4 AND deleted_at IS NULL
            RETURNING
                id         AS "id: RoleId",
                project_id AS "project_id: ProjectId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            "#,
            role.name,
            new_version,
            role.id as RoleId,
            role.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(row) = result {
            replace_permissions(&self.pool, role.id, &role.permissions).await?;
            self.audit
                .log(
                    None,
                    "role",
                    role.id.as_uuid(),
                    "update",
                    serde_json::json!({ "name": role.name }),
                )
                .await?;
            return Ok(Role {
                id: row.id,
                project_id: row.project_id,
                name: row.name,
                permissions: role.permissions.clone(),
                created_at: row.created_at,
                updated_at: row.updated_at,
                deleted_at: row.deleted_at,
                version: row.version,
            });
        }

        let current = sqlx::query!(
            r#"SELECT version, deleted_at FROM roles WHERE id = $1"#,
            role.id as RoleId
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        match current {
            None => Err(RepositoryError::NotFound {
                id: role.id.to_string(),
            }),
            Some(row) if row.deleted_at.is_some() => Err(RepositoryError::NotFound {
                id: role.id.to_string(),
            }),
            Some(row) => Err(RepositoryError::VersionConflict {
                expected: role.version,
                actual: row.version,
            }),
        }
    }

    async fn soft_delete(&self, id: RoleId) -> Result<(), RepositoryError> {
        let result = sqlx::query!(
            r#"
            UPDATE roles
            SET deleted_at = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as RoleId,
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        }

        self.audit
            .log(
                None,
                "role",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}
