//! [`ManagementService`](stitchd_proto::management::v1::management_service_server::ManagementService) gRPC handler — org/project/environment/SDK-key/user CRUD.

use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use stitchd_core::{
    auth::{
        OrgRole,
        crypto::{generate_opaque_token, hash_password},
    },
    id::{EnvironmentId, OrganisationId, ProjectId, SdkKeyId},
    tenant::{Environment, Organisation, Project, SdkKey},
};
use stitchd_db::{
    AuthUserRepository, EnvironmentRepository, OrgMembershipRepository, OrganisationRepository,
    ProjectRepository, RepositoryError, SdkKeyRepository,
};
use stitchd_proto::management::v1::{
    CreateEnvironmentRequest, CreateEnvironmentResponse, CreateOrgRequest, CreateOrgResponse,
    CreateProjectRequest, CreateProjectResponse, CreateSdkKeyRequest, CreateSdkKeyResponse,
    CreateUserRequest, CreateUserResponse, DeleteEnvironmentRequest, DeleteEnvironmentResponse,
    DeleteProjectRequest, DeleteProjectResponse, EnvironmentSummary, ListEnvironmentsRequest,
    ListEnvironmentsResponse, ListOrgsRequest, ListOrgsResponse, ListProjectsRequest,
    ListProjectsResponse, ListSdkKeysRequest, ListSdkKeysResponse, OrgSummary, ProjectSummary,
    RenameEnvironmentRequest, RenameEnvironmentResponse, RenameProjectRequest,
    RenameProjectResponse, RevokeSdkKeyRequest, RevokeSdkKeyResponse, SdkKeySummary,
    management_service_server::ManagementService,
};

use crate::sdk_key::hash_sdk_key;

/// tonic gRPC handler for the [`ManagementService`](stitchd_proto::management::v1::management_service_server::ManagementService) — org/project/environment/SDK-key/user creation.
#[allow(clippy::struct_field_names)]
pub struct ManagementServiceImpl {
    org_repo: Arc<dyn OrganisationRepository>,
    project_repo: Arc<dyn ProjectRepository>,
    env_repo: Arc<dyn EnvironmentRepository>,
    sdk_key_repo: Arc<dyn SdkKeyRepository>,
    user_repo: Arc<dyn AuthUserRepository>,
    membership_repo: Arc<dyn OrgMembershipRepository>,
}

impl ManagementServiceImpl {
    #[must_use]
    /// Create a new [`ManagementServiceImpl`].
    pub fn new(
        org_repo: Arc<dyn OrganisationRepository>,
        project_repo: Arc<dyn ProjectRepository>,
        env_repo: Arc<dyn EnvironmentRepository>,
        sdk_key_repo: Arc<dyn SdkKeyRepository>,
        user_repo: Arc<dyn AuthUserRepository>,
        membership_repo: Arc<dyn OrgMembershipRepository>,
    ) -> Self {
        Self {
            org_repo,
            project_repo,
            env_repo,
            sdk_key_repo,
            user_repo,
            membership_repo,
        }
    }
}

#[allow(clippy::result_large_err)]
fn parse_org_id(s: &str) -> Result<OrganisationId, Status> {
    Uuid::parse_str(s)
        .map(OrganisationId::from_uuid)
        .map_err(|_| Status::invalid_argument("org_id is not a valid UUID"))
}

#[allow(clippy::result_large_err)]
fn parse_project_id(s: &str) -> Result<ProjectId, Status> {
    Uuid::parse_str(s)
        .map(ProjectId::from_uuid)
        .map_err(|_| Status::invalid_argument("project_id is not a valid UUID"))
}

#[allow(clippy::result_large_err)]
fn parse_env_id(s: &str) -> Result<EnvironmentId, Status> {
    Uuid::parse_str(s)
        .map(EnvironmentId::from_uuid)
        .map_err(|_| Status::invalid_argument("environment_id is not a valid UUID"))
}

#[allow(clippy::result_large_err)]
fn parse_sdk_key_id(s: &str) -> Result<SdkKeyId, Status> {
    Uuid::parse_str(s)
        .map(SdkKeyId::from_uuid)
        .map_err(|_| Status::invalid_argument("sdk_key_id is not a valid UUID"))
}

fn map_repo_err(e: RepositoryError) -> Status {
    match e {
        RepositoryError::NotFound { id } => Status::not_found(format!("not found: {id}")),
        RepositoryError::UniqueViolation { .. } => {
            Status::already_exists("resource already exists")
        }
        RepositoryError::InvalidState { reason } => Status::permission_denied(reason),
        other => Status::internal(other.to_string()),
    }
}

#[allow(clippy::result_large_err)]
fn guard_not_system_org(org: &Organisation) -> Result<(), Status> {
    if org.is_system {
        Err(Status::permission_denied(
            "cannot create resources in the System organisation",
        ))
    } else {
        Ok(())
    }
}

#[tonic::async_trait]
impl ManagementService for ManagementServiceImpl {
    async fn create_org(
        &self,
        request: Request<CreateOrgRequest>,
    ) -> Result<Response<CreateOrgResponse>, Status> {
        let r = request.into_inner();
        if r.name.trim().is_empty() {
            return Err(Status::invalid_argument("org name must not be empty"));
        }
        let now = Utc::now();
        let org = Organisation {
            id: OrganisationId::new(),
            name: r.name.trim().to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
            is_system: false,
        };
        self.org_repo.create(&org).await.map_err(map_repo_err)?;

        // Auto-create a default project so new orgs are immediately usable.
        let default_project = Project {
            id: ProjectId::new(),
            organisation_id: org.id,
            name: "Default".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
        };
        self.project_repo
            .create(&default_project)
            .await
            .map_err(map_repo_err)?;

        Ok(Response::new(CreateOrgResponse {
            org_id: org.id.to_string(),
            org_name: org.name,
            project_id: default_project.id.to_string(),
            project_name: default_project.name,
        }))
    }

    async fn list_orgs(
        &self,
        _request: Request<ListOrgsRequest>,
    ) -> Result<Response<ListOrgsResponse>, Status> {
        let all = self.org_repo.list_all().await.map_err(map_repo_err)?;
        let orgs = all
            .into_iter()
            .filter(|o| !o.is_system)
            .map(|o| OrgSummary {
                org_id: o.id.to_string(),
                org_name: o.name,
                created_at: o.created_at.to_rfc3339(),
            })
            .collect();
        Ok(Response::new(ListOrgsResponse { orgs }))
    }

    async fn create_project(
        &self,
        request: Request<CreateProjectRequest>,
    ) -> Result<Response<CreateProjectResponse>, Status> {
        let r = request.into_inner();
        let org_id = parse_org_id(&r.org_id)?;
        let org = self
            .org_repo
            .find_by_id(org_id)
            .await
            .map_err(map_repo_err)?;
        guard_not_system_org(&org)?;
        let now = Utc::now();
        let project = Project {
            id: ProjectId::new(),
            organisation_id: org_id,
            name: r.name.trim().to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
        };
        self.project_repo
            .create(&project)
            .await
            .map_err(map_repo_err)?;
        Ok(Response::new(CreateProjectResponse {
            project_id: project.id.to_string(),
            project_name: project.name,
        }))
    }

    async fn create_environment(
        &self,
        request: Request<CreateEnvironmentRequest>,
    ) -> Result<Response<CreateEnvironmentResponse>, Status> {
        let r = request.into_inner();
        let project_id = ProjectId::from_uuid(
            Uuid::parse_str(&r.project_id)
                .map_err(|_| Status::invalid_argument("project_id is not a valid UUID"))?,
        );
        let now = Utc::now();
        let env = Environment {
            id: EnvironmentId::new(),
            project_id,
            name: r.name.trim().to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
        };
        self.env_repo.create(&env).await.map_err(map_repo_err)?;
        Ok(Response::new(CreateEnvironmentResponse {
            environment_id: env.id.to_string(),
            environment_name: env.name,
        }))
    }

    async fn create_sdk_key(
        &self,
        request: Request<CreateSdkKeyRequest>,
    ) -> Result<Response<CreateSdkKeyResponse>, Status> {
        let r = request.into_inner();
        let env_id = EnvironmentId::from_uuid(
            Uuid::parse_str(&r.environment_id)
                .map_err(|_| Status::invalid_argument("environment_id is not a valid UUID"))?,
        );
        let (raw_key, _) = generate_opaque_token();
        let key_hash = hash_sdk_key(&raw_key);
        let sdk_key = SdkKey {
            id: SdkKeyId::new(),
            environment_id: env_id,
            key_hash,
            is_active: true,
            created_at: Utc::now(),
            revoked_at: None,
        };
        self.sdk_key_repo
            .create(&sdk_key)
            .await
            .map_err(map_repo_err)?;
        Ok(Response::new(CreateSdkKeyResponse {
            sdk_key_id: sdk_key.id.to_string(),
            raw_key,
        }))
    }

    async fn create_user(
        &self,
        request: Request<CreateUserRequest>,
    ) -> Result<Response<CreateUserResponse>, Status> {
        let r = request.into_inner();
        let org_id = parse_org_id(&r.org_id)?;
        let org = self
            .org_repo
            .find_by_id(org_id)
            .await
            .map_err(map_repo_err)?;
        guard_not_system_org(&org)?;

        // Find-or-create: reuse an existing platform user so the same person
        // can be a member of multiple organisations with different roles.
        let user =
            if let Some(existing) = self.user_repo.find_by_email(&r.email).await.map_err(map_repo_err)? {
                // User already exists on the platform — just add the org
                // membership below. No password or profile changes.
                existing
            } else {
                // New user: password is required.
                if r.password.is_empty() {
                    return Err(Status::invalid_argument("password must not be empty for a new user"));
                }
                let hash = hash_password(&r.password).map_err(|e| Status::internal(e.to_string()))?;
                self.user_repo
                    .create(&r.email, &r.display_name, Some(&hash))
                    .await
                    .map_err(map_repo_err)?
            };

        let role = match r.org_role.as_str() {
            "org_admin" => OrgRole::OrgAdmin,
            _ => OrgRole::OrgMember,
        };

        // add_member has a unique constraint on (user_id, org_id) — treat a
        // duplicate as an idempotent success so callers can re-seed safely.
        if let Err(e) = self.membership_repo.add_member(user.id, org_id, role).await {
            if !matches!(e, RepositoryError::UniqueViolation { .. }) {
                return Err(map_repo_err(e));
            }
        }

        Ok(Response::new(CreateUserResponse {
            user_id: user.id.to_string(),
            email: user.email,
            display_name: user.display_name,
        }))
    }

    async fn list_projects(
        &self,
        request: Request<ListProjectsRequest>,
    ) -> Result<Response<ListProjectsResponse>, Status> {
        let r = request.into_inner();
        let org_id = parse_org_id(&r.org_id)?;
        let projects = self
            .project_repo
            .list_by_organisation(org_id)
            .await
            .map_err(map_repo_err)?;
        let summaries = projects
            .into_iter()
            .map(|p| ProjectSummary {
                project_id: p.id.to_string(),
                project_name: p.name,
                created_at: p.created_at.to_rfc3339(),
            })
            .collect();
        Ok(Response::new(ListProjectsResponse { projects: summaries }))
    }

    async fn rename_project(
        &self,
        request: Request<RenameProjectRequest>,
    ) -> Result<Response<RenameProjectResponse>, Status> {
        let r = request.into_inner();
        if r.name.trim().is_empty() {
            return Err(Status::invalid_argument("project name must not be empty"));
        }
        let project_id = parse_project_id(&r.project_id)?;
        let mut project = self
            .project_repo
            .find_by_id(project_id)
            .await
            .map_err(map_repo_err)?;
        project.name = r.name.trim().to_string();
        project.updated_at = Utc::now();
        project.version += 1;
        self.project_repo
            .update(&project)
            .await
            .map_err(map_repo_err)?;
        Ok(Response::new(RenameProjectResponse {}))
    }

    async fn delete_project(
        &self,
        request: Request<DeleteProjectRequest>,
    ) -> Result<Response<DeleteProjectResponse>, Status> {
        let r = request.into_inner();
        let project_id = parse_project_id(&r.project_id)?;
        self.project_repo
            .soft_delete(project_id)
            .await
            .map_err(map_repo_err)?;
        Ok(Response::new(DeleteProjectResponse {}))
    }

    async fn list_environments(
        &self,
        request: Request<ListEnvironmentsRequest>,
    ) -> Result<Response<ListEnvironmentsResponse>, Status> {
        let r = request.into_inner();
        let project_id = parse_project_id(&r.project_id)?;
        let envs = self
            .env_repo
            .list_by_project(project_id)
            .await
            .map_err(map_repo_err)?;
        let summaries = envs
            .into_iter()
            .map(|e| EnvironmentSummary {
                environment_id: e.id.to_string(),
                environment_name: e.name,
                created_at: e.created_at.to_rfc3339(),
            })
            .collect();
        Ok(Response::new(ListEnvironmentsResponse {
            environments: summaries,
        }))
    }

    async fn rename_environment(
        &self,
        request: Request<RenameEnvironmentRequest>,
    ) -> Result<Response<RenameEnvironmentResponse>, Status> {
        let r = request.into_inner();
        if r.name.trim().is_empty() {
            return Err(Status::invalid_argument(
                "environment name must not be empty",
            ));
        }
        let env_id = parse_env_id(&r.environment_id)?;
        let mut env = self.env_repo.find_by_id(env_id).await.map_err(map_repo_err)?;
        env.name = r.name.trim().to_string();
        env.updated_at = Utc::now();
        env.version += 1;
        self.env_repo.update(&env).await.map_err(map_repo_err)?;
        Ok(Response::new(RenameEnvironmentResponse {}))
    }

    async fn delete_environment(
        &self,
        request: Request<DeleteEnvironmentRequest>,
    ) -> Result<Response<DeleteEnvironmentResponse>, Status> {
        let r = request.into_inner();
        let env_id = parse_env_id(&r.environment_id)?;
        self.env_repo
            .soft_delete(env_id)
            .await
            .map_err(map_repo_err)?;
        Ok(Response::new(DeleteEnvironmentResponse {}))
    }

    async fn list_sdk_keys(
        &self,
        request: Request<ListSdkKeysRequest>,
    ) -> Result<Response<ListSdkKeysResponse>, Status> {
        let r = request.into_inner();
        let env_id = parse_env_id(&r.environment_id)?;
        let keys = self
            .sdk_key_repo
            .list_by_environment(env_id)
            .await
            .map_err(map_repo_err)?;
        let summaries = keys
            .into_iter()
            .map(|k| SdkKeySummary {
                sdk_key_id: k.id.to_string(),
                is_active: k.is_active,
                created_at: k.created_at.to_rfc3339(),
                revoked_at: k.revoked_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
            })
            .collect();
        Ok(Response::new(ListSdkKeysResponse { sdk_keys: summaries }))
    }

    async fn revoke_sdk_key(
        &self,
        request: Request<RevokeSdkKeyRequest>,
    ) -> Result<Response<RevokeSdkKeyResponse>, Status> {
        let r = request.into_inner();
        let key_id = parse_sdk_key_id(&r.sdk_key_id)?;
        self.sdk_key_repo.revoke(key_id).await.map_err(|e| match e {
            // min-1-active constraint
            RepositoryError::UniqueViolation { .. } => Status::failed_precondition(
                "cannot revoke the last active SDK key for an environment",
            ),
            other => map_repo_err(other),
        })?;
        Ok(Response::new(RevokeSdkKeyResponse {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;
    use stitchd_core::{
        auth::{OrgMembership, OrgRole, User, UserStatus},
        id::{EnvironmentId, OrganisationId, ProjectId, SdkKeyId, UserId},
        tenant::{Environment, Project, SdkKey},
    };
    use stitchd_db::{
        AuthUserRepository, EnvironmentRepository, OrgMembershipRepository, OrganisationRepository,
        ProjectRepository, RepositoryError, SdkKeyRepository,
    };
    use stitchd_proto::management::v1::{
        CreateOrgRequest, DeleteEnvironmentRequest, DeleteProjectRequest, ListEnvironmentsRequest,
        ListProjectsRequest, ListSdkKeysRequest, RenameEnvironmentRequest, RenameProjectRequest,
        RevokeSdkKeyRequest,
    };
    use tonic::Request;

    // ── Stub repositories ────────────────────────────────────────────────────

    struct StubOrgRepo;

    #[tonic::async_trait]
    impl OrganisationRepository for StubOrgRepo {
        async fn find_by_id(&self, id: OrganisationId) -> Result<Organisation, RepositoryError> {
            Ok(Organisation {
                id,
                name: "stub-org".into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                deleted_at: None,
                version: 1,
                is_system: false,
            })
        }
        async fn list_all(&self) -> Result<Vec<Organisation>, RepositoryError> {
            Ok(vec![])
        }
        async fn create(&self, _: &Organisation) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(&self, org: &Organisation) -> Result<Organisation, RepositoryError> {
            Ok(org.clone())
        }
        async fn soft_delete(&self, id: OrganisationId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
    }

    struct StubProjectRepo {
        projects: Vec<Project>,
    }

    #[tonic::async_trait]
    impl ProjectRepository for StubProjectRepo {
        async fn find_by_id(&self, id: ProjectId) -> Result<Project, RepositoryError> {
            self.projects
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_by_organisation(
            &self,
            org_id: OrganisationId,
        ) -> Result<Vec<Project>, RepositoryError> {
            Ok(self
                .projects
                .iter()
                .filter(|p| p.organisation_id == org_id)
                .cloned()
                .collect())
        }
        async fn create(&self, _: &Project) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(&self, p: &Project) -> Result<Project, RepositoryError> {
            Ok(p.clone())
        }
        async fn soft_delete(&self, _id: ProjectId) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    struct StubEnvRepo {
        envs: Vec<Environment>,
    }

    #[tonic::async_trait]
    impl EnvironmentRepository for StubEnvRepo {
        async fn find_by_id(&self, id: EnvironmentId) -> Result<Environment, RepositoryError> {
            self.envs
                .iter()
                .find(|e| e.id == id)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_by_project(
            &self,
            project_id: ProjectId,
        ) -> Result<Vec<Environment>, RepositoryError> {
            Ok(self
                .envs
                .iter()
                .filter(|e| e.project_id == project_id)
                .cloned()
                .collect())
        }
        async fn create(&self, _: &Environment) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(&self, e: &Environment) -> Result<Environment, RepositoryError> {
            Ok(e.clone())
        }
        async fn soft_delete(&self, _id: EnvironmentId) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    struct StubSdkKeyRepo {
        keys: Vec<SdkKey>,
    }

    #[tonic::async_trait]
    impl SdkKeyRepository for StubSdkKeyRepo {
        async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
            self.keys
                .iter()
                .find(|k| k.id == id)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_by_environment(
            &self,
            environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(self
                .keys
                .iter()
                .filter(|k| k.environment_id == environment_id)
                .cloned()
                .collect())
        }
        async fn create(&self, _: &SdkKey) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
            let active = self.keys.iter().filter(|k| k.is_active).count();
            if active <= 1 {
                return Err(RepositoryError::UniqueViolation {
                    field: "is_active".into(),
                });
            }
            let _ = self
                .keys
                .iter()
                .find(|k| k.id == id)
                .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })?;
            Ok(())
        }
        async fn find_active_by_environment(
            &self,
            environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(self
                .keys
                .iter()
                .filter(|k| k.environment_id == environment_id && k.is_active)
                .cloned()
                .collect())
        }
        async fn find_active_by_hash(&self, hash: &str) -> Result<SdkKey, RepositoryError> {
            self.keys
                .iter()
                .find(|k| k.key_hash == hash && k.is_active)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound { id: hash.into() })
        }
    }

    struct StubUserRepo;

    #[tonic::async_trait]
    impl AuthUserRepository for StubUserRepo {
        async fn create(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<User, RepositoryError> {
            Err(RepositoryError::NotFound { id: "stub".into() })
        }
        async fn find_by_email(&self, _: &str) -> Result<Option<User>, RepositoryError> {
            Ok(None)
        }
        async fn find_by_id(&self, _: UserId) -> Result<Option<User>, RepositoryError> {
            Ok(None)
        }
        async fn rotate_token_secret(&self, id: UserId) -> Result<uuid::Uuid, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn update_status(&self, id: UserId, _: UserStatus) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn update_password_hash(
            &self,
            id: UserId,
            _: &str,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn update_profile(
            &self,
            id: UserId,
            _: &str,
            _: Option<&str>,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_org_users(
            &self,
            _: OrganisationId,
        ) -> Result<Vec<(User, OrgRole)>, RepositoryError> {
            Ok(vec![])
        }
    }

    struct StubMembershipRepo;

    #[tonic::async_trait]
    impl OrgMembershipRepository for StubMembershipRepo {
        async fn add_member(
            &self,
            _: UserId,
            _: OrganisationId,
            _: OrgRole,
        ) -> Result<OrgMembership, RepositoryError> {
            Err(RepositoryError::NotFound { id: "stub".into() })
        }
        async fn find_membership(
            &self,
            _: UserId,
            _: OrganisationId,
        ) -> Result<Option<OrgMembership>, RepositoryError> {
            Ok(None)
        }
        async fn list_orgs_for_user(
            &self,
            _: UserId,
        ) -> Result<Vec<OrgMembership>, RepositoryError> {
            Ok(vec![])
        }
        async fn remove_member(
            &self,
            _: UserId,
            _: OrganisationId,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update_role(
            &self,
            _: UserId,
            _: OrganisationId,
            _: OrgRole,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    // ── Helper ───────────────────────────────────────────────────────────────

    fn make_svc(
        projects: Vec<Project>,
        envs: Vec<Environment>,
        keys: Vec<SdkKey>,
    ) -> ManagementServiceImpl {
        ManagementServiceImpl::new(
            Arc::new(StubOrgRepo),
            Arc::new(StubProjectRepo { projects }),
            Arc::new(StubEnvRepo { envs }),
            Arc::new(StubSdkKeyRepo { keys }),
            Arc::new(StubUserRepo),
            Arc::new(StubMembershipRepo),
        )
    }

    fn make_project(org_id: OrganisationId) -> Project {
        Project {
            id: ProjectId::new(),
            organisation_id: org_id,
            name: "test-project".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    fn make_env(project_id: ProjectId) -> Environment {
        Environment {
            id: EnvironmentId::new(),
            project_id,
            name: "production".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    fn make_sdk_key(env_id: EnvironmentId, active: bool) -> SdkKey {
        SdkKey {
            id: SdkKeyId::new(),
            environment_id: env_id,
            key_hash: uuid::Uuid::new_v4().to_string(),
            is_active: active,
            created_at: Utc::now(),
            revoked_at: None,
        }
    }

    // ── create_org ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_org_also_creates_default_project() {
        let svc = make_svc(vec![], vec![], vec![]);

        let resp = svc
            .create_org(Request::new(CreateOrgRequest {
                name: "Acme".into(),
            }))
            .await
            .unwrap();

        let inner = resp.into_inner();
        assert!(!inner.org_id.is_empty());
        assert_eq!(inner.org_name, "Acme");
        assert!(!inner.project_id.is_empty());
        assert_eq!(inner.project_name, "Default");
    }

    #[tokio::test]
    async fn create_org_empty_name_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .create_org(Request::new(CreateOrgRequest { name: "  ".into() }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ── list_projects ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_projects_returns_projects_for_org() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let svc = make_svc(vec![project.clone()], vec![], vec![]);

        let resp = svc
            .list_projects(Request::new(ListProjectsRequest {
                org_id: org_id.to_string(),
            }))
            .await
            .unwrap();

        let inner = resp.into_inner();
        assert_eq!(inner.projects.len(), 1);
        assert_eq!(inner.projects[0].project_id, project.id.to_string());
        assert_eq!(inner.projects[0].project_name, "test-project");
    }

    #[tokio::test]
    async fn list_projects_invalid_org_id_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .list_projects(Request::new(ListProjectsRequest {
                org_id: "not-a-uuid".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn list_projects_empty_returns_empty_list() {
        let org_id = OrganisationId::new();
        let svc = make_svc(vec![], vec![], vec![]);

        let resp = svc
            .list_projects(Request::new(ListProjectsRequest {
                org_id: org_id.to_string(),
            }))
            .await
            .unwrap();

        assert!(resp.into_inner().projects.is_empty());
    }

    // ── list_environments ────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_environments_returns_envs_for_project() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let env = make_env(project.id);
        let svc = make_svc(vec![project.clone()], vec![env.clone()], vec![]);

        let resp = svc
            .list_environments(Request::new(ListEnvironmentsRequest {
                project_id: project.id.to_string(),
            }))
            .await
            .unwrap();

        let inner = resp.into_inner();
        assert_eq!(inner.environments.len(), 1);
        assert_eq!(inner.environments[0].environment_id, env.id.to_string());
        assert_eq!(inner.environments[0].environment_name, "production");
    }

    #[tokio::test]
    async fn list_environments_invalid_project_id_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .list_environments(Request::new(ListEnvironmentsRequest {
                project_id: "not-a-uuid".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ── list_sdk_keys ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sdk_keys_returns_all_keys_for_environment() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let env = make_env(project.id);
        let active_key = make_sdk_key(env.id, true);
        let revoked_key = make_sdk_key(env.id, false);
        let svc = make_svc(
            vec![project],
            vec![env.clone()],
            vec![active_key.clone(), revoked_key.clone()],
        );

        let resp = svc
            .list_sdk_keys(Request::new(ListSdkKeysRequest {
                environment_id: env.id.to_string(),
            }))
            .await
            .unwrap();

        let inner = resp.into_inner();
        assert_eq!(inner.sdk_keys.len(), 2);
        let ids: Vec<_> = inner.sdk_keys.iter().map(|k| k.sdk_key_id.as_str()).collect();
        assert!(ids.contains(&active_key.id.to_string().as_str()));
        assert!(ids.contains(&revoked_key.id.to_string().as_str()));
    }

    #[tokio::test]
    async fn list_sdk_keys_invalid_env_id_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .list_sdk_keys(Request::new(ListSdkKeysRequest {
                environment_id: "not-a-uuid".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ── revoke_sdk_key ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn revoke_sdk_key_succeeds_when_multiple_active_keys() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let env = make_env(project.id);
        let key1 = make_sdk_key(env.id, true);
        let key2 = make_sdk_key(env.id, true);
        let svc = make_svc(vec![project], vec![env], vec![key1.clone(), key2]);

        let result = svc
            .revoke_sdk_key(Request::new(RevokeSdkKeyRequest {
                sdk_key_id: key1.id.to_string(),
            }))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn revoke_sdk_key_fails_when_last_active_key() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let env = make_env(project.id);
        let key = make_sdk_key(env.id, true);
        let svc = make_svc(vec![project], vec![env], vec![key.clone()]);

        let err = svc
            .revoke_sdk_key(Request::new(RevokeSdkKeyRequest {
                sdk_key_id: key.id.to_string(),
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    }

    #[tokio::test]
    async fn revoke_sdk_key_invalid_id_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .revoke_sdk_key(Request::new(RevokeSdkKeyRequest {
                sdk_key_id: "not-a-uuid".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ── rename_project ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn rename_project_succeeds() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let svc = make_svc(vec![project.clone()], vec![], vec![]);

        let result = svc
            .rename_project(Request::new(RenameProjectRequest {
                project_id: project.id.to_string(),
                name: "new-name".into(),
            }))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rename_project_empty_name_returns_error() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let svc = make_svc(vec![project.clone()], vec![], vec![]);

        let err = svc
            .rename_project(Request::new(RenameProjectRequest {
                project_id: project.id.to_string(),
                name: "   ".into(),
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn rename_project_invalid_id_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .rename_project(Request::new(RenameProjectRequest {
                project_id: "not-a-uuid".into(),
                name: "new-name".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ── delete_project ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_project_succeeds() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let svc = make_svc(vec![project.clone()], vec![], vec![]);

        let result = svc
            .delete_project(Request::new(DeleteProjectRequest {
                project_id: project.id.to_string(),
            }))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_project_invalid_id_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .delete_project(Request::new(DeleteProjectRequest {
                project_id: "not-a-uuid".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ── rename_environment ───────────────────────────────────────────────────

    #[tokio::test]
    async fn rename_environment_succeeds() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let env = make_env(project.id);
        let svc = make_svc(vec![project], vec![env.clone()], vec![]);

        let result = svc
            .rename_environment(Request::new(RenameEnvironmentRequest {
                environment_id: env.id.to_string(),
                name: "staging".into(),
            }))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rename_environment_empty_name_returns_error() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let env = make_env(project.id);
        let svc = make_svc(vec![project], vec![env.clone()], vec![]);

        let err = svc
            .rename_environment(Request::new(RenameEnvironmentRequest {
                environment_id: env.id.to_string(),
                name: "".into(),
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn rename_environment_invalid_id_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .rename_environment(Request::new(RenameEnvironmentRequest {
                environment_id: "not-a-uuid".into(),
                name: "staging".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ── delete_environment ───────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_environment_succeeds() {
        let org_id = OrganisationId::new();
        let project = make_project(org_id);
        let env = make_env(project.id);
        let svc = make_svc(vec![project], vec![env.clone()], vec![]);

        let result = svc
            .delete_environment(Request::new(DeleteEnvironmentRequest {
                environment_id: env.id.to_string(),
            }))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_environment_invalid_id_returns_error() {
        let svc = make_svc(vec![], vec![], vec![]);
        let err = svc
            .delete_environment(Request::new(DeleteEnvironmentRequest {
                environment_id: "not-a-uuid".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }
}
