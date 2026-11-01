//! The vendors behind the aliases (#13): Anthropic (anthropic.rs), OpenAI
//! (openai.rs) and Google's Gemini on Vertex AI (vertex.rs), each through its
//! own API and a keyless credential. [`Providers`] resolves an alias to a
//! vendor and model, then calls that vendor.
//!
//! A vendor takes live traffic only once Jody has recorded its data terms in
//! the README (no training on API inputs, and how long it keeps them):
//! [`LIVE_VENDORS`] is that record in code, so turning a vendor on is a
//! reviewed change beside the README's, never a config push. A route to any
//! other vendor is refused before anything is sent to it, model listings
//! included.

use std::time::Duration;

use llmproxy_model::types::services::llmproxy::llmproxy::{
    content_block::Block, GenerateCompletionRequest, GenerateCompletionResponse, ModelAlias, Role as ProtoRole,
    Vendor,
};

use crate::anthropic::AnthropicClient;
use crate::openai::OpenAiClient;
use crate::routes::{pick_latest, ListingCache, ModelTable};
use crate::vertex::VertexClient;

/// The vendors whose data terms the README records and whose deployment is set
/// up (README, Vendors). OpenAI and Google wait on their setup (#14).
pub const LIVE_VENDORS: &[Vendor] = &[Vendor::Anthropic];

/// The vendor's name in errors and logs.
pub fn vendor_name(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Unspecified | Vendor::Anthropic => "Anthropic",
        Vendor::Openai => "OpenAI",
        Vendor::Google => "Google Vertex AI",
    }
}

#[derive(Debug)]
pub enum ProviderError {
    UnknownModelAlias(String),
    Request(Vendor, reqwest::Error),
    Api { vendor: Vendor, status: u16, body: String },
    UnexpectedResponseShape(Vendor, String),
    /// No provider credential could be obtained (federation: the cached token
    /// is expired and the exchange is failing).
    Credential(String),
    /// The vendor's data terms are not recorded yet ([`LIVE_VENDORS`]).
    NotLive(Vendor),
    /// A route names a vendor this deployment has no credential config for.
    NotConfigured(Vendor),
    /// The request asks for something the route's vendor is not used for here.
    Unsupported(String),
    /// A `latest` route whose pattern matches nothing in the vendor's listing.
    NoLatestMatch(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::UnknownModelAlias(a) => write!(f, "unknown model_alias: {a}"),
            ProviderError::Request(v, e) => write!(f, "request to {} failed: {e}", vendor_name(*v)),
            ProviderError::Api { vendor, status, body } => {
                write!(f, "{} API returned {status}: {body}", vendor_name(*vendor))
            }
            ProviderError::UnexpectedResponseShape(v, s) => {
                write!(f, "unexpected {} response shape: {s}", vendor_name(*v))
            }
            ProviderError::Credential(s) => write!(f, "no provider credential: {s}"),
            ProviderError::NotLive(v) => write!(
                f,
                "{} is not live: its data terms are not recorded in llmproxy's README yet",
                vendor_name(*v)
            ),
            ProviderError::NotConfigured(v) => {
                write!(f, "{} is not configured on this llmproxy", vendor_name(*v))
            }
            ProviderError::Unsupported(s) => write!(f, "{s}"),
            ProviderError::NoLatestMatch(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// One model a vendor's listing names. `preview`: not generally available.
#[derive(Debug, Clone)]
pub struct ListedModel {
    pub id: String,
    pub preview: bool,
}

/// An id that names itself a preview or an experiment.
pub fn looks_preview(id: &str) -> bool {
    id.contains("preview") || id.contains("-exp") || id.contains("experimental")
}

/// Sends a vendor request and returns its parsed JSON reply; a non-2xx is
/// [`ProviderError::Api`] with the vendor's own error body.
pub async fn send_json(vendor: Vendor, request: reqwest::RequestBuilder) -> Result<serde_json::Value, ProviderError> {
    let resp = request.send().await.map_err(|e| ProviderError::Request(vendor, e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| ProviderError::Request(vendor, e))?;
    if !status.is_success() {
        return Err(ProviderError::Api { vendor, status: status.as_u16(), body: text });
    }
    serde_json::from_str(&text)
        .map_err(|e| ProviderError::UnexpectedResponseShape(vendor, format!("invalid JSON: {e}")))
}

/// One message of a text-only conversation: its role and its text blocks.
pub struct TextMessage {
    pub assistant: bool,
    pub texts: Vec<String>,
}

/// The request as plain text turns, for the vendors used text in, text out
/// (OpenAI, Google): tools and tool blocks are refused, not dropped.
pub fn text_only(vendor: Vendor, req: &GenerateCompletionRequest) -> Result<Vec<TextMessage>, ProviderError> {
    let refuse = || {
        ProviderError::Unsupported(format!(
            "{} routes are text in, text out: no tools, tool_use or tool_result",
            vendor_name(vendor)
        ))
    };
    if !req.tools.is_empty() {
        return Err(refuse());
    }
    req.messages
        .iter()
        .map(|m| {
            let texts = m
                .content
                .iter()
                .map(|b| match &b.block {
                    Some(Block::Text(text)) => Ok(text.clone()),
                    _ => Err(refuse()),
                })
                .collect::<Result<_, _>>()?;
            Ok(TextMessage {
                assistant: ProtoRole::try_from(m.role) == Ok(ProtoRole::Assistant),
                texts,
            })
        })
        .collect()
}

/// What an alias means for one call.
pub struct Resolved {
    pub vendor: Vendor,
    pub provider_model: String,
    pub input_micro_usd_per_token: i64,
    pub output_micro_usd_per_token: i64,
}

/// Every vendor client of one config, and the alias table they serve.
pub struct Providers {
    pub models: ModelTable,
    pub anthropic: AnthropicClient,
    pub openai: Option<OpenAiClient>,
    pub vertex: Option<VertexClient>,
    pub live: Vec<Vendor>,
    pub listings: ListingCache,
}

impl Providers {
    /// An empty table and no vendor: what serves before the first config.
    pub fn empty() -> Self {
        Self {
            models: ModelTable::default(),
            anthropic: AnthropicClient::new(crate::anthropic::Credential::ApiKey(String::new()), None),
            openai: None,
            vertex: None,
            live: LIVE_VENDORS.to_vec(),
            listings: ListingCache::new(Duration::ZERO),
        }
    }

    /// The alias's vendor and model now: a pinned model as configured, a
    /// `latest` one from the vendor's (cached) listing. Refused before any
    /// traffic when the vendor is not live or not configured.
    pub async fn resolve(&self, alias: &str) -> Result<Resolved, ProviderError> {
        let route = self.models.route(alias)?;
        let vendor = route.vendor;
        if !self.live.contains(&vendor) {
            return Err(ProviderError::NotLive(vendor));
        }
        let configured = match vendor {
            Vendor::Openai => self.openai.is_some(),
            Vendor::Google => self.vertex.is_some(),
            _ => true,
        };
        if !configured {
            return Err(ProviderError::NotConfigured(vendor));
        }
        let provider_model = match &route.latest {
            None => route.route.provider_model.clone(),
            Some(pattern) => {
                let listing = self.listings.get(vendor, || self.list(vendor)).await?;
                pick_latest(&listing, pattern, route.route.allow_preview)
                    .ok_or_else(|| {
                        ProviderError::NoLatestMatch(format!(
                            "model {alias}: no {} model matches latest_pattern {}",
                            vendor_name(vendor),
                            route.route.latest_pattern
                        ))
                    })?
                    .to_string()
            }
        };
        Ok(Resolved {
            vendor,
            provider_model,
            input_micro_usd_per_token: route.route.input_micro_usd_per_token,
            output_micro_usd_per_token: route.route.output_micro_usd_per_token,
        })
    }

    async fn list(&self, vendor: Vendor) -> Result<Vec<crate::providers::ListedModel>, ProviderError> {
        match vendor {
            Vendor::Openai => self.openai()?.list_models().await,
            Vendor::Google => self.vertex()?.list_models().await,
            _ => self.anthropic.list_models().await,
        }
    }

    fn openai(&self) -> Result<&OpenAiClient, ProviderError> {
        self.openai.as_ref().ok_or(ProviderError::NotConfigured(Vendor::Openai))
    }

    fn vertex(&self) -> Result<&VertexClient, ProviderError> {
        self.vertex.as_ref().ok_or(ProviderError::NotConfigured(Vendor::Google))
    }

    /// One completion, on whichever vendor the alias resolves to; the response
    /// says which model answered.
    pub async fn complete(&self, req: &GenerateCompletionRequest) -> Result<GenerateCompletionResponse, ProviderError> {
        let resolved = self.resolve(&req.model_alias).await?;
        let model = resolved.provider_model.as_str();
        let mut resp = match resolved.vendor {
            Vendor::Openai => self.openai()?.complete(model, req).await?,
            Vendor::Google => self.vertex()?.complete(model, req).await?,
            _ => self.anthropic.generate_completion(model, req).await?,
        };
        resp.provider_model = resolved.provider_model;
        resp.vendor = resolved.vendor as i32;
        Ok(resp)
    }

    /// The aliases served and what each resolves to now (ListModels): an alias
    /// that cannot be resolved now is listed with no provider_model.
    pub async fn list_aliases(&self) -> Vec<ModelAlias> {
        let mut aliases = Vec::new();
        for name in self.models.aliases() {
            let vendor = self.models.route(name).map(|r| r.vendor).unwrap_or(Vendor::Anthropic);
            let provider_model = match self.resolve(name).await {
                Ok(resolved) => resolved.provider_model,
                Err(e) => {
                    tracing::warn!(alias = %name, error = %e, "model alias does not resolve now");
                    String::new()
                }
            };
            aliases.push(ModelAlias {
                name: name.to_string(),
                provider_model,
                vendor: vendor as i32,
                ..Default::default()
            });
        }
        aliases
    }
}
