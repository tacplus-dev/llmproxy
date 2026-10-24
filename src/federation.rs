//! D22: the Anthropic credential by jwt-bearer exchange instead of a static key
//! (decisions/backlog-scotty-slices-2026-09-30.md §4).
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

/// A fresh kernel-signed JWT for the exchange, `aud = ASSERTION_AUDIENCE`, a new
/// `jti` on every call (WIF rejects a reused jti).
#[tonic::async_trait]
pub trait AssertionSource: Send + Sync {
    async fn assertion(&self) -> Result<String, String>;
}

/// The generated token path (BQ14).
pub struct KernelAssertions(pub Arc<ServiceManager>);

#[tonic::async_trait]
impl AssertionSource for KernelAssertions {
    async fn assertion(&self) -> Result<String, String> {
        self.0
            .token_for_external(ASSERTION_AUDIENCE)
            .await
            .map_err(|e| e.to_string())
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
        let ids = [
            federation_rule_id,
            organization_id,
            service_account_id,
            workspace_id,
        ];
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
    /// (decisions/anthropic-wif-setup-2026-10-03.md). ANTHROPIC_API_KEY is never
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

pub struct Federation {
    http: reqwest::Client,
    token_url: String,
    ids: FederationIds,
    assertions: Arc<dyn AssertionSource>,
    cached: Mutex<Option<CachedToken>>,
    /// Held for the whole of one exchange: the single flight.
    exchange: tokio::sync::Mutex<()>,
    background_refresh: AtomicBool,
}

impl Federation {
    pub fn new(
        token_url: String,
        ids: FederationIds,
        assertions: Arc<dyn AssertionSource>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            token_url,
            ids,
            assertions,
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
                    tracing::warn!(error = %e, "Anthropic token exchange failed; serving the cached token until it expires");
                    Ok(cached.token)
                }
                _ => Err(format!(
                    "Anthropic token exchange failed and no unexpired token is cached: {e}"
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
                        tracing::warn!(error = %e, "advisory Anthropic token refresh failed; the cached token is still served");
                    }
                }
            }
            this.background_refresh.store(false, Ordering::SeqCst);
        });
    }

    /// One exchange with a freshly obtained assertion. Caller holds `exchange`.
    async fn exchange_once(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Request<'a> {
            grant_type: &'a str,
            assertion: &'a str,
            federation_rule_id: &'a str,
            organization_id: &'a str,
            service_account_id: &'a str,
            workspace_id: &'a str,
        }
        #[derive(Deserialize)]
        struct Response {
            access_token: String,
            expires_in: u64,
        }

        let assertion = self
            .assertions
            .assertion()
            .await
            .map_err(|e| format!("no kernel assertion: {e}"))?;
        let resp = self
            .http
            .post(&self.token_url)
            .json(&Request {
                grant_type: JWT_BEARER_GRANT,
                assertion: &assertion,
                federation_rule_id: &self.ids.federation_rule_id,
                organization_id: &self.ids.organization_id,
                service_account_id: &self.ids.service_account_id,
                workspace_id: &self.ids.workspace_id,
            })
            .send()
            .await
            .map_err(|e| format!("token endpoint unreachable: {e}"))?;
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

        *self.cached.lock().unwrap() = Some(CachedToken {
            token: parsed.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(parsed.expires_in),
        });
        Ok(parsed.access_token)
    }
}

#[cfg(test)]
#[path = "federation_tests.rs"]
mod tests;
