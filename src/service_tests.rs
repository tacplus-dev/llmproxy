//! Wrapper-level tests for the LlmProxy service.
//!
//! Every RPC goes in through the GENERATED `ValidationWrapper`, with a signed
//! bearer on the request metadata, exactly as a real client's call arrives —
//! same shape as llmsettings' and mailbox's own `service_tests.rs`. The unary
//! RPCs are driven on the wrapper directly; `BrokerToolCallback` is bidi, so
//! the wrapper is served on a localhost port and driven with the generated
//! tonic client (an in-process callback client). The provider is a mock
//! Messages API (test_support.rs) reached through `default_provider_base_url`.
//!
//! Covers backlog §1.8 items 1–3 (A29) and the service-level half of §4.6
//! item 1 (D22: the federated token on the wire, UNAVAILABLE when it fails).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tokio_stream::StreamExt;

use llmproxy_model::s2sm::internal::grpc::servers::services::llmproxy::llm_proxy::{
    Authorizer, ValidationWrapper,
};
use llmproxy_model::types::services::llmproxy::llmproxy::llm_proxy_client::LlmProxyClient;
use llmproxy_model::types::services::llmproxy::llmproxy::llm_proxy_server::LlmProxy;
use llmproxy_model::types::services::llmproxy::llmproxy::{
    content_block::Block, Config as LlmProxyConfig, ContentBlock, GenerateCompletionRequest,
    GetBudgetRequest, ListModelsRequest, Message, ModelAlias, Role, SessionTurnRequest,
    SessionTurnResponse, StopReason, ToolSpec,
};
use llmproxy_model::types::services::llmproxy::helpers::config_defaults;
use llmproxy_model::types::services::llmproxy::llmproxy::BudgetBucket;
use llmproxy_model::types::sys::accounting::accounting::Event;
use llmproxy_model::types::sys::mcp::McpResponse;

use crate::federation::{AssertionSource, FederationIds};
use crate::service::LlmProxyService;
use crate::test_support::{now_secs, serve, MockAnthropic, MockAssertions, MockTokenEndpoint};

const TRUST_DOMAIN: &str = "td.test";
const TENANT_A: &str = "/tenants/a";
const TENANT_B: &str = "/tenants/b";

// ---------------------------------------------------------------------------
// A permissive test Authorizer — same convention llmsettings' service_tests.rs
// uses: identity extraction only, no real auth_services enforcement (that has
// its own tests in tacplus's authorizer.rs.tmpl coverage, not per-service).
// ---------------------------------------------------------------------------

struct TokenAuthorizer;

impl TokenAuthorizer {
    fn identity(authorization: Option<&str>) -> Result<HashMap<String, String>, tonic::Status> {
        let parsed = authorization
            .and_then(|v| v.strip_prefix("Bearer "))
            .and_then(|t| token::token::Token::parse(t).ok())
            .ok_or_else(|| tonic::Status::unauthenticated("no parsable bearer token"))?;
        let mut identity = HashMap::new();
        identity.insert(
            "sub".to_string(),
            parsed.subject().unwrap_or_default().to_string(),
        );
        Ok(identity)
    }
}

#[tonic::async_trait]
impl Authorizer for TokenAuthorizer {
    async fn authorize(
        &self,
        _full_method: &str,
        authorization: Option<&str>,
    ) -> Result<HashMap<String, String>, tonic::Status> {
        Self::identity(authorization)
    }

    async fn check_at_or_below(
        &self,
        authorization: Option<&str>,
        _target_namespace: &str,
    ) -> Result<HashMap<String, String>, tonic::Status> {
        Self::identity(authorization)
    }

    async fn authenticate(
        &self,
        authorization: Option<&str>,
    ) -> Result<HashMap<String, String>, tonic::Status> {
        Self::identity(authorization)
    }
}

/// An accounting `Sink` that keeps the events instead of publishing them.
struct CapturingSink(Arc<Mutex<Vec<Event>>>);

#[tonic::async_trait]
impl am::Sink for CapturingSink {
    async fn emit(&self, event: &Event) {
        self.0.lock().unwrap().push(event.clone());
    }
}

/// A caller: its namespace (tenant) and service label.
#[derive(Clone, Copy)]
struct Who(&'static str, &'static str);

const FRANK_A: Who = Who(TENANT_A, "frank");
const FRANK_B: Who = Who(TENANT_B, "frank");
const SCOTTY: Who = Who("/system", "scotty");

impl Who {
    fn spiffe(self) -> String {
        format!("spiffe://{TRUST_DOMAIN}{}/services/{}", self.0, self.1)
    }

    /// A signed service bearer the wrapper's `token::Token::parse` accepts.
    /// Signed with a throwaway key because `Token::base64()` only exists once
    /// a token has been signed — nothing downstream verifies the signature in
    /// this harness (see TokenAuthorizer above).
    fn bearer(self) -> String {
        let key = key::keys::generate_ed25519_key().unwrap();
        let mut t = token::token::Token::new();
        t.set_issuer("llmproxy-tests").unwrap();
        t.set_sub_type("service").unwrap();
        t.set_subject(self.1).unwrap();
        t.set_namespace(self.0).unwrap();
        t.set_expiration(now_secs() + 3600).unwrap();
        t.sign(&key).unwrap();
        format!("Bearer {}", t.base64().unwrap())
    }

    fn signed<T>(self, req: T) -> tonic::Request<T> {
        let mut request = tonic::Request::new(req);
        request
            .metadata_mut()
            .insert("authorization", self.bearer().parse().unwrap());
        request
    }
}

struct Harness {
    svc: Arc<LlmProxyService>,
    wrapper: ValidationWrapper,
    anthropic: Arc<MockAnthropic>,
    tokens: Arc<MockTokenEndpoint>,
    cfg: LlmProxyConfig,
    grpc_url: String,
}

impl Harness {
    async fn start() -> Harness {
        Harness::start_with(|_| {}, None).await
    }

    async fn start_with(
        configure: impl FnOnce(&mut LlmProxyConfig),
        assertions: Option<Arc<dyn AssertionSource>>,
    ) -> Harness {
        Harness::start_with_env(configure, assertions, None).await
    }

    async fn start_with_env(
        configure: impl FnOnce(&mut LlmProxyConfig),
        assertions: Option<Arc<dyn AssertionSource>>,
        env_ids: Option<FederationIds>,
    ) -> Harness {
        // One mock host serves both the Messages API and the token endpoint,
        // as api.anthropic.com does.
        let anthropic = Arc::new(MockAnthropic::default());
        let tokens = MockTokenEndpoint::new(3600);
        let provider_url = serve(anthropic.router().merge(tokens.router())).await;
        let mut cfg = LlmProxyConfig {
            llm_credential: "sk-ant-static-test".to_string(),
            default_provider_base_url: provider_url,
            ..Default::default()
        };
        configure(&mut cfg);
        config_defaults(&mut cfg);

        let svc = Arc::new(LlmProxyService::new(
            TRUST_DOMAIN.to_string(),
            assertions,
            env_ids,
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts").into(),
        ));
        svc.set_config(&cfg).unwrap();

        let accounting: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let wrapper = ValidationWrapper::new(svc.clone());
        wrapper
            .set_authorize_func(Some(Arc::new(TokenAuthorizer)))
            .await;
        wrapper.set_instance_id("testinst".to_string()).await;
        wrapper
            .set_emitter(Some(Arc::new(am::Emitter::new(
                "llmproxy-testinst".to_string(),
                Box::new(CapturingSink(accounting)),
            ))))
            .await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let grpc_url = format!("http://{}", listener.local_addr().unwrap());
        let router = wrapper.register_services(tonic::transport::Server::builder());
        tokio::spawn(async move {
            router
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap()
        });

        Harness { svc, wrapper, anthropic, tokens, cfg, grpc_url }
    }

    async fn list_models(&self, who: Who) -> Result<Vec<ModelAlias>, tonic::Status> {
        self.wrapper
            .list_models(who.signed(ListModelsRequest::default()))
            .await
            .map(|r| r.into_inner().model_aliases)
    }

    async fn turn(&self, who: Who, req: SessionTurnRequest) -> Result<SessionTurnResponse, tonic::Status> {
        self.wrapper
            .generate_session_turn(who.signed(req))
            .await
            .map(|r| r.into_inner())
    }

    /// Opens `who`'s callback stream and answers frames per `answer`.
    async fn open_callback(&self, who: Who, answer: Answer) -> CallbackClient {
        let mut client = LlmProxyClient::connect(self.grpc_url.clone()).await.unwrap();
        let (tx, rx) = mpsc::channel::<McpResponse>(16);
        let mut frames = client
            .broker_tool_callback(who.signed(ReceiverStream::new(rx)))
            .await
            .expect("BrokerToolCallback opens")
            .into_inner();

        let seen = Arc::new(Mutex::new(Vec::new()));
        let (ended_tx, ended_rx) = tokio::sync::oneshot::channel();
        let recorded = seen.clone();
        let mut tx = Some(tx);
        tokio::spawn(async move {
            while let Some(Ok(frame)) = frames.next().await {
                let frame: serde_json::Value = serde_json::from_str(&frame.payload).unwrap();
                recorded.lock().unwrap().push(frame.clone());
                match answer {
                    Answer::Result => {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": frame["id"],
                            "result": {"content": [{"type": "text", "text": "posted"}], "isError": false},
                        });
                        let payload = reply.to_string();
                        if let Some(tx) = &tx {
                            tx.send(McpResponse { payload }).await.unwrap();
                        }
                    }
                    Answer::Never => {}
                    // Ends the caller's half of the stream mid-turn.
                    Answer::Drop => {
                        tx.take();
                    }
                }
            }
            let _ = ended_tx.send(());
        });
        // Let the open reach the registry before a turn is requested.
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.svc.callbacks.get(&who.spiffe()).is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("callback stream registered");
        CallbackClient { seen, ended: ended_rx }
    }
}

#[derive(Clone, Copy)]
enum Answer {
    Result,
    Never,
    Drop,
}

struct CallbackClient {
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
    ended: tokio::sync::oneshot::Receiver<()>,
}

impl CallbackClient {
    fn frames(&self) -> Vec<serde_json::Value> {
        self.seen.lock().unwrap().clone()
    }
}

fn user_text(text: &str) -> Message {
    Message {
        role: Role::User as i32,
        content: vec![ContentBlock {
            block: Some(Block::Text(text.to_string())),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn turn_request(thread_id: &str, with_tools: bool) -> SessionTurnRequest {
    SessionTurnRequest {
        thread_id: thread_id.to_string(),
        turn_id: format!("turn-{thread_id}"),
        model_alias: "claude-sonnet".to_string(),
        bucket: BudgetBucket::DayJob as i32,
        system: "you are frank".to_string(),
        messages: vec![user_text("@frank what's up")],
        tools: if with_tools {
            vec![ToolSpec {
                name: "post_room".to_string(),
                description: "post to the room".to_string(),
                input_schema_json: r#"{"type":"object","properties":{"text":{"type":"string"}}}"#.to_string(),
                ..Default::default()
            }]
        } else {
            vec![]
        },
        max_tokens: 1024,
        ..Default::default()
    }
}

fn tool_result_text(msg: &Message) -> &str {
    match &msg.content[0].block {
        Some(Block::ToolResult(r)) => &r.content,
        other => panic!("expected a tool_result, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Phase 1 RPCs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_models_through_the_wrapper_returns_a_non_empty_list() {
    let h = Harness::start().await;
    let aliases = h.list_models(SCOTTY).await.expect("list_models succeeds");
    assert!(!aliases.is_empty(), "llmproxy should always know at least one model alias");
}

#[tokio::test]
async fn list_models_is_the_same_regardless_of_caller() {
    // Phase 1 has no per-caller policy on ListModels (llmproxy.proto's own
    // comment: "this carries no policy of its own to protect") — asserting
    // that stays true rather than assuming it.
    let h = Harness::start().await;
    let as_scotty = h.list_models(SCOTTY).await.expect("succeeds");
    let as_someone_else = h.list_models(Who("/system", "reveille")).await.expect("succeeds");
    assert_eq!(as_scotty, as_someone_else);
}

#[tokio::test]
async fn an_unsigned_call_is_refused() {
    let h = Harness::start().await;
    let request = tonic::Request::new(ListModelsRequest::default());
    let err = h.wrapper.list_models(request).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[tokio::test]
async fn the_static_key_is_sent_as_x_api_key_when_no_federation_ids_are_set() {
    let h = Harness::start().await;
    h.wrapper
        .generate_completion_as_service(SCOTTY.signed(GenerateCompletionRequest {
            model_alias: "claude-sonnet".to_string(),
            messages: vec![user_text("hi")],
            ..Default::default()
        }))
        .await
        .expect("completion succeeds");
    let (headers, _) = h.anthropic.requests.lock().unwrap()[0].clone();
    assert_eq!(headers["x-api-key"], "sk-ant-static-test");
    assert!(headers.get("authorization").is_none());
}

// ---------------------------------------------------------------------------
// A29 session path (backlog §1.8 item 1)
// ---------------------------------------------------------------------------

/// (a) tool_use → frame on the caller's stream → tool_result → end_turn.
#[tokio::test]
async fn a_tool_use_round_trips_through_the_callers_stream() {
    let h = Harness::start().await;
    let callback = h.open_callback(FRANK_A, Answer::Result).await;

    let resp = h.turn(FRANK_A, turn_request("t-1", true)).await.expect("turn succeeds");

    assert_eq!(resp.stop_reason, StopReason::EndTurn as i32);
    assert_eq!(resp.appended.len(), 3, "assistant tool_use, tool_result, assistant text");
    assert!(matches!(&resp.appended[0].content[0].block, Some(Block::ToolUse(tu)) if tu.name == "post_room"));
    assert_eq!(tool_result_text(&resp.appended[1]), "posted");
    assert!(matches!(&resp.appended[2].content[0].block, Some(Block::Text(t)) if t == "done"));
    let usage = resp.usage.unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (20, 10));
    assert_eq!(usage.cost_micro_usd, 2 * (10 * 3 + 5 * 15));
    assert_eq!(usage.bucket, BudgetBucket::DayJob as i32);

    let frames = callback.frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["jsonrpc"], "2.0");
    assert_eq!(frames[0]["method"], "tools/call");
    assert_eq!(frames[0]["params"]["name"], "post_room");
    assert_eq!(frames[0]["params"]["arguments"], serde_json::json!({"text": "on it"}));
    assert_eq!(frames[0]["params"]["_meta"]["turn"], "turn-t-1");
    assert!(frames[0]["id"].is_string());

    let second = &h.anthropic.bodies()[1];
    let result = &second["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["content"], "posted");
    // metadata.user_id is a hash of the session id, never the SPIFFE ID itself.
    let user_id = second["metadata"]["user_id"].as_str().unwrap();
    assert_eq!(user_id.len(), 64);
    assert!(!user_id.contains("spiffe"));
}

#[tokio::test]
async fn frame_ids_are_unique_across_turns() {
    let h = Harness::start().await;
    let callback = h.open_callback(FRANK_A, Answer::Result).await;
    h.turn(FRANK_A, turn_request("t-1", true)).await.unwrap();
    h.turn(FRANK_A, turn_request("t-2", true)).await.unwrap();
    let ids: std::collections::HashSet<String> = callback
        .frames()
        .iter()
        .map(|f| f["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);
}

/// (b) two tenants' frank, same thread_id: different sessions, own streams.
#[tokio::test]
async fn the_same_thread_id_in_two_tenants_is_two_sessions_on_two_streams() {
    let h = Harness::start().await;
    let a = h.open_callback(FRANK_A, Answer::Result).await;
    let b = h.open_callback(FRANK_B, Answer::Result).await;

    h.turn(FRANK_A, turn_request("t-1", true)).await.unwrap();
    h.turn(FRANK_B, turn_request("t-1", true)).await.unwrap();
    h.turn(FRANK_A, turn_request("t-1", true)).await.unwrap();

    let sessions: Vec<String> = h
        .svc
        .ledger
        .lock()
        .unwrap()
        .rows()
        .iter()
        .map(|r| r.session_id.clone())
        .collect();
    let a_session = "spiffe://td.test/tenants/a/services/frank#t-1";
    let b_session = "spiffe://td.test/tenants/b/services/frank#t-1";
    assert_eq!(sessions.iter().filter(|s| *s == a_session).count(), 4);
    assert_eq!(sessions.iter().filter(|s| *s == b_session).count(), 2);
    assert_eq!(sessions.len(), 6);

    assert_eq!(a.frames().len(), 2, "tenant a's frames only on a's stream");
    assert_eq!(b.frames().len(), 1, "tenant b's frames only on b's stream");
}

/// (c) tools offered, no stream: FAILED_PRECONDITION, zero provider requests.
#[tokio::test]
async fn tools_without_a_callback_stream_are_refused_before_any_provider_call() {
    let h = Harness::start().await;
    let err = h.turn(FRANK_A, turn_request("t-1", true)).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(h.anthropic.bodies().is_empty());
}

#[tokio::test]
async fn a_turn_without_tools_needs_no_stream() {
    let h = Harness::start().await;
    let resp = h.turn(FRANK_A, turn_request("t-1", false)).await.unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn as i32);
    assert_eq!(h.anthropic.bodies().len(), 1);
}

/// (d) a frame nobody answers: descriptive is_error result, turn continues.
#[tokio::test]
async fn a_tool_timeout_becomes_a_descriptive_is_error_result() {
    let h = Harness::start_with(|c| c.tool_call_timeout_ms = 200, None).await;
    let _callback = h.open_callback(FRANK_A, Answer::Never).await;

    let resp = h.turn(FRANK_A, turn_request("t-1", true)).await.unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn as i32);
    assert_eq!(
        tool_result_text(&resp.appended[1]),
        "tool post_room timed out after 200 ms; the call may or may not have taken effect; check before retrying"
    );
    let second = &h.anthropic.bodies()[1];
    let result = &second["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert_eq!(result["is_error"], true);
}

/// (e) the caller's stream drops mid-turn: callback_lost, no hang.
#[tokio::test]
async fn a_stream_drop_mid_turn_ends_the_turn_callback_lost() {
    let h = Harness::start().await;
    let callback = h.open_callback(FRANK_A, Answer::Drop).await;

    let resp = tokio::time::timeout(
        Duration::from_secs(5),
        h.turn(FRANK_A, turn_request("t-1", true)),
    )
    .await
    .expect("the turn does not hang")
    .unwrap();

    assert_eq!(resp.stop_reason, StopReason::CallbackLost as i32);
    assert!(tool_result_text(&resp.appended[1]).contains("callback stream was lost"));
    assert_eq!(callback.frames().len(), 1);
    assert_eq!(h.anthropic.bodies().len(), 1, "no provider call after the loss");
}

/// (f) kill switch: PERMISSION_DENIED on every rpc, open stream closed on reload.
#[tokio::test]
async fn a_blocked_caller_is_denied_and_its_open_stream_is_closed() {
    let h = Harness::start().await;
    let callback = h.open_callback(FRANK_A, Answer::Result).await;

    let mut cfg = h.cfg.clone();
    cfg.blocked_callers = vec!["spiffe://td.test/tenants/a/".to_string()];
    h.svc.set_config(&cfg).unwrap();

    tokio::time::timeout(Duration::from_secs(5), callback.ended)
        .await
        .expect("the blocked caller's stream is closed on config reload")
        .unwrap();
    assert!(h.svc.callbacks.get(&FRANK_A.spiffe()).is_none());

    let err = h.turn(FRANK_A, turn_request("t-1", false)).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    let err = h
        .wrapper
        .get_budget(FRANK_A.signed(GetBudgetRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    let mut client = LlmProxyClient::connect(h.grpc_url.clone()).await.unwrap();
    let (_tx, rx) = mpsc::channel::<McpResponse>(1);
    let err = client
        .broker_tool_callback(FRANK_A.signed(ReceiverStream::new(rx)))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);

    // Tenant b is untouched.
    h.turn(FRANK_B, turn_request("t-1", false)).await.unwrap();
    assert_eq!(h.anthropic.bodies().len(), 1);
}

/// The fleet-wide LLM spend stop (ops/datagate/README.md): `spiffe://` prefixes
/// every caller, so one entry refuses every tenant and system caller before any
/// provider call; clearing it restores service.
#[tokio::test]
async fn blocking_the_spiffe_scheme_stops_every_caller() {
    let h = Harness::start().await;
    let mut cfg = h.cfg.clone();
    cfg.blocked_callers = vec!["spiffe://".to_string()];
    h.svc.set_config(&cfg).unwrap();

    for who in [FRANK_A, FRANK_B] {
        let err = h.turn(who, turn_request("t-1", false)).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }
    let err = h
        .wrapper
        .generate_completion_as_service(SCOTTY.signed(completion()))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    assert_eq!(h.anthropic.bodies().len(), 0, "no provider call while stopped");

    cfg.blocked_callers.clear();
    h.svc.set_config(&cfg).unwrap();
    h.turn(FRANK_A, turn_request("t-1", false)).await.unwrap();
    assert_eq!(h.anthropic.bodies().len(), 1);
}

#[tokio::test]
async fn a_reopened_stream_replaces_the_old_one() {
    let h = Harness::start().await;
    let old = h.open_callback(FRANK_A, Answer::Result).await;
    let new = h.open_callback(FRANK_A, Answer::Result).await;

    tokio::time::timeout(Duration::from_secs(5), old.ended)
        .await
        .expect("the old stream is closed")
        .unwrap();
    h.turn(FRANK_A, turn_request("t-1", true)).await.unwrap();
    assert_eq!(new.frames().len(), 1);
}

#[tokio::test]
async fn the_turn_ceiling_stops_before_another_provider_call() {
    let h = Harness::start().await;
    let _callback = h.open_callback(FRANK_A, Answer::Result).await;
    let mut req = turn_request("t-1", true);
    req.max_tool_rounds = 1;

    let resp = h.turn(FRANK_A, req).await.unwrap();

    assert_eq!(resp.stop_reason, StopReason::MaxToolRounds as i32);
    assert_eq!(resp.appended.len(), 2, "ends on the tool_result, never a dangling tool_use");
    assert_eq!(h.anthropic.bodies().len(), 1);
}

#[tokio::test]
async fn an_exhausted_bucket_stops_the_turn_and_shows_in_get_budget() {
    let h = Harness::start_with(|c| c.default_day_job_micro_usd = 1, None).await;
    let _callback = h.open_callback(FRANK_A, Answer::Result).await;

    let resp = h.turn(FRANK_A, turn_request("t-1", true)).await.unwrap();
    assert_eq!(resp.stop_reason, StopReason::BudgetExhausted as i32);
    assert_eq!(h.anthropic.bodies().len(), 1);

    let budget = h
        .wrapper
        .get_budget(FRANK_A.signed(GetBudgetRequest::default()))
        .await
        .unwrap()
        .into_inner();
    let day_job = budget
        .buckets
        .iter()
        .find(|b| b.bucket == BudgetBucket::DayJob as i32)
        .unwrap();
    assert_eq!((day_job.limit_micro_usd, day_job.spent_micro_usd), (1, 105));
    // Spend is per caller: tenant b's frank has spent nothing.
    let b_budget = h
        .wrapper
        .get_budget(FRANK_B.signed(GetBudgetRequest::default()))
        .await
        .unwrap()
        .into_inner();
    assert!(b_budget.buckets.iter().all(|b| b.spent_micro_usd == 0));
}

// ---------------------------------------------------------------------------
// §1.8 items 2 and 3: the request never carries a session id; the 09-14
// kernel RPCs are not added.
// ---------------------------------------------------------------------------

#[test]
fn no_session_id_rides_the_request_and_no_llm_tool_session_rpcs_exist() {
    let proto = include_str!("../proto/services/llmproxy.proto");
    let message = proto
        .split("message SessionTurnRequest {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("SessionTurnRequest is declared");
    assert!(
        !message.lines().any(|l| !l.trim_start().starts_with("//") && l.contains("session_id")),
        "SessionTurnRequest must not carry a session_id field"
    );
    for source in [include_str!("session.rs"), include_str!("service.rs")] {
        assert!(!source.contains(concat!("req.", "session_id")));
    }
    // The header comment names the 09-14 RPCs to say they are not used; what
    // must not exist is an rpc declaration.
    assert!(!proto
        .lines()
        .any(|l| l.trim_start().starts_with("rpc ") && l.contains("LlmToolSession")));
}

// ---------------------------------------------------------------------------
// D22 at the service (backlog §4.6 item 1, with federation_tests.rs)
// ---------------------------------------------------------------------------

fn federation_ids(c: &mut LlmProxyConfig) {
    c.anthropic_federation_rule_id = "fdrl_test".to_string();
    c.anthropic_organization_id = "org".to_string();
    c.anthropic_service_account_id = "svac_test".to_string();
    c.anthropic_workspace_id = "wrkspc_test".to_string();
}

fn completion() -> GenerateCompletionRequest {
    GenerateCompletionRequest {
        model_alias: "claude-sonnet".to_string(),
        messages: vec![user_text("hi")],
        ..Default::default()
    }
}

#[tokio::test]
async fn federation_mode_sends_the_exchanged_token_and_ignores_llm_credential() {
    let h = Harness::start_with(federation_ids, Some(Arc::new(MockAssertions::default()))).await;
    h.wrapper
        .generate_completion_as_service(SCOTTY.signed(completion()))
        .await
        .expect("completion succeeds");

    assert_eq!(h.tokens.exchanges(), 1);
    let (headers, _) = h.anthropic.requests.lock().unwrap()[0].clone();
    assert_eq!(headers["authorization"], "Bearer tok-1");
    assert!(headers.get("x-api-key").is_none());
}

#[tokio::test]
async fn federation_ids_from_the_env_select_federation_mode_without_config_ids() {
    let env_ids = FederationIds::from_parts("fdrl_env", "org", "svac_env", "wrkspc_env").unwrap();
    let h = Harness::start_with_env(|_| {}, Some(Arc::new(MockAssertions::default())), env_ids).await;
    h.wrapper
        .generate_completion_as_service(SCOTTY.signed(completion()))
        .await
        .expect("completion succeeds");

    assert_eq!(h.tokens.exchanges(), 1);
    assert_eq!(h.tokens.bodies.lock().unwrap()[0]["federation_rule_id"], "fdrl_env");
    let (headers, _) = h.anthropic.requests.lock().unwrap()[0].clone();
    assert_eq!(headers["authorization"], "Bearer tok-1");
    assert!(headers.get("x-api-key").is_none());
}

#[tokio::test]
async fn an_expired_token_with_a_failing_exchange_is_unavailable() {
    let h = Harness::start_with(federation_ids, Some(Arc::new(MockAssertions::default()))).await;
    h.tokens.expires_in.store(0, Ordering::SeqCst);
    h.wrapper
        .generate_completion_as_service(SCOTTY.signed(completion()))
        .await
        .expect("first completion succeeds");

    h.tokens.fail.store(true, Ordering::SeqCst);
    let err = h
        .wrapper
        .generate_completion_as_service(SCOTTY.signed(completion()))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unavailable);
    assert!(err.message().contains("token exchange failed"), "{}", err.message());
    assert_eq!(h.anthropic.bodies().len(), 1, "no provider call without a credential");
}

#[tokio::test]
async fn federation_ids_without_an_assertion_source_or_half_set_are_config_errors() {
    let h = Harness::start().await;
    let mut cfg = h.cfg.clone();
    federation_ids(&mut cfg);
    let err = h.svc.set_config(&cfg).unwrap_err();
    assert!(err.contains("no kernel assertion source"), "{err}");

    let mut cfg = h.cfg.clone();
    cfg.anthropic_federation_rule_id = "fdrl_test".to_string();
    let err = h.svc.set_config(&cfg).unwrap_err();
    assert!(err.contains("all four"), "{err}");
}

// ---------------------------------------------------------------------------
// harness-scripted stand-in (harness-scotty-scenarios §1.3)
// ---------------------------------------------------------------------------

fn scripted_for_tenant_a(cfg: &mut LlmProxyConfig) {
    cfg.scripted_model_tenants = vec!["a".to_string()];
}

fn marked_turn(thread_id: &str, text: &str) -> SessionTurnRequest {
    SessionTurnRequest {
        messages: vec![user_text(text)],
        ..turn_request(thread_id, false)
    }
}

fn last_text(resp: &SessionTurnResponse) -> &str {
    match &resp.appended.last().unwrap().content[0].block {
        Some(Block::Text(t)) => t,
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn a_marker_from_a_listed_tenant_gets_the_scripted_reply() {
    let h = Harness::start_with(scripted_for_tenant_a, None).await;
    let resp = h
        .turn(FRANK_A, marked_turn("t-1", "@frank [[script:echo]] marker m1"))
        .await
        .expect("scripted turn succeeds");
    assert_eq!(resp.stop_reason, StopReason::EndTurn as i32);
    assert_eq!(last_text(&resp), "echo t-1 seen=[m1]");
    assert_eq!(resp.usage.unwrap().cost_micro_usd, 0);
    assert!(h.anthropic.bodies().is_empty(), "no provider call");
}

#[tokio::test]
async fn a_marker_from_an_unlisted_tenant_goes_to_the_real_model() {
    let h = Harness::start_with(scripted_for_tenant_a, None).await;
    let resp = h
        .turn(FRANK_B, marked_turn("t-1", "@frank [[script:echo]] marker m1"))
        .await
        .expect("turn succeeds");
    assert_eq!(last_text(&resp), "done");
    let bodies = h.anthropic.bodies();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["messages"][0]["content"], "@frank [[script:echo]] marker m1");
}

#[tokio::test]
async fn a_listed_tenant_without_a_marker_goes_to_the_real_model() {
    let h = Harness::start_with(scripted_for_tenant_a, None).await;
    let resp = h.turn(FRANK_A, turn_request("t-1", false)).await.expect("turn succeeds");
    assert_eq!(last_text(&resp), "done");
    assert_eq!(h.anthropic.bodies().len(), 1);
}

#[tokio::test]
async fn an_unknown_script_is_invalid_argument() {
    let h = Harness::start_with(scripted_for_tenant_a, None).await;
    let err = h
        .turn(FRANK_A, marked_turn("t-1", "[[script:no-such-script]]"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert_eq!(err.message(), "unknown script: \"no-such-script\"");
    let err = h.turn(FRANK_A, marked_turn("t-1", "[[script:../echo]]")).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(h.anthropic.bodies().is_empty());
}

#[tokio::test]
async fn the_texas811_review_script_answers_a_completion_with_repair_step() {
    let h = Harness::start_with(scripted_for_tenant_a, None).await;
    let resp = h
        .wrapper
        .generate_completion_as_service(FRANK_A.signed(GenerateCompletionRequest {
            model_alias: "claude-sonnet".to_string(),
            messages: vec![user_text("Step 5 'review' [[script:texas811-review]]")],
            ..Default::default()
        }))
        .await
        .expect("scripted completion succeeds")
        .into_inner();
    let Some(llmproxy_model::types::services::llmproxy::llmproxy::generate_completion_response::Result::ToolUse(tool_use)) =
        resp.result
    else {
        panic!("expected a tool_use, got {:?}", resp.result)
    };
    assert_eq!(tool_use.name, "repair_step");
    let input: serde_json::Value = serde_json::from_str(&tool_use.input_json).unwrap();
    assert_eq!(
        input,
        serde_json::json!({
            "landmark": ["heading", "Review"],
            "actions": [{"action": "click", "target_role": "button", "target_name": "Save Draft"}],
        })
    );
    assert!(h.anthropic.bodies().is_empty());
}

// The caller list used to be a literal here (beagle's pigeon path pinned to
// pigeon_service_path_for); it is now auth_services_from_fleet, filled by the
// fleet build from the fleet's declared clients. See tacplus-dev/llmproxy#3.
