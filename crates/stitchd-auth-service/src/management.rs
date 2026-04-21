//! ManagementService gRPC handler — org/project/environment/SDK-key/user CRUD.

use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use stitchd_core::{
    auth::{OrgRole, crypto::{generate_opaque_token, hash_password}},
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
    CreateUserRequest, CreateUserResponse,
    management_service_server::ManagementService,
};

use crate::sdk_key::hash_sdk_key;

/// tonic gRPC handler for the ManagementService — org/project/environment/SDK-key/user creation.
pub struct ManagementServiceImpl {
    org_repo:        Arc<dyn OrganisationRepository>,
    project_repo:    Arc<dyn ProjectRepository>,
    env_repo:        Arc<dyn EnvironmentRepository>,
    sdk_key_repo:    Arc<dyn SdkKeyRepository>,
    user_repo:       Arc<dyn AuthUserRepository>,
    membership_repo: Arc<dyn OrgMembershipRepository>,
}

impl ManagementServiceImpl {
    #[must_use]
    /// Create a new [`ManagementServiceImpl`].
    pub fn new(
        org_repo:        Arc<dyn OrganisationRepository>,
        project_repo:    Arc<dyn ProjectRepository>,
        env_repo:        Arc<dyn EnvironmentRepository>,
        sdk_key_repo:    Arc<dyn SdkKeyRepository>,
        user_repo:       Arc<dyn AuthUserRepository>,
        membership_repo: Arc<dyn OrgMembershipRepository>,
    ) -> Self {
        Self { org_repo, project_repo, env_repo, sdk_key_repo, user_repo, membership_repo }
    }
}

fn parse_org_id(s: &str) -> Result<OrganisationId, Status> {
    Uuid::parse_str(s)
        .map(OrganisationId::from_uuid)
        .map_err(|_| Status::invalid_argument("org_id is not a valid UUID"))
}

fn map_repo_err(e: RepositoryError) -> Status {
    match e {
        RepositoryError::NotFound { id } => Status::not_found(format!("not found: {id}")),
        RepositoryError::UniqueViolation { .. } => Status::already_exists("resource already exists"),
        RepositoryError::InvalidState { reason } => Status::permission_denied(reason),
        other => Status::internal(other.to_string()),
    }
}

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
        Ok(Response::new(CreateOrgResponse {
            org_id:   org.id.to_string(),
            org_name: org.name,
        }))
    }

    async fn create_project(
        &self,
        request: Request<CreateProjectRequest>,
    ) -> Result<Response<CreateProjectResponse>, Status> {
        let r = request.into_inner();
        let org_id = parse_org_id(&r.org_id)?;
        let org = self.org_repo.find_by_id(org_id).await.map_err(map_repo_err)?;
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
        self.project_repo.create(&project).await.map_err(map_repo_err)?;
        Ok(Response::new(CreateProjectResponse {
            project_id:   project.id.to_string(),
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
            environment_id:   env.id.to_string(),
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
        self.sdk_key_repo.create(&sdk_key).await.map_err(map_repo_err)?;
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
        let org = self.org_repo.find_by_id(org_id).await.map_err(map_repo_err)?;
        guard_not_system_org(&org)?;
        if r.password.is_empty() {
            return Err(Status::invalid_argument("password must not be empty"));
        }
        let hash = hash_password(&r.password).map_err(|e| Status::internal(e.to_string()))?;
        let user = self.user_repo
            .create(&r.email, &r.display_name, Some(&hash))
            .await
            .map_err(map_repo_err)?;

        let role = match r.org_role.as_str() {
            "org_admin" => OrgRole::OrgAdmin,
            _ => OrgRole::OrgMember,
        };
        self.membership_repo
            .add_member(user.id, org_id, role)
            .await
            .map_err(map_repo_err)?;

        Ok(Response::new(CreateUserResponse {
            user_id:      user.id.to_string(),
            email:        user.email,
            display_name: user.display_name,
        }))
    }
}
