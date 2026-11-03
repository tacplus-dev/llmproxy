//! OpenAI (#13): Chat Completions, text in and text out, and the model
//! listing `latest` routes resolve from.
//!
//! The credential is workload identity federation, never a key (Jody,
//! 2026-10-31): a kernel-signed JWT for this service, `aud =
//! Config.openai_subject_audience`, is exchanged (RFC 8693) at
//! `https://auth.openai.com/oauth/token` for a short-lived token of the OpenAI
//! project service account the identity provider's mapping names. OpenAI
//! issues no refresh token and never one that outlives the subject token, so
//! [`Federation`] re-exchanges before `expires_in` runs out.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use llmproxy_model::types::services::llmproxy::llmproxy::{
    generate_completion_response::Result as GenResult, Config as LlmProxyConfig, GenerateCompletionRequest,
    GenerateCompletionResponse, Usage as ProtoUsage, Vendor,
};

use crate::federation::{post_token_request, AssertionSource, Exchange, Federation, Issued};
use crate::providers::{looks_preview, send_json, text_only, ListedModel, ProviderError};

pub const OPENAI_API_BASE: &str = "https://api.openai.com";
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const TOKEN_EXCHANGE_GRANT: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const JWT_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:jwt";

/// The ids of OpenAI's federation, from Config: all three or none.
#[derive(Clone)]
pub struct OpenAiIds {
    pub identity_provider_id: String,
    pub service_account_id: String,
    pub subject_audience: String,
}

impl OpenAiIds {
    pub fn from_config(cfg: &LlmProxyConfig) -> Result<Option<Self>, String> {
        let ids = [
            &cfg.openai_identity_provider_id,
            &cfg.openai_service_account_id,
            &cfg.openai_subject_audience,
        ];
        if ids.iter().all(|id| id.is_empty()) {
            return Ok(None);
        }
        if ids.iter().any(|id| id.is_empty()) {
            return Err("OpenAI federation needs all three openai_* ids set, or none".to_string());
        }
        Ok(Some(Self {
            identity_provider_id: cfg.openai_identity_provider_id.clone(),
            service_account_id: cfg.openai_service_account_id.clone(),
            subject_audience: cfg.openai_subject_audience.clone(),
        }))
    }
}

/// The RFC 8693 exchange of a fresh kernel JWT.
pub struct OpenAiExchange {
    pub token_url: String,
    pub ids: OpenAiIds,
    pub assertions: Arc<dyn AssertionSource>,
}

#[tonic::async_trait]
impl Exchange for OpenAiExchange {
    async fn exchange(&self, http: &reqwest::Client) -> Result<Issued, String> {
        #[derive(Serialize)]
        struct Request<'a> {
            grant_type: &'a str,
            subject_token_type: &'a str,
            subject_token: &'a str,
            identity_provider_id: &'a str,
            service_account_id: &'a str,
        }

        // trace: unrelated: the token is cached and shared across requests
        let subject = self
            .assertions
            .assertion(&self.ids.subject_audience)
            .await
            .map_err(|e| format!("no kernel assertion: {e}"))?;
        post_token_request(
            http,
            &self.token_url,
            &Request {
                grant_type: TOKEN_EXCHANGE_GRANT,
                subject_token_type: JWT_TOKEN_TYPE,
                subject_token: &subject,
                identity_provider_id: &self.ids.identity_provider_id,
                service_account_id: &self.ids.service_account_id,
            },
        )
        .await
    }
}

pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    federation: Arc<Federation>,
}

impl OpenAiClient {
    pub fn new(base_url: Option<String>, federation: Arc<Federation>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.filter(|s| !s.is_empty()).unwrap_or_else(|| OPENAI_API_BASE.to_string()),
            federation,
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<serde_json::Value, ProviderError> {
        let token = self.federation.token().await.map_err(ProviderError::Credential)?;
        send_json(Vendor::Openai, request.bearer_auth(token)).await
    }

    /// Every model `/v1/models` lists. OpenAI says nothing of availability
    /// there, so a preview is one whose id says so.
    pub async fn list_models(&self) -> Result<Vec<ListedModel>, ProviderError> {
        #[derive(Deserialize)]
        struct Model {
            id: String,
        }
        #[derive(Deserialize)]
        struct Listing {
            data: Vec<Model>,
        }

        // trace: unrelated: the listing is cached and shared across requests
        let body = self.send(self.http.get(format!("{}/v1/models", self.base_url))).await?;
        let listing: Listing = serde_json::from_value(body)
            .map_err(|e| ProviderError::UnexpectedResponseShape(Vendor::Openai, format!("model listing: {e}")))?;
        Ok(listing
            .data
            .into_iter()
            .map(|m| ListedModel { preview: looks_preview(&m.id), id: m.id })
            .collect())
    }

    /// One `/v1/chat/completions` call.
    pub async fn complete(
        &self,
        model: &str,
        req: &GenerateCompletionRequest,
    ) -> Result<GenerateCompletionResponse, ProviderError> {
        let body = build_request_body(model, req)?;
        // trace: unrelated: leaves the fleet; vendor APIs carry no trace_id (README, Vendors)
        let reply = self
            .send(self.http.post(format!("{}/v1/chat/completions", self.base_url)).json(&body))
            .await?;
        parse_response(&reply)
    }
}

/// The body carries no prompt-caching parameter (`prompt_cache_retention`,
/// `prompt_cache_options`): each model keeps its own default, up to 24 hours
/// in OpenAI's prompt cache (Jody, 2026-11-01, #14; README, Vendors). A
/// setting that asks for less fails on the latest models, which accept only
/// the 24-hour cache.
fn build_request_body(model: &str, req: &GenerateCompletionRequest) -> Result<serde_json::Value, ProviderError> {
    #[derive(Serialize)]
    struct Message {
        role: &'static str,
        content: String,
    }
    #[derive(Serialize)]
    struct Request<'a> {
        model: &'a str,
        messages: Vec<Message>,
        max_completion_tokens: i32,
    }

    let system = (!req.system.is_empty()).then(|| Message { role: "system", content: req.system.clone() });
    let turns = text_only(Vendor::Openai, req)?.into_iter().map(|m| Message {
        role: if m.assistant { "assistant" } else { "user" },
        content: m.texts.join("\n\n"),
    });
    Ok(serde_json::to_value(Request {
        model,
        messages: system.into_iter().chain(turns).collect(),
        max_completion_tokens: crate::anthropic::output_budget(req.max_tokens),
    })
    .expect("Request serializes: every field type has a Serialize impl"))
}

fn parse_response(body: &serde_json::Value) -> Result<GenerateCompletionResponse, ProviderError> {
    #[derive(Deserialize)]
    struct Message {
        #[serde(default)]
        content: Option<String>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: Message,
        #[serde(default)]
        finish_reason: Option<String>,
    }
    #[derive(Deserialize)]
    struct WireUsage {
        prompt_tokens: i64,
        completion_tokens: i64,
    }
    #[derive(Deserialize)]
    struct Response {
        choices: Vec<Choice>,
        #[serde(default)]
        usage: Option<WireUsage>,
    }

    let parsed: Response = serde_json::from_value(body.clone())
        .map_err(|e| ProviderError::UnexpectedResponseShape(Vendor::Openai, format!("{e}: {body}")))?;
    let choice = parsed.choices.into_iter().next();
    let finish_reason = choice.as_ref().and_then(|c| c.finish_reason.clone());
    match choice.and_then(|c| c.message.content).filter(|t| !t.is_empty()) {
        Some(text) => Ok(GenerateCompletionResponse {
            result: Some(GenResult::Text(text)),
            usage: parsed.usage.map(|u| ProtoUsage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                ..Default::default()
            }),
            ..Default::default()
        }),
        // A reasoning model can spend the whole budget before any text.
        None => Err(ProviderError::UnexpectedResponseShape(
            Vendor::Openai,
            format!(
                "no text in the first choice (finish_reason {})",
                finish_reason.as_deref().unwrap_or("<none>")
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmproxy_model::types::services::llmproxy::llmproxy::{
        content_block::Block, ContentBlock, Message as ProtoMessage, Role as ProtoRole, ToolSpec,
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
    fn the_request_is_system_then_the_turns_with_the_output_budget() {
        let req = GenerateCompletionRequest {
            system: "be brief".to_string(),
            messages: vec![text(ProtoRole::User, "hi"), text(ProtoRole::Assistant, "hello"), text(ProtoRole::User, "2+2")],
            ..Default::default()
        };
        let body = build_request_body("gpt-5.1", &req).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "gpt-5.1",
                "messages": [
                    {"role": "system", "content": "be brief"},
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": "hello"},
                    {"role": "user", "content": "2+2"},
                ],
                "max_completion_tokens": 4096,
            })
        );
    }

    #[test]
    fn no_model_is_sent_a_prompt_caching_parameter() {
        let req = GenerateCompletionRequest { messages: vec![text(ProtoRole::User, "hi")], ..Default::default() };
        for model in ["gpt-4.1", "gpt-5.4", "gpt-5.5", "gpt-5.5-pro", "gpt-5.6", "gpt-7"] {
            let body = build_request_body(model, &req).unwrap();
            let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
            assert!(keys.iter().all(|k| !k.starts_with("prompt_cache")), "{model}: {keys:?}");
        }
    }

    #[test]
    fn tools_are_refused_not_dropped() {
        let req = GenerateCompletionRequest {
            messages: vec![text(ProtoRole::User, "hi")],
            tools: vec![ToolSpec { name: "t".to_string(), ..Default::default() }],
            ..Default::default()
        };
        assert!(matches!(build_request_body("gpt-5.1", &req), Err(ProviderError::Unsupported(_))));
    }

    #[test]
    fn a_reply_is_its_first_choices_text_and_usage() {
        let body = json!({
            "model": "gpt-5.1-2025-11-13",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "4"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15},
        });
        let resp = parse_response(&body).unwrap();
        assert_eq!(resp.result, Some(GenResult::Text("4".to_string())));
        let usage = resp.usage.unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (12, 3));
    }

    #[test]
    fn a_reply_with_no_text_names_its_finish_reason() {
        let body = json!({"choices": [{"message": {"role": "assistant", "content": null}, "finish_reason": "length"}]});
        let err = parse_response(&body).unwrap_err().to_string();
        assert!(err.contains("finish_reason length"), "{err}");
    }

    #[test]
    fn the_ids_are_all_three_or_none() {
        let mut cfg = LlmProxyConfig::default();
        assert!(OpenAiIds::from_config(&cfg).unwrap().is_none());
        cfg.openai_identity_provider_id = "idp_x".to_string();
        assert!(OpenAiIds::from_config(&cfg).err().unwrap().contains("all three"));
        cfg.openai_service_account_id = "svc_x".to_string();
        cfg.openai_subject_audience = "https://openai.test".to_string();
        assert_eq!(OpenAiIds::from_config(&cfg).unwrap().unwrap().service_account_id, "svc_x");
    }
}
