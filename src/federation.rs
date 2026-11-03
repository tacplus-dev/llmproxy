//! D22: the Anthropic credential by jwt-bearer exchange instead of a static key
//! (the slices backlog, PROVENANCE.md, §4).
//!
//! get a kernel JWT (aud = https://api.anthropic.com, fresh jti) → `POST
//! {base}{oauth_token_path}` with the RFC 7523 jwt-bearer grant and the four
//! federation ids → cache the `sk-ant-oat01-…` token → refresh on two tiers.
//! Request and response shapes are the ones pinned in Anthropic's Workload
//! Identity Federation docs (BQ15): a JSON body, `access_token` + `expires_in`
//! back, and the token sent as `Authorization: Bearer` (anthropic.rs).
//!
//! THE JWT IS NOT MINTED HERE. It comes through [`AssertionSource`], which
//! [`KernelAssertions`] backs with the generated
//! `ServiceManager::token_for_external` (tacplus bf5f32ec, LI1): a new
//! kernel-signed token on every call, never cached. Until kernel declares
//! `MintExternalServiceToken` the generated accessor returns an error, so an
//! exchange fails rather than falling back to the static key.
//!
//! The cache and its refresh tiers are the same for every vendor (#13): what
//! differs is the one exchange, an [`Exchange`]. Anthropic's is
//! [`AnthropicExchange`]; OpenAI's (openai.rs) presents the same kind of kernel
//! JWT in an RFC 8693 token exchange, and Google's (vertex.rs) asks the GKE
//! metadata server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use llmproxy_model::s2sm::ServiceManager;
use serde::{Deserialize, Serialize};

const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// The assertion's `aud`: what the federation rule matches
/// (ops/anthropic-federation.yaml `common.audience`).
pub const ASSERTION_AUDIENCE: &str = "https://api.anthropic.com";

/// Below this much remaining lifetime a background refresh starts and callers
/// keep the cached token (BQ16 default).
const ADVISORY_REFRESH: Duration = Duration::from_secs(5 * 60);
/// Below this much remaining lifetime callers wait for the refresh (BQ16
/// default).
const MANDATORY_REFRESH: Duration = Duration::from_secs(60);

/// A fresh kernel-signed JWT for an exchange, `aud = audience` (Anthropic's
/// is ASSERTION_AUDIENCE), a new `jti` on every call (WIF rejects a reused jti).
#[tonic::async_trait]
pub trait AssertionSource: Send + Sync {
    async fn assertion(&self, audience: &str) -> Result<String, String>;
}

/// The generated token path (BQ14).
pub struct KernelAssertions(pub Arc<ServiceManager>);

#[tonic::async_trait]
impl AssertionSource for KernelAssertions {
    async fn assertion(&self, audience: &str) -> Result<String, String> {
        // trace: unrelated: the token is cached and shared across requests
        self.0.token_for_external(audience).await.map_err(|e| e.to_string())
    }
}

/// The four ids of `llmproxy.Config` §4.3. Federation mode is selected only
/// when all four are set.
#[derive(Clone)]
pub struct FederationIds {
    pub federation_rule_id: String,
    pub organization_id: String,
    pub service_account_id: String,
    pub workspace_id: String,
}

impl FederationIds {
    /// All four set → Some, none set → None, some set → an error.
    pub fn from_parts(
        federation_rule_id: &str,
        organization_id: &str,
        service_account_id: &str,
        workspace_id: &str,
    ) -> Result<Option<Self>, String> {
        let ids = [federation_rule_id, organization_id, service_account_id, workspace_id];
        if ids.iter().all(|id| id.is_empty()) {
            return Ok(None);
        }
        if ids.iter().any(|id| id.is_empty()) {
            return Err("federation needs all four anthropic_* ids set, or none".to_string());
        }
        Ok(Some(Self {
            federation_rule_id: federation_rule_id.to_string(),
            organization_id: organization_id.to_string(),
            service_account_id: service_account_id.to_string(),
            workspace_id: workspace_id.to_string(),
        }))
    }

    /// The ids the operator puts on the pod from GitLab variables
    /// (the WIF setup, PROVENANCE.md). ANTHROPIC_API_KEY is never
    /// read: it would shadow federation.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let get = |name: &str| var(name).unwrap_or_default();
        Self::from_parts(
            &get("ANTHROPIC_FEDERATION_RULE_ID"),
            &get("ANTHROPIC_ORGANIZATION_ID"),
            &get("ANTHROPIC_SERVICE_ACCOUNT_ID"),
            &get("ANTHROPIC_WORKSPACE_ID"),
        )
    }
}

#[derive(Clone)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// A token one exchange issued, and how long it lives.
pub struct Issued {
    pub token: String,
    pub expires_in: Duration,
}

/// One vendor's exchange: obtain a new provider token. [`Federation`] decides
/// when to call it.
#[tonic::async_trait]
pub trait Exchange: Send + Sync {
    async fn exchange(&self, http: &reqwest::Client) -> Result<Issued, String>;
}

/// POSTs a JSON token request and reads `access_token` + `expires_in` back:
/// the reply shape Anthropic's and OpenAI's token endpoints share.
pub async fn post_token_request(
    http: &reqwest::Client,
    url: &str,
    body: &impl Serialize,
) -> Result<Issued, String> {
    // trace: unrelated: the token is cached and shared across requests
    let resp = http
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("token endpoint unreachable: {e}"))?;
    read_issued(resp).await
}

/// A token endpoint's reply: an error for a non-2xx, else the token.
pub async fn read_issued(resp: reqwest::Response) -> Result<Issued, String> {
    #[derive(Deserialize)]
    struct Response {
        access_token: String,
        expires_in: u64,
    }

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("reading token endpoint reply: {e}"))?;
    if !status.is_success() {
        return Err(format!("token endpoint returned {status}: {text}"));
    }
    let parsed: Response = serde_json::from_str(&text)
        .map_err(|e| format!("unexpected token endpoint reply: {e}"))?;
    Ok(Issued {
        token: parsed.access_token,
        expires_in: Duration::from_secs(parsed.expires_in),
    })
}

/// Anthropic's jwt-bearer exchange with the four federation ids.
pub struct AnthropicExchange {
    token_url: String,
    ids: FederationIds,
    assertions: Arc<dyn AssertionSource>,
}

#[tonic::async_trait]
impl Exchange for AnthropicExchange {
    async fn exchange(&self, http: &reqwest::Client) -> Result<Issued, String> {
        #[derive(Serialize)]
        struct Request<'a> {
            grant_type: &'a str,
            assertion: &'a str,
            federation_rule_id: &'a str,
            organization_id: &'a str,
            service_account_id: &'a str,
            workspace_id: &'a str,
        }

        // trace: unrelated: the token is cached and shared across requests
        let assertion = self
            .assertions
            .assertion(ASSERTION_AUDIENCE)
            .await
            .map_err(|e| format!("no kernel assertion: {e}"))?;
        post_token_request(
            http,
            &self.token_url,
            &Request {
                grant_type: JWT_BEARER_GRANT,
                assertion: &assertion,
                federation_rule_id: &self.ids.federation_rule_id,
                organization_id: &self.ids.organization_id,
                service_account_id: &self.ids.service_account_id,
                workspace_id: &self.ids.workspace_id,
            },
        )
        .await
    }
}

pub struct Federation {
    http: reqwest::Client,
    /// Whose token this is, for errors and logs ("Anthropic").
    vendor: &'static str,
    exchanger: Box<dyn Exchange>,
    cached: Mutex<Option<CachedToken>>,
    /// Held for the whole of one exchange: the single flight.
    exchange: tokio::sync::Mutex<()>,
    background_refresh: AtomicBool,
}

impl Federation {
    /// Anthropic's federation (D22).
    pub fn new(token_url: String, ids: FederationIds, assertions: Arc<dyn AssertionSource>) -> Self {
        Self::with_exchange("Anthropic", Box::new(AnthropicExchange { token_url, ids, assertions }))
    }

    pub fn with_exchange(vendor: &'static str, exchanger: Box<dyn Exchange>) -> Self {
        Self {
            http: reqwest::Client::new(),
            vendor,
            exchanger,
            cached: Mutex::new(None),
            exchange: tokio::sync::Mutex::new(()),
            background_refresh: AtomicBool::new(false),
        }
    }

    /// The current provider token. Fresh enough → returned as is. Inside the
    /// advisory window → returned as is, and one background refresh starts.
    /// Inside the mandatory window or absent → the caller waits for the single
    /// in-flight exchange. A failed exchange keeps serving a still-valid
    /// cached token; only expired-and-failing is an error.
    pub async fn token(self: &Arc<Self>) -> Result<String, String> {
        if let Some(cached) = self.cached() {
            let remaining = cached.expires_at.saturating_duration_since(Instant::now());
            if remaining > ADVISORY_REFRESH {
                return Ok(cached.token);
            }
            if remaining > MANDATORY_REFRESH {
                self.start_background_refresh();
                return Ok(cached.token);
            }
        }

        let _flight = self.exchange.lock().await;
        // Another caller may have finished the exchange while this one waited.
        if let Some(cached) = self.cached() {
            if cached.expires_at.saturating_duration_since(Instant::now()) > MANDATORY_REFRESH {
                return Ok(cached.token);
            }
        }
        match self.exchange_once().await {
            Ok(token) => Ok(token),
            Err(e) => match self.cached() {
                Some(cached) if cached.expires_at > Instant::now() => {
                    tracing::warn!(vendor = self.vendor, error = %e, "token exchange failed; serving the cached token until it expires");
                    Ok(cached.token)
                }
                _ => Err(format!(
                    "{} token exchange failed and no unexpired token is cached: {e}",
                    self.vendor
                )),
            },
        }
    }

    fn cached(&self) -> Option<CachedToken> {
        self.cached.lock().unwrap().clone()
    }

    fn start_background_refresh(self: &Arc<Self>) {
        if self.background_refresh.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            {
                let _flight = this.exchange.lock().await;
                let still_due = this.cached().is_none_or(|c| {
                    c.expires_at.saturating_duration_since(Instant::now()) <= ADVISORY_REFRESH
                });
                if still_due {
                    if let Err(e) = this.exchange_once().await {
                        tracing::warn!(vendor = this.vendor, error = %e, "advisory token refresh failed; the cached token is still served");
                    }
                }
            }
            this.background_refresh.store(false, Ordering::SeqCst);
        });
    }

    /// One exchange. Caller holds `exchange`.
    async fn exchange_once(&self) -> Result<String, String> {
        let issued = self.exchanger.exchange(&self.http).await?;
        *self.cached.lock().unwrap() = Some(CachedToken {
            token: issued.token.clone(),
            expires_at: Instant::now() + issued.expires_in,
        });
        Ok(issued.token)
    }
}

#[cfg(test)]
#[path = "federation_tests.rs"]
mod tests;
