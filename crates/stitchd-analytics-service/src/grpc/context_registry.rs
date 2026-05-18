//! Context registry gRPC handlers — RegisterContext, ListContextTypes, ListContextParams.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use stitchd_core::context::InferredType;
use stitchd_core::id::EnvironmentId;
use stitchd_db::ContextRegistryRepository;
use stitchd_proto::analytics::v1::{
    ContextParamRecord, ContextTypeRecord, ListContextParamsRequest, ListContextParamsResponse,
    ListContextTypesRequest, ListContextTypesResponse, RegisterContextRequest,
    RegisterContextResponse,
};

/// Handle RegisterContext — upserts the context type and each observed parameter.
pub async fn handle_register_context(
    repo: &Arc<dyn ContextRegistryRepository>,
    request: Request<RegisterContextRequest>,
) -> Result<Response<RegisterContextResponse>, Status> {
    let req = request.into_inner();

    let env_id = parse_env_id(&req.environment_id)?;

    // Upsert the context type (fire-and-forget; ignore individual errors so a
    // single param failure doesn't block evaluation logging).
    let repo_clone = Arc::clone(repo);
    let context_type = req.context_type.clone();
    tokio::spawn(async move {
        if let Err(e) = repo_clone.upsert_context_type(env_id, &context_type).await {
            tracing::warn!("context_registry: upsert_context_type failed: {e}");
        }
    });

    for param in req.params {
        let inferred_type = param
            .inferred_type
            .parse::<InferredType>()
            .unwrap_or(InferredType::Str);
        let repo_clone = Arc::clone(repo);
        let context_type = req.context_type.clone();
        tokio::spawn(async move {
            if let Err(e) = repo_clone
                .upsert_param(
                    env_id,
                    &context_type,
                    &param.param_key,
                    inferred_type,
                    param.is_private,
                )
                .await
            {
                tracing::warn!("context_registry: upsert_param failed: {e}");
            }
        });
    }

    Ok(Response::new(RegisterContextResponse {}))
}

/// Handle ListContextTypes.
pub async fn handle_list_context_types(
    repo: &Arc<dyn ContextRegistryRepository>,
    request: Request<ListContextTypesRequest>,
) -> Result<Response<ListContextTypesResponse>, Status> {
    let env_id = parse_env_id(&request.into_inner().environment_id)?;

    let records = repo
        .list_types(env_id)
        .await
        .map_err(|e| Status::internal(format!("context_registry error: {e}")))?;

    let types = records
        .into_iter()
        .map(|r| ContextTypeRecord {
            context_type: r.context_type,
            last_seen_at: r.last_seen_at.to_rfc3339(),
        })
        .collect();

    Ok(Response::new(ListContextTypesResponse { types }))
}

/// Handle ListContextParams.
pub async fn handle_list_context_params(
    repo: &Arc<dyn ContextRegistryRepository>,
    request: Request<ListContextParamsRequest>,
) -> Result<Response<ListContextParamsResponse>, Status> {
    let req = request.into_inner();
    let env_id = parse_env_id(&req.environment_id)?;

    let records = repo
        .list_params(env_id, &req.context_type)
        .await
        .map_err(|e| Status::internal(format!("context_registry error: {e}")))?;

    let params = records
        .into_iter()
        .map(|r| ContextParamRecord {
            param_key: r.param_key,
            inferred_type: r.inferred_type.to_string(),
            is_private: r.is_private,
            last_seen_at: r.last_seen_at.to_rfc3339(),
        })
        .collect();

    Ok(Response::new(ListContextParamsResponse { params }))
}

#[allow(clippy::result_large_err)] // tonic::Status is large; we can't shrink an external type
fn parse_env_id(s: &str) -> Result<EnvironmentId, Status> {
    s.parse::<uuid::Uuid>()
        .map(EnvironmentId::from_uuid)
        .map_err(|_| Status::invalid_argument(format!("invalid environment_id: {s}")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::Utc;
    use stitchd_core::{
        context::{ContextParamRecord, ContextTypeRecord, InferredType},
        id::EnvironmentId,
    };
    use stitchd_db::{ContextRegistryRepository, RepositoryError};
    use stitchd_proto::analytics::v1::ContextParam;

    use super::*;

    // -----------------------------------------------------------------------
    // Mock ContextRegistryRepository
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockContextRegistry {
        types: Mutex<Vec<ContextTypeRecord>>,
        params: Mutex<Vec<ContextParamRecord>>,
        upsert_type_calls: Mutex<Vec<(EnvironmentId, String)>>,
        upsert_param_calls: Mutex<Vec<(EnvironmentId, String, String)>>,
    }

    impl MockContextRegistry {
        fn new_with_types(env_id: EnvironmentId, type_names: Vec<&str>) -> Self {
            let now = Utc::now();
            let types = type_names
                .into_iter()
                .map(|t| ContextTypeRecord {
                    env_id,
                    context_type: t.to_string(),
                    first_seen_at: now,
                    last_seen_at: now,
                })
                .collect();
            Self {
                types: Mutex::new(types),
                ..Default::default()
            }
        }

        fn new_with_params(env_id: EnvironmentId, params: Vec<(&str, &str)>) -> Self {
            let now = Utc::now();
            let records = params
                .into_iter()
                .map(|(key, context_type)| ContextParamRecord {
                    env_id,
                    context_type: context_type.to_string(),
                    param_key: key.to_string(),
                    inferred_type: InferredType::Str,
                    is_private: false,
                    first_seen_at: now,
                    last_seen_at: now,
                })
                .collect();
            Self {
                params: Mutex::new(records),
                ..Default::default()
            }
        }
    }

    #[async_trait]
    impl ContextRegistryRepository for MockContextRegistry {
        async fn upsert_context_type(
            &self,
            env_id: EnvironmentId,
            context_type: &str,
        ) -> Result<(), RepositoryError> {
            self.upsert_type_calls
                .lock()
                .unwrap()
                .push((env_id, context_type.to_string()));
            Ok(())
        }

        async fn upsert_param(
            &self,
            env_id: EnvironmentId,
            context_type: &str,
            param_key: &str,
            _inferred_type: InferredType,
            _is_private: bool,
        ) -> Result<(), RepositoryError> {
            self.upsert_param_calls.lock().unwrap().push((
                env_id,
                context_type.to_string(),
                param_key.to_string(),
            ));
            Ok(())
        }

        async fn list_types(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<ContextTypeRecord>, RepositoryError> {
            Ok(self
                .types
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.env_id == env_id)
                .cloned()
                .collect())
        }

        async fn list_params(
            &self,
            env_id: EnvironmentId,
            context_type: &str,
        ) -> Result<Vec<ContextParamRecord>, RepositoryError> {
            Ok(self
                .params
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.env_id == env_id && r.context_type == context_type)
                .cloned()
                .collect())
        }

        async fn purge_stale(
            &self,
            _older_than: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helper
    // -----------------------------------------------------------------------

    fn env_id_str() -> (EnvironmentId, String) {
        let id = EnvironmentId::new();
        let s = id.as_uuid().to_string();
        (id, s)
    }

    // -----------------------------------------------------------------------
    // RegisterContext
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn register_context_rejects_invalid_env_id() {
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockContextRegistry::default());
        let req = Request::new(RegisterContextRequest {
            environment_id: "not-a-uuid".into(),
            context_type: "user".into(),
            context_key: "u1".into(),
            params: vec![],
        });
        let err = handle_register_context(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn register_context_returns_ok() {
        let (_, env_str) = env_id_str();
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockContextRegistry::default());
        let req = Request::new(RegisterContextRequest {
            environment_id: env_str,
            context_type: "user".into(),
            context_key: "u1".into(),
            params: vec![ContextParam {
                param_key: "plan".into(),
                inferred_type: "string".into(),
                is_private: false,
            }],
        });
        let resp = handle_register_context(&repo, req).await;
        assert!(resp.is_ok());
    }

    // -----------------------------------------------------------------------
    // ListContextTypes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_context_types_returns_types() {
        let (env_id, env_str) = env_id_str();
        let repo: Arc<dyn ContextRegistryRepository> =
            Arc::new(MockContextRegistry::new_with_types(
                env_id,
                vec!["user", "device"],
            ));
        let req = Request::new(ListContextTypesRequest {
            environment_id: env_str,
        });
        let resp = handle_list_context_types(&repo, req).await.unwrap();
        let body = resp.into_inner();
        let names: Vec<_> = body.types.iter().map(|t| t.context_type.as_str()).collect();
        assert!(names.contains(&"user"));
        assert!(names.contains(&"device"));
    }

    #[tokio::test]
    async fn list_context_types_rejects_invalid_env_id() {
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockContextRegistry::default());
        let req = Request::new(ListContextTypesRequest {
            environment_id: "bad-uuid".into(),
        });
        let err = handle_list_context_types(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn list_context_types_empty_env() {
        let (_, env_str) = env_id_str();
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockContextRegistry::default());
        let req = Request::new(ListContextTypesRequest {
            environment_id: env_str,
        });
        let resp = handle_list_context_types(&repo, req).await.unwrap();
        assert!(resp.into_inner().types.is_empty());
    }

    // -----------------------------------------------------------------------
    // ListContextParams
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_context_params_returns_params() {
        let (env_id, env_str) = env_id_str();
        let repo: Arc<dyn ContextRegistryRepository> =
            Arc::new(MockContextRegistry::new_with_params(
                env_id,
                vec![("plan", "user"), ("email", "user")],
            ));
        let req = Request::new(ListContextParamsRequest {
            environment_id: env_str,
            context_type: "user".into(),
        });
        let resp = handle_list_context_params(&repo, req).await.unwrap();
        let body = resp.into_inner();
        let keys: Vec<_> = body.params.iter().map(|p| p.param_key.as_str()).collect();
        assert!(keys.contains(&"plan"));
        assert!(keys.contains(&"email"));
    }

    #[tokio::test]
    async fn list_context_params_filters_by_type() {
        let (env_id, env_str) = env_id_str();
        let repo: Arc<dyn ContextRegistryRepository> =
            Arc::new(MockContextRegistry::new_with_params(
                env_id,
                vec![("device_model", "device"), ("plan", "user")],
            ));
        let req = Request::new(ListContextParamsRequest {
            environment_id: env_str,
            context_type: "device".into(),
        });
        let resp = handle_list_context_params(&repo, req).await.unwrap();
        let body = resp.into_inner();
        assert_eq!(body.params.len(), 1);
        assert_eq!(body.params[0].param_key, "device_model");
    }

    #[tokio::test]
    async fn list_context_params_rejects_invalid_env_id() {
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockContextRegistry::default());
        let req = Request::new(ListContextParamsRequest {
            environment_id: "not-uuid".into(),
            context_type: "user".into(),
        });
        let err = handle_list_context_params(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }
}
