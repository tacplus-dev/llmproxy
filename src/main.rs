mod anthropic;
mod budget;
mod callbacks;
mod federation;
mod scripted;
mod service;
mod session;
#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod test_support;

use std::error::Error;
use std::sync::Arc;

use llmproxy_model::s2sm::ServiceManager;

#[tokio::main]
async fn main() {
    // The chassis logger, NOT tracing_subscriber::fmt(): it redacts, which
    // matters everywhere in this fleet; it matters MORE here — this is the one
    // service in the fleet that ever holds the real LLM credential, and an
    // unredacted log line is the one place it could leave the entitlement
    // model behind.
    if let Err(e) = logger::install_bootstrap() {
        eprintln!("FATAL: could not install the redacting logger: {e}");
        std::process::exit(1);
    }

    if let Err(e) = run().await {
        tracing::error!(error = %e, "llmproxy failed");
        let mut source = e.source();
        while let Some(cause) = source {
            tracing::error!(cause = %cause, "caused by");
            source = cause.source();
        }
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    // ServiceManager::new() reads BOOTSTRAP_CONFIG, connects to kernel over
    // mTLS, and returns once the service has an identity and config. sm.run()
    // then serves the generated gRPC surface and blocks until shutdown.
    let sm = Arc::new(ServiceManager::new().await?);

    // llm_credential is NOT read here: sm.config() returns the chassis's own
    // base grpcconfig type, not this service's typed Config. The real
    // credential arrives through the generated llm_proxy_config callback
    // (service.rs), delivered before the server starts serving — the one and
    // only place in the whole fleet this value is read into a live HTTP
    // client. Operator must fan it out to ONLY this service's config
    // (llm_credential_services: ["llmproxy"], ConfigHasKeyMaterial: true so
    // it renders as a Secret) — see llmproxy.proto's Config.llm_credential.
    //
    // D22: the federation ids come from the pod env and the assertion from the
    // generated token path (federation.rs).
    let env_ids = federation::FederationIds::from_env(|name| std::env::var(name).ok())?;
    let svc = Arc::new(service::LlmProxyService::new(
        sm.config().spiffe_trust_domain.clone(),
        Some(Arc::new(federation::KernelAssertions(sm.clone()))),
        env_ids,
        scripted::SCRIPTS_DIR.into(),
    ));
    let handler = Arc::new(service::LlmProxyServiceHandler::new(svc));

    sm.run(handler).await
}
