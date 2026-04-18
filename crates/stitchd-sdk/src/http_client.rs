//! HTTP client for the list-check REST endpoints.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::SdkError;

#[derive(Serialize)]
struct ListCheckRequest<'a> {
    context_type: &'a str,
    context_key: &'a str,
    segment_keys: &'a [String],
}

#[derive(Deserialize)]
struct ListCheckResponse {
    memberships: HashMap<String, bool>,
}

#[derive(Serialize)]
struct BatchCtx<'a> {
    context_type: &'a str,
    context_key: &'a str,
}

#[derive(Serialize)]
struct BatchListCheckRequest<'a> {
    contexts: Vec<BatchCtx<'a>>,
    segment_keys: &'a [String],
}

#[derive(Deserialize)]
struct BatchContextMembership {
    context_type: String,
    context_key: String,
    memberships: HashMap<String, bool>,
}

#[derive(Deserialize)]
struct BatchListCheckResponse {
    results: Vec<BatchContextMembership>,
}

pub struct SdkHttpClient {
    http_url: String,
    sdk_key: String,
    inner: reqwest::Client,
}

impl SdkHttpClient {
    pub fn new(http_url: impl Into<String>, sdk_key: impl Into<String>) -> Self {
        Self {
            http_url: http_url.into(),
            sdk_key: sdk_key.into(),
            inner: reqwest::Client::new(),
        }
    }

    /// Check membership for one context type and multiple segment keys.
    ///
    /// Returns a map of segment_key → is_member.
    pub async fn list_check(
        &self,
        environment_id: &str,
        context_type: &str,
        context_key: &str,
        segment_keys: &[String],
    ) -> Result<HashMap<String, bool>, SdkError> {
        if segment_keys.is_empty() {
            return Ok(HashMap::new());
        }

        let url = format!(
            "{}/v1/environments/{}/segments/list-check",
            self.http_url, environment_id
        );

        let body = ListCheckRequest {
            context_type,
            context_key,
            segment_keys,
        };
        let resp = self
            .inner
            .post(&url)
            .header("x-sdk-key", &self.sdk_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<ListCheckResponse>()
            .await?;

        Ok(resp.memberships)
    }

    /// Check membership for multiple contexts and multiple segment keys in one call.
    ///
    /// Returns `(context_type, context_key, segment_key) → is_member`.
    pub async fn list_check_batch(
        &self,
        environment_id: &str,
        contexts: &[(String, String)],
        segment_keys: &[String],
    ) -> Result<HashMap<(String, String, String), bool>, SdkError> {
        if contexts.is_empty() || segment_keys.is_empty() {
            return Ok(HashMap::new());
        }

        let url = format!(
            "{}/v1/environments/{}/segments/list-check/batch",
            self.http_url, environment_id
        );

        let ctx_refs: Vec<BatchCtx<'_>> = contexts
            .iter()
            .map(|(ct, ck)| BatchCtx {
                context_type: ct.as_str(),
                context_key: ck.as_str(),
            })
            .collect();

        let body = BatchListCheckRequest {
            contexts: ctx_refs,
            segment_keys,
        };

        let resp = self
            .inner
            .post(&url)
            .header("x-sdk-key", &self.sdk_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<BatchListCheckResponse>()
            .await?;

        let mut out = HashMap::new();
        for entry in resp.results {
            for (seg_key, is_member) in entry.memberships {
                out.insert(
                    (
                        entry.context_type.clone(),
                        entry.context_key.clone(),
                        seg_key,
                    ),
                    is_member,
                );
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn list_check_empty_keys_returns_empty_map() {
        let client = SdkHttpClient::new("http://localhost:9999", "test-key");
        let result = client.list_check("env-1", "user", "u1", &[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_check_batch_empty_contexts_returns_empty_map() {
        let client = SdkHttpClient::new("http://localhost:9999", "test-key");
        let result = client
            .list_check_batch("env-1", &[], &["seg-a".to_string()])
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_check_batch_empty_keys_returns_empty_map() {
        let client = SdkHttpClient::new("http://localhost:9999", "test-key");
        let result = client
            .list_check_batch("env-1", &[("user".to_string(), "u1".to_string())], &[])
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_check_returns_membership_map() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/environments/env-123/segments/list-check"))
            .and(header("x-sdk-key", "sdk-key-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "memberships": {
                    "beta-users": true,
                    "premium": false
                }
            })))
            .mount(&server)
            .await;

        let client = SdkHttpClient::new(server.uri(), "sdk-key-abc");
        let keys = vec!["beta-users".to_string(), "premium".to_string()];
        let result = client
            .list_check("env-123", "user", "u1", &keys)
            .await
            .unwrap();

        assert_eq!(result.get("beta-users"), Some(&true));
        assert_eq!(result.get("premium"), Some(&false));
    }

    #[tokio::test]
    async fn list_check_http_error_returns_sdk_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/environments/env-123/segments/list-check"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = SdkHttpClient::new(server.uri(), "invalid-key");
        let keys = vec!["beta-users".to_string()];
        let result = client.list_check("env-123", "user", "u1", &keys).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_check_batch_returns_flattened_map() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/environments/env-123/segments/list-check/batch"))
            .and(header("x-sdk-key", "sdk-key-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": [
                    {
                        "context_type": "user",
                        "context_key": "u1",
                        "memberships": {
                            "beta-users": true
                        }
                    },
                    {
                        "context_type": "user",
                        "context_key": "u2",
                        "memberships": {
                            "beta-users": false
                        }
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = SdkHttpClient::new(server.uri(), "sdk-key-abc");
        let contexts = vec![
            ("user".to_string(), "u1".to_string()),
            ("user".to_string(), "u2".to_string()),
        ];
        let keys = vec!["beta-users".to_string()];
        let result = client
            .list_check_batch("env-123", &contexts, &keys)
            .await
            .unwrap();

        assert_eq!(
            result.get(&(
                "user".to_string(),
                "u1".to_string(),
                "beta-users".to_string()
            )),
            Some(&true)
        );
        assert_eq!(
            result.get(&(
                "user".to_string(),
                "u2".to_string(),
                "beta-users".to_string()
            )),
            Some(&false)
        );
    }

    #[tokio::test]
    async fn list_check_batch_http_error_returns_sdk_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/environments/env-123/segments/list-check/batch"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = SdkHttpClient::new(server.uri(), "sdk-key-abc");
        let contexts = vec![("user".to_string(), "u1".to_string())];
        let keys = vec!["beta-users".to_string()];
        let result = client.list_check_batch("env-123", &contexts, &keys).await;

        assert!(result.is_err());
    }

    #[test]
    fn sdk_http_client_constructor_sets_fields() {
        let client = SdkHttpClient::new("http://api:8080", "my-key");
        assert_eq!(client.http_url, "http://api:8080");
        assert_eq!(client.sdk_key, "my-key");
    }
}
