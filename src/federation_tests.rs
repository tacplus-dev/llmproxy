//! D22 §4.6 item 1, against a mock WIF token endpoint.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::test_support::{MockAssertions, MockTokenEndpoint};

fn ids() -> FederationIds {
    FederationIds {
        federation_rule_id: "fdrl_test".to_string(),
        organization_id: "00000000-0000-0000-0000-000000000001".to_string(),
        service_account_id: "svac_test".to_string(),
        workspace_id: "wrkspc_test".to_string(),
    }
}

async fn federation(expires_in: u64) -> (Arc<MockTokenEndpoint>, Arc<Federation>) {
    let (endpoint, url) = MockTokenEndpoint::start(expires_in).await;
    let federation = Arc::new(Federation::new(
        format!("{url}/v1/oauth/token"),
        ids(),
        Arc::new(MockAssertions::default()),
    ));
    (endpoint, federation)
}

/// Waits (bounded) for the endpoint to have seen `n` exchanges.
async fn until_exchanges(endpoint: &MockTokenEndpoint, n: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while endpoint.exchanges() < n {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {n} exchanges, saw {}", endpoint.exchanges()));
}

#[tokio::test]
async fn exchange_body_is_the_jwt_bearer_grant_with_the_four_ids_and_a_fresh_assertion() {
    let (endpoint, federation) = federation(0).await;
    // expires_in 0: every call is a mandatory refresh, so each one exchanges.
    assert_eq!(federation.token().await.unwrap(), "tok-1");
    assert_eq!(federation.token().await.unwrap(), "tok-2");

    let bodies = endpoint.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    let mut jtis = HashSet::new();
    for body in &bodies {
        assert_eq!(
            body["grant_type"],
            "urn:ietf:params:oauth:grant-type:jwt-bearer"
        );
        assert_eq!(body["federation_rule_id"], "fdrl_test");
        assert_eq!(
            body["organization_id"],
            "00000000-0000-0000-0000-000000000001"
        );
        assert_eq!(body["service_account_id"], "svac_test");
        assert_eq!(body["workspace_id"], "wrkspc_test");
        let assertion = token::token::Token::parse(body["assertion"].as_str().unwrap()).unwrap();
        assert_eq!(assertion.audience(), Some("https://api.anthropic.com"));
        jtis.insert(assertion.jti().unwrap().to_string());
    }
    assert_eq!(jtis.len(), 2, "each exchange presents a distinct jti");
}

#[tokio::test]
async fn fifty_concurrent_callers_on_a_cold_cache_cause_one_exchange() {
    let (endpoint, federation) = federation(3600).await;
    let calls: Vec<_> = (0..50)
        .map(|_| {
            let f = federation.clone();
            tokio::spawn(async move { f.token().await })
        })
        .collect();
    for call in calls {
        assert_eq!(call.await.unwrap().unwrap(), "tok-1");
    }
    assert_eq!(endpoint.exchanges(), 1);
}

#[tokio::test]
async fn a_token_inside_the_advisory_window_is_served_while_a_background_refresh_runs() {
    // T-4 min: inside the 5 min advisory window, outside the 60 s mandatory one.
    let (endpoint, federation) = federation(240).await;
    assert_eq!(federation.token().await.unwrap(), "tok-1");

    assert_eq!(
        federation.token().await.unwrap(),
        "tok-1",
        "the caller does not wait"
    );
    until_exchanges(&endpoint, 2).await;
}

#[tokio::test]
async fn a_token_inside_the_mandatory_window_makes_callers_wait_for_the_new_one() {
    // T-30 s.
    let (endpoint, federation) = federation(30).await;
    assert_eq!(federation.token().await.unwrap(), "tok-1");

    assert_eq!(federation.token().await.unwrap(), "tok-2");
    assert_eq!(endpoint.exchanges(), 2);
}

#[tokio::test]
async fn a_failed_exchange_keeps_serving_a_still_valid_token() {
    let (endpoint, federation) = federation(30).await;
    assert_eq!(federation.token().await.unwrap(), "tok-1");

    endpoint.fail.store(true, Ordering::SeqCst);
    assert_eq!(federation.token().await.unwrap(), "tok-1");
}

#[tokio::test]
async fn an_expired_token_with_a_failing_exchange_is_a_descriptive_error() {
    let (endpoint, federation) = federation(0).await;
    assert_eq!(federation.token().await.unwrap(), "tok-1");

    endpoint.fail.store(true, Ordering::SeqCst);
    let err = federation.token().await.unwrap_err();
    assert!(
        err.contains("no unexpired token is cached") && err.contains("503"),
        "unexpected error: {err}"
    );
}

#[test]
fn the_env_ids_are_all_four_or_none_and_the_api_key_is_never_read() {
    let read = std::sync::Mutex::new(Vec::new());
    let env = |vars: &'static [(&'static str, &'static str)]| {
        let read = &read;
        move |name: &str| {
            read.lock().unwrap().push(name.to_string());
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    };

    let ids = FederationIds::from_env(env(&[
        ("ANTHROPIC_FEDERATION_RULE_ID", "fdrl_x"),
        ("ANTHROPIC_ORGANIZATION_ID", "org_x"),
        ("ANTHROPIC_SERVICE_ACCOUNT_ID", "svac_x"),
        ("ANTHROPIC_WORKSPACE_ID", "wrkspc_x"),
        ("ANTHROPIC_API_KEY", "sk-ant-must-not-be-read"),
    ]))
    .unwrap()
    .expect("all four set");
    assert_eq!(ids.federation_rule_id, "fdrl_x");
    assert_eq!(ids.organization_id, "org_x");
    assert_eq!(ids.service_account_id, "svac_x");
    assert_eq!(ids.workspace_id, "wrkspc_x");

    assert!(FederationIds::from_env(env(&[])).unwrap().is_none());
    let err = FederationIds::from_env(env(&[("ANTHROPIC_WORKSPACE_ID", "wrkspc_x")]))
        .err()
        .expect("a partial set is an error");
    assert!(err.contains("all four"), "{err}");

    assert!(!read
        .lock()
        .unwrap()
        .iter()
        .any(|name| name == "ANTHROPIC_API_KEY"));
}
