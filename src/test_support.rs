//! Mocks shared by the federation and service tests: the Anthropic Messages
//! API, the WIF token endpoint, and a kernel assertion source.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::federation::{AssertionSource, ASSERTION_AUDIENCE};

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

/// Mints kernel-shaped JWTs with `aud = ASSERTION_AUDIENCE` and a fresh jti
/// each call — what the generated token path will provide (BQ14).
#[derive(Default)]
pub struct MockAssertions {
    minted: AtomicU64,
}

#[tonic::async_trait]
impl AssertionSource for MockAssertions {
    async fn assertion(&self) -> Result<String, String> {
        let n = self.minted.fetch_add(1, Ordering::SeqCst);
        let key = key::keys::generate_ed25519_key().unwrap();
        let mut t = token::token::Token::new();
        t.set_issuer("https://fleet.test").unwrap();
        t.set_subject("spiffe://td.test/system/services/llmproxy").unwrap();
        t.set_namespace("/system").unwrap();
        t.set_audience(ASSERTION_AUDIENCE).unwrap();
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
#[derive(Default)]
pub struct MockAnthropic {
    pub requests: Mutex<Vec<(HeaderMap, Value)>>,
}

impl MockAnthropic {
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/v1/messages", post(Self::messages))
            .with_state(self.clone())
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
