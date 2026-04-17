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
