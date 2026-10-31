//! Mocks shared by the federation and service tests: the Anthropic Messages
//! API, the WIF token endpoint, a kernel assertion source, and OpenAI and
//! Google (Vertex AI and the GKE metadata server) on one host.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::federation::AssertionSource;

/// Serves `router` on an ephemeral localhost port and returns its base URL.
pub async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

/// Mints kernel-shaped JWTs with the asked-for audience and a fresh jti each
/// call — what the generated token path will provide (BQ14).
#[derive(Default)]
pub struct MockAssertions {
    minted: AtomicU64,
}

#[tonic::async_trait]
impl AssertionSource for MockAssertions {
    async fn assertion(&self, audience: &str) -> Result<String, String> {
        let n = self.minted.fetch_add(1, Ordering::SeqCst);
        let key = key::keys::generate_ed25519_key().unwrap();
        let mut t = token::token::Token::new();
        t.set_issuer("https://fleet.test").unwrap();
        t.set_subject("spiffe://td.test/system/services/llmproxy").unwrap();
        t.set_namespace("/system").unwrap();
        t.set_audience(audience).unwrap();
        t.set_jti(format!("jti-{n}")).unwrap();
        t.set_expiration(now_secs() + 600).unwrap();
        t.sign(&key).unwrap();
        Ok(t.base64().unwrap().to_string())
    }
}

/// The WIF token endpoint. Each successful exchange returns `tok-<n>` with
/// the configured `expires_in`.
#[derive(Default)]
pub struct MockTokenEndpoint {
    pub bodies: Mutex<Vec<Value>>,
    pub expires_in: AtomicU64,
    pub fail: AtomicBool,
}

impl MockTokenEndpoint {
    pub fn new(expires_in: u64) -> Arc<Self> {
        let mock = Arc::new(Self::default());
        mock.expires_in.store(expires_in, Ordering::SeqCst);
        mock
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/v1/oauth/token", post(Self::exchange))
            .with_state(self.clone())
    }

    pub async fn start(expires_in: u64) -> (Arc<Self>, String) {
        let mock = Self::new(expires_in);
        let url = serve(mock.router()).await;
        (mock, url)
    }

    pub fn exchanges(&self) -> usize {
        self.bodies.lock().unwrap().len()
    }

    async fn exchange(State(mock): State<Arc<Self>>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
        // Long enough that concurrent callers overlap the one exchange.
        tokio::time::sleep(Duration::from_millis(50)).await;
        if mock.fail.load(Ordering::SeqCst) {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "unavailable"})));
        }
        let n = {
            let mut bodies = mock.bodies.lock().unwrap();
            bodies.push(body);
            bodies.len()
        };
        (
            StatusCode::OK,
            Json(json!({
                "access_token": format!("tok-{n}"),
                "token_type": "Bearer",
                "expires_in": mock.expires_in.load(Ordering::SeqCst),
            })),
        )
    }
}

/// The Messages API. When tools are offered it asks for `post_room` until the
/// last message carries a tool_result, then ends the turn with text. Usage is
/// 10 in / 5 out.
/// `/v1/models` lists `models` (ids), counting the listings.
#[derive(Default)]
pub struct MockAnthropic {
    pub requests: Mutex<Vec<(HeaderMap, Value)>>,
    pub models: Mutex<Vec<&'static str>>,
    pub listings: AtomicU64,
}

impl MockAnthropic {
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/v1/messages", post(Self::messages))
            .route("/v1/models", get(Self::list))
            .with_state(self.clone())
    }

    async fn list(State(mock): State<Arc<Self>>) -> Json<Value> {
        mock.listings.fetch_add(1, Ordering::SeqCst);
        let data: Vec<Value> = mock.models.lock().unwrap().iter().map(|id| json!({"id": id, "type": "model"})).collect();
        Json(json!({"data": data, "has_more": false}))
    }

    pub fn bodies(&self) -> Vec<Value> {
        self.requests.lock().unwrap().iter().map(|(_, b)| b.clone()).collect()
    }

    async fn messages(
        State(mock): State<Arc<Self>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let answered = body["messages"]
            .as_array()
            .and_then(|m| m.last())
            .and_then(|m| m["content"].as_array())
            .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
            || body.get("tools").is_none();
        let n = {
            let mut requests = mock.requests.lock().unwrap();
            requests.push((headers, body));
            requests.len()
        };
        let content = if answered {
            json!([{"type": "text", "text": "done"}])
        } else {
            json!([{"type": "tool_use", "id": format!("toolu_{n}"), "name": "post_room", "input": {"text": "on it"}}])
        };
        Json(json!({
            "id": format!("msg_{n}"),
            "type": "message",
            "role": "assistant",
            "content": content,
            "stop_reason": if answered { "end_turn" } else { "tool_use" },
            "usage": {"input_tokens": 10, "output_tokens": 5},
        }))
    }
}

/// One request a [`MockVendor`] saw.
#[derive(Clone)]
pub struct Seen {
    pub method: Method,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Value,
}

/// OpenAI and Google on one host: OpenAI's token exchange, model listing and
/// Chat Completions; the metadata server's token, Vertex AI's publisher model
/// listing and `generateContent`. Each answers with text naming the model it
/// was asked for; `openai_models` and `google_models` are the listings.
#[derive(Default)]
pub struct MockVendor {
    pub seen: Mutex<Vec<Seen>>,
    pub openai_models: Mutex<Vec<&'static str>>,
    /// (id, launch stage)
    pub google_models: Mutex<Vec<(&'static str, &'static str)>>,
}

impl MockVendor {
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new().fallback(Self::handle).with_state(self.clone())
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// The requests to paths containing `fragment`.
    pub fn to(&self, fragment: &str) -> Vec<Seen> {
        self.seen().into_iter().filter(|s| s.path.contains(fragment)).collect()
    }

    async fn handle(
        State(mock): State<Arc<Self>>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, Json<Value>) {
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let path = uri.path().to_string();
        mock.seen.lock().unwrap().push(Seen { method, path: path.clone(), headers, body: body.clone() });
        let reply = if path == "/oauth/token" {
            json!({"access_token": "oai-tok", "issued_token_type": "urn:ietf:params:oauth:token-type:access_token", "token_type": "Bearer", "expires_in": 3600})
        } else if path.starts_with("/computeMetadata/") {
            json!({"access_token": "goog-tok", "token_type": "Bearer", "expires_in": 3600})
        } else if path == "/v1/models" {
            let data: Vec<Value> = mock.openai_models.lock().unwrap().iter().map(|id| json!({"id": id, "object": "model", "owned_by": "openai"})).collect();
            json!({"object": "list", "data": data})
        } else if path == "/v1beta1/publishers/google/models" {
            let models: Vec<Value> = mock
                .google_models
                .lock()
                .unwrap()
                .iter()
                .map(|(id, stage)| json!({"name": format!("publishers/google/models/{id}"), "launchStage": stage}))
                .collect();
            json!({"publisherModels": models})
        } else if path == "/v1/chat/completions" {
            json!({
                "model": body["model"],
                "choices": [{"index": 0, "message": {"role": "assistant", "content": format!("{} says hi", body["model"].as_str().unwrap_or_default())}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5},
            })
        } else if let Some(model) = path.strip_suffix(":generateContent").and_then(|p| p.rsplit('/').next()) {
            json!({
                "candidates": [{"content": {"role": "model", "parts": [{"text": format!("{model} says hi")}]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 5},
            })
        } else {
            return (StatusCode::NOT_FOUND, Json(json!({"error": path})));
        };
        (StatusCode::OK, Json(reply))
    }
}
