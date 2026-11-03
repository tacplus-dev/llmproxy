//! Google (#13): Gemini through Vertex AI's `generateContent`, text in and
//! text out, and the publisher model listing `latest` routes resolve from.
//!
//! The credential is GKE Workload Identity, never a key (Jody, 2026-10-31):
//! the pod's Kubernetes service account is bound to a Google service account,
//! and the GKE metadata server hands out that account's short-lived access
//! token. [`Federation`] caches it and asks again before it expires.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use llmproxy_model::types::services::llmproxy::llmproxy::{
    generate_completion_response::Result as GenResult, GenerateCompletionRequest, GenerateCompletionResponse,
    Usage as ProtoUsage, Vendor,
};

use crate::federation::{read_issued, Exchange, Federation, Issued};
use crate::providers::{looks_preview, send_json, text_only, ListedModel, ProviderError};

/// The metadata server; `GCE_METADATA_HOST` moves it, as in Google's own clients.
pub const METADATA_BASE: &str = "http://metadata.google.internal";
const TOKEN_PATH: &str = "/computeMetadata/v1/instance/service-accounts/default/token";

/// The pod's Google access token from the metadata server.
pub struct MetadataToken {
    pub base_url: String,
}

#[tonic::async_trait]
impl Exchange for MetadataToken {
    async fn exchange(&self, http: &reqwest::Client) -> Result<Issued, String> {
        // trace: unrelated: the token is cached and shared across requests
        let resp = http
            .get(format!("{}{TOKEN_PATH}", self.base_url))
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| format!("metadata server unreachable: {e}"))?;
        read_issued(resp).await
    }
}

/// The Vertex AI host for a location: `global` has no regional prefix.
pub fn vertex_base_url(location: &str) -> String {
    if location == "global" {
        "https://aiplatform.googleapis.com".to_string()
    } else {
        format!("https://{location}-aiplatform.googleapis.com")
    }
}

pub struct VertexClient {
    http: reqwest::Client,
    base_url: String,
    project: String,
    location: String,
    federation: Arc<Federation>,
}

impl VertexClient {
    pub fn new(base_url: Option<String>, project: String, location: String, federation: Arc<Federation>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.filter(|s| !s.is_empty()).unwrap_or_else(|| vertex_base_url(&location)),
            project,
            location,
            federation,
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<serde_json::Value, ProviderError> {
        let token = self.federation.token().await.map_err(ProviderError::Credential)?;
        send_json(Vendor::Google, request.bearer_auth(token)).await
    }

    /// Every Google publisher model, all pages. A model whose launch stage is
    /// anything but GA is a preview, as is one whose id says so.
    pub async fn list_models(&self) -> Result<Vec<ListedModel>, ProviderError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Model {
            name: String,
            #[serde(default)]
            launch_stage: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Page {
            #[serde(default)]
            publisher_models: Vec<Model>,
            #[serde(default)]
            next_page_token: Option<String>,
        }

        let mut models = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![("pageSize", "1000".to_string())];
            if let Some(token) = &page_token {
                query.push(("pageToken", token.clone()));
            }
            // trace: unrelated: the listing is cached and shared across requests
            let body = self
                .send(
                    self.http
                        .get(format!("{}/v1beta1/publishers/google/models", self.base_url))
                        .query(&query),
                )
                .await?;
            let page: Page = serde_json::from_value(body)
                .map_err(|e| ProviderError::UnexpectedResponseShape(Vendor::Google, format!("model listing: {e}")))?;
            models.extend(page.publisher_models.into_iter().map(|m| {
                // "publishers/google/models/gemini-2.5-pro"
                let id = m.name.rsplit('/').next().unwrap_or_default().to_string();
                let preview = looks_preview(&id) || m.launch_stage.as_deref().is_some_and(|s| s != "GA");
                ListedModel { id, preview }
            }));
            match page.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => return Ok(models),
            }
        }
    }

    /// One `generateContent` call.
    pub async fn complete(
        &self,
        model: &str,
        req: &GenerateCompletionRequest,
    ) -> Result<GenerateCompletionResponse, ProviderError> {
        let url = format!(
            "{}/v1/projects/{}/locations/{}/publishers/google/models/{model}:generateContent",
            self.base_url, self.project, self.location
        );
        let body = build_request_body(req)?;
        // trace: unrelated: leaves the fleet; vendor APIs carry no trace_id (README, Vendors)
        parse_response(&self.send(self.http.post(url).json(&body)).await?)
    }
}

#[derive(Serialize)]
struct Part {
    text: String,
}

fn build_request_body(req: &GenerateCompletionRequest) -> Result<serde_json::Value, ProviderError> {
    #[derive(Serialize)]
    struct Content {
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<&'static str>,
        parts: Vec<Part>,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct GenerationConfig {
        max_output_tokens: i32,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Request {
        contents: Vec<Content>,
        #[serde(skip_serializing_if = "Option::is_none")]
        system_instruction: Option<Content>,
        generation_config: GenerationConfig,
    }

    let contents = text_only(Vendor::Google, req)?
        .into_iter()
        .map(|m| Content {
            role: Some(if m.assistant { "model" } else { "user" }),
            parts: m.texts.into_iter().map(|text| Part { text }).collect(),
        })
        .collect();
    let system_instruction = (!req.system.is_empty()).then(|| Content {
        role: None,
        parts: vec![Part { text: req.system.clone() }],
    });
    Ok(serde_json::to_value(Request {
        contents,
        system_instruction,
        generation_config: GenerationConfig {
            max_output_tokens: crate::anthropic::output_budget(req.max_tokens),
        },
    })
    .expect("Request serializes: every field type has a Serialize impl"))
}

/// The first candidate's text parts, joined; a thinking model's thought parts
/// are not the answer and are skipped.
fn parse_response(body: &serde_json::Value) -> Result<GenerateCompletionResponse, ProviderError> {
    #[derive(Deserialize)]
    struct WirePart {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        thought: bool,
    }
    #[derive(Deserialize)]
    struct WireContent {
        #[serde(default)]
        parts: Vec<WirePart>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Candidate {
        #[serde(default)]
        content: Option<WireContent>,
        #[serde(default)]
        finish_reason: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WireUsage {
        #[serde(default)]
        prompt_token_count: i64,
        #[serde(default)]
        candidates_token_count: i64,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Response {
        #[serde(default)]
        candidates: Vec<Candidate>,
        #[serde(default)]
        usage_metadata: Option<WireUsage>,
    }

    let parsed: Response = serde_json::from_value(body.clone())
        .map_err(|e| ProviderError::UnexpectedResponseShape(Vendor::Google, format!("{e}: {body}")))?;
    let candidate = parsed.candidates.into_iter().next();
    let finish_reason = candidate.as_ref().and_then(|c| c.finish_reason.clone());
    let text: String = candidate
        .and_then(|c| c.content)
        .map(|c| c.parts.into_iter().filter(|p| !p.thought).filter_map(|p| p.text).collect())
        .unwrap_or_default();
    if text.is_empty() {
        return Err(ProviderError::UnexpectedResponseShape(
            Vendor::Google,
            format!(
                "no text in the first candidate (finishReason {})",
                finish_reason.as_deref().unwrap_or("<none>")
            ),
        ));
    }
    Ok(GenerateCompletionResponse {
        result: Some(GenResult::Text(text)),
        usage: parsed.usage_metadata.map(|u| ProtoUsage {
            input_tokens: u.prompt_token_count,
            output_tokens: u.candidates_token_count,
            ..Default::default()
        }),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmproxy_model::types::services::llmproxy::llmproxy::{
        content_block::Block, ContentBlock, Message as ProtoMessage, Role as ProtoRole, ToolResult,
    };
    use serde_json::json;

    fn text(role: ProtoRole, text: &str) -> ProtoMessage {
        ProtoMessage {
            role: role as i32,
            content: vec![ContentBlock { block: Some(Block::Text(text.to_string())), ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn the_request_has_user_and_model_turns_and_a_system_instruction() {
        let req = GenerateCompletionRequest {
            system: "be brief".to_string(),
            messages: vec![text(ProtoRole::User, "hi"), text(ProtoRole::Assistant, "hello")],
            max_tokens: 100,
            ..Default::default()
        };
        assert_eq!(
            build_request_body(&req).unwrap(),
            json!({
                "contents": [
                    {"role": "user", "parts": [{"text": "hi"}]},
                    {"role": "model", "parts": [{"text": "hello"}]},
                ],
                "systemInstruction": {"parts": [{"text": "be brief"}]},
                "generationConfig": {"maxOutputTokens": 100},
            })
        );
    }

    #[test]
    fn a_tool_result_block_is_refused() {
        let req = GenerateCompletionRequest {
            messages: vec![ProtoMessage {
                role: ProtoRole::User as i32,
                content: vec![ContentBlock {
                    block: Some(Block::ToolResult(ToolResult::default())),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(matches!(build_request_body(&req), Err(ProviderError::Unsupported(_))));
    }

    #[test]
    fn a_reply_is_the_first_candidates_text_without_thoughts() {
        let body = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "thinking...", "thought": true}, {"text": "4"}]},
                "finishReason": "STOP",
            }],
            "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 1, "totalTokenCount": 8},
            "modelVersion": "gemini-2.5-pro",
        });
        let resp = parse_response(&body).unwrap();
        assert_eq!(resp.result, Some(GenResult::Text("4".to_string())));
        let usage = resp.usage.unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (7, 1));
    }

    #[test]
    fn a_reply_with_no_text_names_its_finish_reason() {
        let body = json!({"candidates": [{"finishReason": "SAFETY"}]});
        let err = parse_response(&body).unwrap_err().to_string();
        assert!(err.contains("finishReason SAFETY"), "{err}");
    }

    #[test]
    fn the_global_location_has_no_regional_host() {
        assert_eq!(vertex_base_url("global"), "https://aiplatform.googleapis.com");
        assert_eq!(vertex_base_url("us-central1"), "https://us-central1-aiplatform.googleapis.com");
    }
}
