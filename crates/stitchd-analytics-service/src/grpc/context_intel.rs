//! GetContextIntelligence gRPC handler — returns all context types + their params
//! in a single round-trip (used by the admin UI autocomplete widget).

use std::sync::Arc;

use chrono::{Duration, Utc};
use tonic::{Request, Response, Status};

use stitchd_core::id::EnvironmentId;
use stitchd_db::ContextRegistryRepository;
use stitchd_proto::analytics::v1::{
    ContextIntelligenceType, ContextParamRecord, GetContextIntelligenceRequest,
    GetContextIntelligenceResponse,
};

pub async fn handle_get_context_intelligence(
    repo: &Arc<dyn ContextRegistryRepository>,
    request: Request<GetContextIntelligenceRequest>,
) -> Result<Response<GetContextIntelligenceResponse>, Status> {
    let env_id_str = request.into_inner().environment_id;
    let env_id = env_id_str
        .parse::<uuid::Uuid>()
        .map(EnvironmentId::from_uuid)
        .map_err(|_| Status::invalid_argument(format!("invalid environment_id: {env_id_str}")))?;

    let context_types = repo
        .list_types(env_id)
        .await
        .map_err(|e| Status::internal(format!("context_registry error: {e}")))?;

    let cutoff = Utc::now() - Duration::days(90);

    let mut types = Vec::with_capacity(context_types.len());

    for ct in context_types {
        let params = repo
            .list_params(env_id, &ct.context_type)
            .await
            .map_err(|e| Status::internal(format!("context_registry error: {e}")))?;

        let param_records: Vec<ContextParamRecord> = params
            .into_iter()
            .filter(|p| p.last_seen_at >= cutoff)
            .map(|p| ContextParamRecord {
                param_key: p.param_key,
                inferred_type: p.inferred_type.to_string(),
                is_private: p.is_private,
                last_seen_at: p.last_seen_at.to_rfc3339(),
            })
            .collect();

        types.push(ContextIntelligenceType {
            context_type: ct.context_type,
            last_seen_at: ct.last_seen_at.to_rfc3339(),
            params: param_records,
        });
    }

    Ok(Response::new(GetContextIntelligenceResponse { types }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use chrono::Utc;

    use stitchd_core::context::{ContextParamRecord, ContextTypeRecord, InferredType};
    use stitchd_core::id::EnvironmentId;
    use stitchd_db::{ContextRegistryRepository, RepositoryError};

    use super::*;

    // -----------------------------------------------------------------------
    // Mock
    // -----------------------------------------------------------------------

    struct MockRegistry {
        types: Vec<ContextTypeRecord>,
        params: Vec<ContextParamRecord>,
    }

    #[async_trait]
    impl ContextRegistryRepository for MockRegistry {
        async fn upsert_context_type(
            &self,
            _env_id: EnvironmentId,
            _context_type: &str,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn upsert_param(
            &self,
            _env_id: EnvironmentId,
            _context_type: &str,
            _param_key: &str,
            _inferred_type: InferredType,
            _is_private: bool,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn list_types(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<ContextTypeRecord>, RepositoryError> {
            Ok(self
                .types
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
    // Helpers
    // -----------------------------------------------------------------------

    fn env_id_pair() -> (EnvironmentId, String) {
        let id = EnvironmentId::new();
        (id, id.as_uuid().to_string())
    }

    fn make_type(env_id: EnvironmentId, name: &str) -> ContextTypeRecord {
        let now = Utc::now();
        ContextTypeRecord {
            env_id,
            context_type: name.to_string(),
            first_seen_at: now,
            last_seen_at: now,
        }
    }

    fn make_param(env_id: EnvironmentId, context_type: &str, key: &str) -> ContextParamRecord {
        let now = Utc::now();
        ContextParamRecord {
            env_id,
            context_type: context_type.to_string(),
            param_key: key.to_string(),
            inferred_type: InferredType::Str,
            is_private: false,
            first_seen_at: now,
            last_seen_at: now,
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejects_invalid_env_id() {
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockRegistry {
            types: vec![],
            params: vec![],
        });
        let req = Request::new(GetContextIntelligenceRequest {
            environment_id: "bad-uuid".into(),
        });
        let err = handle_get_context_intelligence(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn returns_empty_for_unknown_env() {
        let (_, env_str) = env_id_pair();
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockRegistry {
            types: vec![],
            params: vec![],
        });
        let req = Request::new(GetContextIntelligenceRequest {
            environment_id: env_str,
        });
        let resp = handle_get_context_intelligence(&repo, req)
            .await
            .unwrap()
            .into_inner();
        assert!(resp.types.is_empty());
    }

    #[tokio::test]
    async fn returns_types_with_params() {
        let (env_id, env_str) = env_id_pair();
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockRegistry {
            types: vec![make_type(env_id, "user"), make_type(env_id, "device")],
            params: vec![
                make_param(env_id, "user", "plan"),
                make_param(env_id, "user", "email"),
                make_param(env_id, "device", "os"),
            ],
        });
        let req = Request::new(GetContextIntelligenceRequest {
            environment_id: env_str,
        });
        let resp = handle_get_context_intelligence(&repo, req)
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.types.len(), 2);

        let user = resp.types.iter().find(|t| t.context_type == "user").unwrap();
        assert_eq!(user.params.len(), 2);

        let device = resp
            .types
            .iter()
            .find(|t| t.context_type == "device")
            .unwrap();
        assert_eq!(device.params.len(), 1);
        assert_eq!(device.params[0].param_key, "os");
    }

    #[tokio::test]
    async fn type_with_no_params_returns_empty_params() {
        let (env_id, env_str) = env_id_pair();
        let repo: Arc<dyn ContextRegistryRepository> = Arc::new(MockRegistry {
            types: vec![make_type(env_id, "org")],
            params: vec![],
        });
        let req = Request::new(GetContextIntelligenceRequest {
            environment_id: env_str,
        });
        let resp = handle_get_context_intelligence(&repo, req)
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.types.len(), 1);
        assert!(resp.types[0].params.is_empty());
    }
}
