//! LlmProxyService: the handler for llmproxy's RPCs.
//!
//! Phase 1 (decisions/llm-proxy-design-proposal-2026-09-14.md): plain
//! service-token auth (`option (spec.service_s2s)` on the whole `LlmProxy`
//! service in llmproxy.proto), no delegated/user-token path, no lawful
//! intercept. Phase 2 adds the A29 session path (session.rs, callbacks.rs,
//! budget.rs) and the D22 federation credential (federation.rs).

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock};

use servicemanager::auth::Caller;
use tonic::Status;

use llmproxy_model::s2sm::handler::Handler;
use llmproxy_model::s2sm::internal::grpc::servers::services::llmproxy::llm_proxy::LlmProxyHandler;
use llmproxy_model::types::services::llmproxy::llmproxy::BudgetBucket;
use llmproxy_model::types::services::llmproxy::llmproxy::{
    BucketBalance, Config as LlmProxyConfig, GenerateCompletionRequest, GenerateCompletionResponse,
    GetBudgetRequest, GetBudgetResponse, ListModelsRequest, ListModelsResponse,
    Message as ProtoMessage, SessionTurnRequest, SessionTurnResponse,
};
use llmproxy_model::types::sys::healthreport::healthreport::InstanceStatus;

use crate::anthropic::{
    AnthropicClient, AnthropicError, Credential, ModelTable, ANTHROPIC_API_BASE,
};
use crate::budget::Ledger;
use crate::callbacks::{CallbackRegistry, InboundStream, OutboundStream};
use crate::federation::{AssertionSource, Federation, FederationIds};
use crate::scripted::Script;

pub(crate) fn to_status(e: AnthropicError) -> Status {
    match e {
        AnthropicError::UnknownModelAlias(a) => {
            Status::invalid_argument(format!("unknown model_alias: {a}"))
        }
        // The caller's own request shape (a bad tool schema, etc) rather than an
        // upstream failure.
        AnthropicError::UnexpectedResponseShape(s) if s.contains("input_schema_json") => {
            Status::invalid_argument(s)
        }
        AnthropicError::Request(e) => Status::unavailable(format!("Anthropic unreachable: {e}")),
        AnthropicError::Api { status, body } => {
            // Never echo the REQUEST back (it could carry a caller's tool
            // arguments); the status and Anthropic's own error body are enough
            // for a caller to act on, and neither ever contains the credential —
            // that's sent as a header this service alone attaches, never part of
            // any response we relay.
            Status::internal(format!("Anthropic API error {status}: {body}"))
        }
        AnthropicError::UnexpectedResponseShape(s) => {
            Status::internal(format!("unexpected Anthropic response: {s}"))
        }
        AnthropicError::Credential(s) => Status::unavailable(s),
    }
}

pub struct LlmProxyService {
    // Config arrives only through the async llm_proxy_config callback below,
    // delivered before the server starts and again on every kernel push — NOT
    // through sm.config(), which returns the chassis's own base grpcconfig
    // type, not this service's typed Config. So the client has to be
    // rebuildable after construction; the Arc makes swapping it cheap and lets
    // an in-flight call finish against the client it started with rather than
    // racing a concurrent config push.
    anthropic: RwLock<Arc<AnthropicClient>>,
    config: RwLock<Arc<LlmProxyConfig>>,
    /// The fleet trust domain, for forming caller SPIFFE IDs.
    trust_domain: String,
    /// The D22 JWT source (federation.rs module doc).
    assertions: Option<Arc<dyn AssertionSource>>,
    /// The federation ids from the pod env; when set they win over Config's.
    env_ids: Option<FederationIds>,
    /// Where the harness-scripted stand-in's scripts are (scripted.rs).
    scripts_dir: PathBuf,
    pub(crate) callbacks: Arc<CallbackRegistry>,
    pub(crate) ledger: Mutex<Ledger>,
    pub(crate) next_frame: AtomicU64,
}

impl LlmProxyService {
    /// Constructed with an empty client — real credentials arrive via
    /// set_config before the server actually starts serving.
    pub fn new(
        trust_domain: String,
        assertions: Option<Arc<dyn AssertionSource>>,
        env_ids: Option<FederationIds>,
        scripts_dir: PathBuf,
    ) -> Self {
        Self {
            anthropic: RwLock::new(Arc::new(AnthropicClient::new(
                Credential::ApiKey(String::new()),
                None,
                ModelTable::default(),
            ))),
            config: RwLock::new(Arc::new(LlmProxyConfig::default())),
            trust_domain,
            assertions,
            env_ids,
            scripts_dir,
            callbacks: Arc::new(CallbackRegistry::default()),
            ledger: Mutex::new(Ledger::default()),
            next_frame: AtomicU64::new(0),
        }
    }

    pub fn set_config(&self, cfg: &LlmProxyConfig) -> Result<(), String> {
        let base_url = (!cfg.default_provider_base_url.is_empty())
            .then(|| cfg.default_provider_base_url.clone());
        // Mode select (D22 §4.2 step 6): all four federation ids set (env,
        // else Config) → the jwt-bearer exchange, llm_credential ignored; none
        // set → the static key (transition only).
        let ids = match &self.env_ids {
            Some(ids) => Some(ids.clone()),
            None => federation_ids(cfg)?,
        };
        let credential = match ids {
            Some(ids) => {
                let assertions = self.assertions.clone().ok_or(
                    "federation ids are set but this build has no kernel assertion source",
                )?;
                let base = base_url.as_deref().unwrap_or(ANTHROPIC_API_BASE);
                Credential::Federation(Arc::new(Federation::new(
                    format!("{base}{}", cfg.oauth_token_path),
                    ids,
                    assertions,
                )))
            }
            None => Credential::ApiKey(cfg.llm_credential.clone()),
        };
        *self.anthropic.write().unwrap() = Arc::new(AnthropicClient::new(
            credential,
            base_url,
            ModelTable::new(&cfg.models),
        ));
        *self.config.write().unwrap() = Arc::new(cfg.clone());
        self.callbacks.close_matching(&cfg.blocked_callers);
        Ok(())
    }

    pub(crate) fn config(&self) -> Arc<LlmProxyConfig> {
        self.config.read().unwrap().clone()
    }

    pub(crate) fn anthropic(&self) -> Arc<AnthropicClient> {
        // Clone the Arc out from under the lock (rather than holding the read
        // guard across an .await) so a config push mid-flight never blocks
        // on, or is blocked by, an in-flight call.
        self.anthropic.read().unwrap().clone()
    }

    /// The caller's SPIFFE ID, tenant-qualified (`spiffe://<td>/tenants/a/
    /// services/frank`): the token's namespace scope plus its service path.
    /// `Token::subject_spiffe_id` is location-agnostic for services and would
    /// give two tenants' dogs the same identity. Refuses a caller matched by
    /// `Config.blocked_callers` (kill switch, BQ7).
    fn caller_id(&self, caller: Option<&Caller>) -> Result<String, Status> {
        let token = caller
            .ok_or_else(|| Status::unauthenticated("no authenticated caller"))?
            .token();
        let scope = token
            .namespace_spiffe_id(&self.trust_domain)
            .map_err(|e| Status::permission_denied(format!("caller has no namespace: {e}")))?;
        let service = token
            .service_path()
            .map_err(|e| Status::permission_denied(format!("caller is not a service: {e}")))?;
        let id = format!("{scope}/services/{service}");
        if self
            .config()
            .blocked_callers
            .iter()
            .any(|p| id.starts_with(p.as_str()))
        {
            return Err(Status::permission_denied(format!("caller {id} is blocked")));
        }
        Ok(id)
    }

    /// The harness-scripted stand-in for this request (scripted.rs): only for
    /// a caller in a `Config.scripted_model_tenants` tenant, and only when the
    /// newest user turn names a script.
    pub(crate) fn script_for(
        &self,
        caller: &str,
        messages: &[ProtoMessage],
    ) -> Result<Option<Script>, Status> {
        let listed = self.config().scripted_model_tenants.iter().any(|tenant| {
            spiffe::tenant_spiffe_id(&self.trust_domain, tenant)
                .is_ok_and(|id| caller.starts_with(&format!("{id}/")))
        });
        if !listed {
            return Ok(None);
        }
        let script = Script::select(&self.scripts_dir, messages)?;
        if let Some(script) = &script {
            tracing::info!(caller = %caller, script = %script.name(), "scripted model turn");
        }
        Ok(script)
    }
}

fn federation_ids(cfg: &LlmProxyConfig) -> Result<Option<FederationIds>, String> {
    FederationIds::from_parts(
        &cfg.anthropic_federation_rule_id,
        &cfg.anthropic_organization_id,
        &cfg.anthropic_service_account_id,
        &cfg.anthropic_workspace_id,
    )
}

#[tonic::async_trait]
impl LlmProxyHandler for LlmProxyService {
    async fn generate_completion_as_service(
        &self,
        caller: Option<&Caller>,
        req: GenerateCompletionRequest,
    ) -> Result<GenerateCompletionResponse, Status> {
        let caller = self.caller_id(caller)?;
        if let Some(script) = self.script_for(&caller, &req.messages)? {
            return script.completion(&req.messages);
        }
        self.anthropic()
            .generate_completion(&req)
            .await
            .map_err(to_status)
    }

    async fn generate_session_turn(
        &self,
        caller: Option<&Caller>,
        req: SessionTurnRequest,
    ) -> Result<SessionTurnResponse, Status> {
        let caller = self.caller_id(caller)?;
        self.run_turn(caller, req).await
    }

    async fn broker_tool_callback(
        &self,
        caller: Option<&Caller>,
        req: tonic::Request<InboundStream>,
    ) -> Result<tonic::Response<OutboundStream>, Status> {
        let caller = self.caller_id(caller)?;
        Ok(tonic::Response::new(
            self.callbacks.open(caller, req.into_inner()),
        ))
    }

    async fn get_budget(
        &self,
        caller: Option<&Caller>,
        _req: GetBudgetRequest,
    ) -> Result<GetBudgetResponse, Status> {
        let caller = self.caller_id(caller)?;
        let cfg = self.config();
        let mut ledger = self.ledger.lock().unwrap();
        let buckets = [
            BudgetBucket::DayJob,
            BudgetBucket::AboveAndBeyond,
            BudgetBucket::Reserve,
            BudgetBucket::Personal,
        ]
        .into_iter()
        .map(|bucket| BucketBalance {
            bucket: bucket as i32,
            limit_micro_usd: Ledger::limit(&cfg, bucket),
            spent_micro_usd: ledger.spent(&caller, bucket),
            ..Default::default()
        })
        .collect();
        Ok(GetBudgetResponse {
            buckets,
            resets_at_ms: Ledger::resets_at_ms(),
            ..Default::default()
        })
    }

    async fn list_models(
        &self,
        caller: Option<&Caller>,
        _req: ListModelsRequest,
    ) -> Result<ListModelsResponse, Status> {
        self.caller_id(caller)?;
        Ok(ListModelsResponse {
            model_aliases: self.anthropic().models().aliases(),
            ..Default::default()
        })
    }
}

/// LlmProxyServiceHandler is the lifecycle handler passed to sm.run(), same
/// shape as every other service in this fleet (see services/weather/src/
/// service.rs's WeatherServiceHandler).
pub struct LlmProxyServiceHandler {
    svc: Arc<LlmProxyService>,
}

impl LlmProxyServiceHandler {
    pub fn new(svc: Arc<LlmProxyService>) -> Self {
        Self { svc }
    }
}

#[async_trait::async_trait]
impl Handler for LlmProxyServiceHandler {
    fn llm_proxy_grpc_impl(&self) -> Arc<dyn LlmProxyHandler> {
        self.svc.clone()
    }

    async fn llm_proxy_config(
        &self,
        cfg: &LlmProxyConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Called once before the server starts (with the real llm_credential
        // already present) and again on every kernel push, so a credential
        // rotation, a federation cutover or a kill-switch entry takes effect
        // with no restart.
        self.svc.set_config(cfg).map_err(Into::into)
    }

    fn check_health(&self) -> InstanceStatus {
        InstanceStatus::Healthy
    }

    async fn shutdown(&self) {}
}
