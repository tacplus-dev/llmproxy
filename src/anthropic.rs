//! The Anthropic side of `llmproxy`: the URLs, the HTTP call, and translation
//! between Anthropic's own wire shape and this service's generated proto types.
//!
//! Moved here from `services/scotty/src/llm.rs` 2026-09-15
//! (decisions/llm-proxy-design-proposal-2026-09-14.md,
//! decisions/llm-gateway-mcp-session-auth-2026-09-14.md) — scotty no longer
//! holds the credential or talks to Anthropic directly; this is the one place
//! that does, for every calling service. The request/response SHAPE and
//! (de)serialization logic below is carried over near-verbatim from llm.rs;
//! what's new is the translation to/from `llmproxy_model`'s generated types
//! (`build_request`/`to_generate_completion_response`), since callers now speak
//! this service's own proto, not Anthropic's JSON, directly.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use std::collections::HashMap;

use llmproxy_model::types::services::llmproxy::llmproxy::{
    content_block::Block, generate_completion_response::Result as GenResult, ContentBlock,
    GenerateCompletionRequest, GenerateCompletionResponse, Message as ProtoMessage, ModelAlias, ModelRoute,
    Role as ProtoRole, ToolSpec as ProtoToolSpec, ToolUse as ProtoToolUse, Usage as ProtoUsage,
};

use crate::federation::Federation;

pub const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The alias table: `Config.models`, keyed by alias. The provider model and
/// its price come from config, so a model changes with a config push, never a
/// rebuild (this replaces the code table and LP4(b)'s in-code pricing).
#[derive(Clone, Default)]
pub struct ModelTable {
    routes: HashMap<String, ModelRoute>,
}

impl ModelTable {
    pub fn new(routes: &[ModelRoute]) -> Self {
        Self { routes: routes.iter().map(|r| (r.alias.clone(), r.clone())).collect() }
    }

    /// The route for an alias, or UnknownModelAlias — the gate every call
    /// passes first.
    pub fn resolve(&self, alias: &str) -> Result<&ModelRoute, AnthropicError> {
        self.routes.get(alias).ok_or_else(|| AnthropicError::UnknownModelAlias(alias.to_string()))
    }

    /// The aliases served, for ListModels, in a stable order.
    pub fn aliases(&self) -> Vec<ModelAlias> {
        let mut names: Vec<&String> = self.routes.keys().collect();
        names.sort();
        names.into_iter().map(|name| ModelAlias { name: name.clone(), ..Default::default() }).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WireMessage {
    role: WireRole,
    content: WireContent,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum WireRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum WireContent {
    Text(String),
    Blocks(Vec<WireContentBlock>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
struct WireToolSpec {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug)]
pub enum AnthropicError {
    UnknownModelAlias(String),
    Request(reqwest::Error),
    Api { status: u16, body: String },
    UnexpectedResponseShape(String),
    /// No provider credential could be obtained (D22 federation: the cached
    /// token is expired and the exchange is failing).
    Credential(String),
}

impl std::fmt::Display for AnthropicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnthropicError::UnknownModelAlias(a) => write!(f, "unknown model_alias: {a}"),
            AnthropicError::Request(e) => write!(f, "request to Anthropic failed: {e}"),
            AnthropicError::Api { status, body } => {
                write!(f, "Anthropic API returned {status}: {body}")
            }
            AnthropicError::UnexpectedResponseShape(s) => {
                write!(f, "unexpected Anthropic response shape: {s}")
            }
            AnthropicError::Credential(s) => write!(f, "no Anthropic credential: {s}"),
        }
    }
}

impl std::error::Error for AnthropicError {}

impl From<reqwest::Error> for AnthropicError {
    fn from(e: reqwest::Error) -> Self {
        AnthropicError::Request(e)
    }
}

/// Where the provider credential comes from (D22 §4.2 step 6, mode select).
pub enum Credential {
    /// The static `llm_credential`, sent as `x-api-key`. Transition only.
    ApiKey(String),
    /// A jwt-bearer-exchanged `sk-ant-oat01-…` token, sent as
    /// `Authorization: Bearer`.
    Federation(Arc<Federation>),
}

/// One provider reply inside a session turn.
pub enum ReplyBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

pub struct TurnReply {
    pub blocks: Vec<ReplyBlock>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cost_micro_usd: i64,
}

/// One tool_result the turn loop feeds back to the model.
pub struct WireToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

impl WireMessage {
    pub fn assistant(blocks: &[ReplyBlock]) -> Self {
        WireMessage {
            role: WireRole::Assistant,
            content: WireContent::Blocks(
                blocks
                    .iter()
                    .map(|b| match b {
                        ReplyBlock::Text(text) => WireContentBlock::Text { text: text.clone() },
                        ReplyBlock::ToolUse { id, name, input } => WireContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        },
                    })
                    .collect(),
            ),
        }
    }

    pub fn tool_results(results: &[WireToolResult]) -> Self {
        WireMessage {
            role: WireRole::User,
            content: WireContent::Blocks(
                results
                    .iter()
                    .map(|r| WireContentBlock::ToolResult {
                        tool_use_id: r.tool_use_id.clone(),
                        content: r.content.clone(),
                        is_error: r.is_error,
                    })
                    .collect(),
            ),
        }
    }
}

pub fn to_wire_messages(messages: &[ProtoMessage]) -> Vec<WireMessage> {
    messages
        .iter()
        .map(|m| WireMessage {
            role: to_wire_role(m.role),
            content: to_wire_content(&m.content),
        })
        .collect()
}

/// Anthropic's `metadata.user_id` for a session: a hash of the session id, so
/// prompt caching and abuse signals are per session without sending the
/// caller's SPIFFE ID off-cluster (backlog §1.4).
pub fn session_user_id(session_id: &str) -> String {
    Sha256::digest(session_id.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub struct AnthropicClient {
    http: reqwest::Client,
    credential: Credential,
    base_url: String,
    models: ModelTable,
}

impl AnthropicClient {
    pub fn models(&self) -> &ModelTable {
        &self.models
    }

    pub fn new(credential: Credential, base_url: Option<String>, models: ModelTable) -> Self {
        Self {
            http: reqwest::Client::new(),
            credential,
            models,
            base_url: base_url
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ANTHROPIC_API_BASE.to_string()),
        }
    }

    /// POSTs one `/v1/messages` body with this client's credential and returns
    /// the parsed JSON reply.
    async fn post_messages(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, AnthropicError> {
        let request = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(body);
        let request = match &self.credential {
            Credential::ApiKey(key) => request.header("x-api-key", key),
            Credential::Federation(federation) => request.bearer_auth(
                federation
                    .token()
                    .await
                    .map_err(|e| AnthropicError::Credential(e.to_string()))?,
            ),
        };
        let resp = request.send().await?;

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(AnthropicError::Api {
                status: status.as_u16(),
                body: text,
            });
        }

        serde_json::from_str(&text)
            .map_err(|e| AnthropicError::UnexpectedResponseShape(format!("invalid JSON: {e}")))
    }

    /// One provider call of a session turn (A29). `messages` is the turn's
    /// running wire history, so tool_results keep their `is_error` flag.
    pub async fn create_turn_message(
        &self,
        model_alias: &str,
        system: &str,
        max_tokens: i32,
        messages: &[WireMessage],
        tools: &[ProtoToolSpec],
        user_id: &str,
    ) -> Result<TurnReply, AnthropicError> {
        #[derive(Serialize)]
        struct Metadata<'a> {
            user_id: &'a str,
        }
        #[derive(Serialize)]
        struct Request<'a> {
            model: &'a str,
            max_tokens: i32,
            system: &'a str,
            messages: &'a [WireMessage],
            #[serde(skip_serializing_if = "Vec::is_empty")]
            tools: Vec<WireToolSpec>,
            metadata: Metadata<'a>,
        }

        let route = self.models.resolve(model_alias)?;
        let model = route.provider_model.as_str();
        let body = serde_json::to_value(Request {
            model,
            max_tokens,
            system,
            messages,
            tools: to_wire_tools(tools)?,
            metadata: Metadata { user_id },
        })
        .expect("Request serializes: every field type has a Serialize impl");

        let reply = parse_turn_reply(&self.post_messages(&body).await?)?;
        let (per_in, per_out) = (route.input_micro_usd_per_token, route.output_micro_usd_per_token);
        Ok(TurnReply {
            cost_micro_usd: reply.input_tokens * per_in + reply.output_tokens * per_out,
            ..reply
        })
    }

    /// One non-streaming Anthropic `/v1/messages` call, taking and returning this
    /// service's own generated proto types.
    pub async fn generate_completion(
        &self,
        req: &GenerateCompletionRequest,
    ) -> Result<GenerateCompletionResponse, AnthropicError> {
        let model = self.models.resolve(&req.model_alias)?.provider_model.as_str();
        let body = build_request_body(model, req)?;
        parse_response(&self.post_messages(&body).await?)
    }
}

fn to_wire_role(role: i32) -> WireRole {
    match ProtoRole::try_from(role).unwrap_or(ProtoRole::Unspecified) {
        ProtoRole::Assistant => WireRole::Assistant,
        _ => WireRole::User,
    }
}

fn to_wire_content(blocks: &[ContentBlock]) -> WireContent {
    if let [ContentBlock {
        block: Some(Block::Text(text)),
        ..
    }] = blocks
    {
        return WireContent::Text(text.clone());
    }
    WireContent::Blocks(
        blocks
            .iter()
            .filter_map(|b| match &b.block {
                Some(Block::Text(text)) => Some(WireContentBlock::Text { text: text.clone() }),
                Some(Block::ToolUse(tu)) => Some(WireContentBlock::ToolUse {
                    id: tu.id.clone(),
                    name: tu.name.clone(),
                    input: serde_json::from_str(&tu.input_json)
                        .unwrap_or(serde_json::Value::Object(Default::default())),
                }),
                Some(Block::ToolResult(tr)) => Some(WireContentBlock::ToolResult {
                    tool_use_id: tr.tool_use_id.clone(),
                    content: tr.content.clone(),
                    is_error: false,
                }),
                None => None,
            })
            .collect(),
    )
}

/// Builds the `/v1/messages` request body. Pure and unit-tested separately from
/// the network call so the request shape can be checked without a credential or
/// a live server.
fn build_request_body(
    model: &str,
    req: &GenerateCompletionRequest,
) -> Result<serde_json::Value, AnthropicError> {
    #[derive(Serialize)]
    struct Request<'a> {
        model: &'a str,
        max_tokens: i32,
        system: &'a str,
        messages: Vec<WireMessage>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tools: Vec<WireToolSpec>,
    }

    let messages = req
        .messages
        .iter()
        .map(|m: &ProtoMessage| WireMessage {
            role: to_wire_role(m.role),
            content: to_wire_content(&m.content),
        })
        .collect();

    Ok(serde_json::to_value(Request {
        model,
        max_tokens: req.max_tokens,
        system: &req.system,
        messages,
        tools: to_wire_tools(&req.tools)?,
    })
    .expect("Request serializes: every field type has a Serialize impl"))
}

fn to_wire_tools(tools: &[ProtoToolSpec]) -> Result<Vec<WireToolSpec>, AnthropicError> {
    tools
        .iter()
        .map(|t| {
            Ok(WireToolSpec {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: serde_json::from_str(&t.input_schema_json).map_err(|e| {
                    AnthropicError::UnexpectedResponseShape(format!(
                        "tool {} has invalid input_schema_json: {e}",
                        t.name
                    ))
                })?,
            })
        })
        .collect()
}

/// Parses a `/v1/messages` reply for the turn loop: every text and tool_use
/// block in order (a reply can carry several tool_use blocks), plus usage.
/// `cost_micro_usd` is filled in by the caller, which knows the model.
fn parse_turn_reply(body: &serde_json::Value) -> Result<TurnReply, AnthropicError> {
    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum WireResponseBlock {
        Text {
            text: String,
        },
        ToolUse {
            id: String,
            name: String,
            input: serde_json::Value,
        },
        #[serde(other)]
        Other,
    }

    #[derive(Deserialize, Default)]
    struct WireUsage {
        input_tokens: i64,
        output_tokens: i64,
        #[serde(default)]
        cache_read_input_tokens: i64,
    }

    #[derive(Deserialize)]
    struct Response {
        content: Vec<WireResponseBlock>,
        #[serde(default)]
        usage: WireUsage,
    }

    let parsed: Response = serde_json::from_value(body.clone())
        .map_err(|e| AnthropicError::UnexpectedResponseShape(format!("{e}: {body}")))?;
    Ok(TurnReply {
        blocks: parsed
            .content
            .into_iter()
            .filter_map(|b| match b {
                WireResponseBlock::Text { text } => Some(ReplyBlock::Text(text)),
                WireResponseBlock::ToolUse { id, name, input } => {
                    Some(ReplyBlock::ToolUse { id, name, input })
                }
                WireResponseBlock::Other => None,
            })
            .collect(),
        input_tokens: parsed.usage.input_tokens,
        output_tokens: parsed.usage.output_tokens,
        cache_read_input_tokens: parsed.usage.cache_read_input_tokens,
        cost_micro_usd: 0,
    })
}

/// Parses an Anthropic `/v1/messages` response body into this service's own
/// [`GenerateCompletionResponse`]. Pure and unit-tested against canned response
/// JSON — no network needed.
fn parse_response(body: &serde_json::Value) -> Result<GenerateCompletionResponse, AnthropicError> {
    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum WireResponseBlock {
        Text {
            text: String,
        },
        ToolUse {
            id: String,
            name: String,
            input: serde_json::Value,
        },
        #[serde(other)]
        Other,
    }

    #[derive(Deserialize)]
    struct WireUsage {
        input_tokens: i64,
        output_tokens: i64,
    }

    #[derive(Deserialize)]
    struct Response {
        content: Vec<WireResponseBlock>,
        #[serde(default)]
        usage: Option<WireUsage>,
    }

    let parsed: Response = serde_json::from_value(body.clone())
        .map_err(|e| AnthropicError::UnexpectedResponseShape(format!("{e}: {body}")))?;

    let usage = parsed.usage.map(|u| ProtoUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        ..Default::default()
    });

    for block in parsed.content {
        match block {
            WireResponseBlock::ToolUse { id, name, input } => {
                return Ok(GenerateCompletionResponse {
                    result: Some(GenResult::ToolUse(ProtoToolUse {
                        id,
                        name,
                        input_json: input.to_string(),
                        ..Default::default()
                    })),
                    usage,
                    ..Default::default()
                })
            }
            WireResponseBlock::Text { text } => {
                return Ok(GenerateCompletionResponse {
                    result: Some(GenResult::Text(text)),
                    usage,
                    ..Default::default()
                })
            }
            WireResponseBlock::Other => continue,
        }
    }

    Err(AnthropicError::UnexpectedResponseShape(
        "no text or tool_use block in content".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_message(role: i32, text: &str) -> ProtoMessage {
        ProtoMessage {
            role,
            content: vec![ContentBlock {
                block: Some(Block::Text(text.to_string())),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn unknown_model_alias_is_rejected() {
        // build_request_body takes an ALREADY-RESOLVED provider model string
        // (see generate_completion: ModelTable::resolve runs first), so it has
        // no opinion on model_alias itself — what's under test is that the
        // table, the actual gate, refuses an alias config did not declare,
        // and that an empty table refuses everything.
        let table = ModelTable::new(&[ModelRoute {
            alias: "claude-sonnet".to_string(),
            provider_model: "claude-sonnet-4-5".to_string(),
            input_micro_usd_per_token: 3,
            output_micro_usd_per_token: 15,
            ..Default::default()
        }]);
        let err = table.resolve("gpt-5").unwrap_err();
        assert!(matches!(err, AnthropicError::UnknownModelAlias(a) if a == "gpt-5"));
        assert_eq!(table.resolve("claude-sonnet").unwrap().provider_model, "claude-sonnet-4-5");
        assert!(matches!(ModelTable::default().resolve("claude-sonnet"), Err(AnthropicError::UnknownModelAlias(_))));
    }

    #[test]
    fn request_body_carries_system_messages_and_tools() {
        let req = GenerateCompletionRequest {
            model_alias: "claude-sonnet".to_string(),
            system: "be helpful".to_string(),
            messages: vec![text_message(ProtoRole::User as i32, "hello")],
            tools: vec![ProtoToolSpec {
                name: "get_my_ticket_audit".to_string(),
                description: "audits the caller's own ticket activity".to_string(),
                input_schema_json: json!({"type": "object", "properties": {}}).to_string(),
                ..Default::default()
            }],
            max_tokens: 4096,
            ..Default::default()
        };

        let body = build_request_body("claude-sonnet-4-5", &req).expect("builds");

        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["system"], "be helpful");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hello");
        assert_eq!(body["tools"][0]["name"], "get_my_ticket_audit");
    }

    #[test]
    fn request_body_omits_tools_when_none_given() {
        let req = GenerateCompletionRequest {
            model_alias: "claude-sonnet".to_string(),
            system: "sys".to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: 4096,
            ..Default::default()
        };
        let body = build_request_body("claude-sonnet-4-5", &req).expect("builds");
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn tool_use_and_tool_result_round_trip_as_a_content_block_array() {
        let req = GenerateCompletionRequest {
            model_alias: "claude-sonnet".to_string(),
            system: "sys".to_string(),
            messages: vec![
                ProtoMessage {
                    role: ProtoRole::Assistant as i32,
                    content: vec![ContentBlock {
                        block: Some(Block::ToolUse(ProtoToolUse {
                            id: "toolu_1".to_string(),
                            name: "get_my_ticket_audit".to_string(),
                            input_json: json!({"chain_id": "c-1"}).to_string(),
                            ..Default::default()
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ProtoMessage {
                    role: ProtoRole::User as i32,
                    content: vec![ContentBlock {
                        block: Some(Block::ToolResult(llmproxy_model::types::services::llmproxy::llmproxy::ToolResult {
                            tool_use_id: "toolu_1".to_string(),
                            content: "{}".to_string(),
                            ..Default::default()
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            tools: vec![],
            max_tokens: 4096,
            ..Default::default()
        };

        let body = build_request_body("claude-sonnet-4-5", &req).expect("builds");

        assert_eq!(body["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][0]["content"][0]["id"], "toolu_1");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn parses_text_reply() {
        let body = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "the answer is 4"}],
            "usage": {"input_tokens": 10, "output_tokens": 5},
        });

        let parsed = parse_response(&body).expect("parses");
        assert_eq!(parsed.result, Some(GenResult::Text("the answer is 4".to_string())));
        assert_eq!(parsed.usage.unwrap().input_tokens, 10);
    }

    #[test]
    fn a_turn_reply_carries_the_cache_read_tokens() {
        let body = json!({
            "content": [{"type": "text", "text": "hi"}],
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 90},
        });
        let reply = parse_turn_reply(&body).expect("parses");
        assert_eq!((reply.input_tokens, reply.output_tokens, reply.cache_read_input_tokens), (10, 5, 90));

        let body = json!({"content": [], "usage": {"input_tokens": 10, "output_tokens": 5}});
        assert_eq!(parse_turn_reply(&body).expect("parses").cache_read_input_tokens, 0);
    }

    #[test]
    fn parses_tool_use_reply() {
        let body = json!({
            "id": "msg_2",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "get_my_ticket_audit", "input": {"ticket_id": "t-1"}}
            ],
        });

        let parsed = parse_response(&body).expect("parses");
        match parsed.result {
            Some(GenResult::ToolUse(tu)) => {
                assert_eq!(tu.id, "toolu_1");
                assert_eq!(tu.name, "get_my_ticket_audit");
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&tu.input_json).unwrap(),
                    json!({"ticket_id": "t-1"})
                );
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn rejects_response_with_no_recognized_block() {
        let body = json!({"content": []});
        let err = parse_response(&body).unwrap_err();
        assert!(matches!(err, AnthropicError::UnexpectedResponseShape(_)));
    }

    #[test]
    fn list_models_is_the_configured_aliases_in_order() {
        let table = ModelTable::new(&[
            ModelRoute { alias: "b".to_string(), provider_model: "m2".to_string(), ..Default::default() },
            ModelRoute { alias: "a".to_string(), provider_model: "m1".to_string(), ..Default::default() },
        ]);
        let names: Vec<String> = table.aliases().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
        assert!(ModelTable::default().aliases().is_empty());
    }
}
