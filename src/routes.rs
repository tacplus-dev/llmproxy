//! The alias table and `latest` resolution (#13).
//!
//! `Config.models` maps each alias to a vendor and a model: pinned
//! (`provider_model`) or the vendor's newest model whose id matches
//! `latest_pattern`, picked from the vendor's own model listing. A listing is
//! fetched once per vendor per `Config.latest_cache_ttl_ms` and serves every
//! `latest` alias of that vendor.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;

use llmproxy_model::types::services::llmproxy::llmproxy::{ModelRoute, Vendor};

use crate::providers::{ListedModel, ProviderError};

/// One configured alias: its route, its vendor (unspecified read as
/// Anthropic), and its compiled `latest_pattern` when it has one.
pub struct Route {
    pub route: ModelRoute,
    pub vendor: Vendor,
    pub latest: Option<Regex>,
}

/// The alias table: `Config.models`, keyed by alias. The provider model and
/// its price come from config, so a model changes with a config push, never a
/// rebuild (this replaces the code table and LP4(b)'s in-code pricing).
#[derive(Default)]
pub struct ModelTable {
    routes: HashMap<String, Route>,
}

impl ModelTable {
    /// Refuses a route that names its model both ways or neither, or whose
    /// pattern does not compile: a config error, not a failed call later.
    pub fn new(routes: &[ModelRoute]) -> Result<Self, String> {
        let mut table = HashMap::new();
        for r in routes {
            let latest = match (r.provider_model.is_empty(), r.latest_pattern.is_empty()) {
                (false, true) => None,
                (true, false) => Some(
                    Regex::new(&format!("^(?:{})$", r.latest_pattern))
                        .map_err(|e| format!("model {}: latest_pattern does not compile: {e}", r.alias))?,
                ),
                _ => {
                    return Err(format!(
                        "model {}: set exactly one of provider_model and latest_pattern",
                        r.alias
                    ))
                }
            };
            let vendor = match Vendor::try_from(r.vendor) {
                Ok(Vendor::Unspecified) => Vendor::Anthropic,
                Ok(vendor) => vendor,
                Err(_) => return Err(format!("model {}: unknown vendor {}", r.alias, r.vendor)),
            };
            table.insert(r.alias.clone(), Route { route: r.clone(), vendor, latest });
        }
        Ok(Self { routes: table })
    }

    /// The route for an alias, or UnknownModelAlias — the gate every call
    /// passes first.
    pub fn route(&self, alias: &str) -> Result<&Route, ProviderError> {
        self.routes.get(alias).ok_or_else(|| ProviderError::UnknownModelAlias(alias.to_string()))
    }

    /// The aliases served, in a stable order.
    pub fn aliases(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.routes.keys().map(String::as_str).collect();
        names.sort();
        names
    }
}

/// The newest listed model whose whole id matches `pattern`: previews only
/// when allowed; the pattern's capture groups read as numbers, compared left
/// to right (an absent or non-numeric group is 0); a tie goes to the greater id.
pub fn pick_latest<'a>(models: &'a [ListedModel], pattern: &Regex, allow_preview: bool) -> Option<&'a str> {
    models
        .iter()
        .filter(|m| allow_preview || !m.preview)
        .filter_map(|m| {
            let caps = pattern.captures(&m.id)?;
            let version: Vec<u64> = caps
                .iter()
                .skip(1)
                .map(|g| g.and_then(|g| g.as_str().parse().ok()).unwrap_or(0))
                .collect();
            Some((version, m.id.as_str()))
        })
        .max()
        .map(|(_, id)| id)
}

/// Each vendor's model listing, reused for `ttl`. A failed fetch serves the
/// last listing (and says so in the log); with none, the error.
pub struct ListingCache {
    ttl: Duration,
    entries: Mutex<HashMap<i32, (Instant, Arc<Vec<ListedModel>>)>>,
}

impl ListingCache {
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, entries: Mutex::new(HashMap::new()) }
    }

    pub async fn get<F, Fut>(&self, vendor: Vendor, fetch: F) -> Result<Arc<Vec<ListedModel>>, ProviderError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<ListedModel>, ProviderError>>,
    {
        let cached = self.entries.lock().unwrap().get(&(vendor as i32)).cloned();
        if let Some((at, listing)) = &cached {
            if at.elapsed() < self.ttl {
                return Ok(listing.clone());
            }
        }
        match fetch().await {
            Ok(listing) => {
                let listing = Arc::new(listing);
                self.entries
                    .lock()
                    .unwrap()
                    .insert(vendor as i32, (Instant::now(), listing.clone()));
                Ok(listing)
            }
            Err(e) => match cached {
                Some((_, listing)) => {
                    tracing::warn!(vendor = vendor.as_str_name(), error = %e, "model listing failed; resolving latest from the last one");
                    Ok(listing)
                }
                None => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(ids: &[&str]) -> Vec<ListedModel> {
        ids.iter()
            .map(|id| ListedModel { id: id.to_string(), preview: crate::providers::looks_preview(id) })
            .collect()
    }

    fn pattern(p: &str) -> Regex {
        Regex::new(&format!("^(?:{p})$")).unwrap()
    }

    #[test]
    fn openais_flagship_is_the_highest_version_without_snapshots_or_minis() {
        let models = listed(&[
            "gpt-4o", "gpt-5", "gpt-5-mini", "gpt-5-2025-08-07", "gpt-5.1", "gpt-5.1-codex",
            "gpt-4.1", "gpt-5.2-preview", "o3",
        ]);
        let p = pattern(r"gpt-(\d+)(?:\.(\d+))?");
        assert_eq!(pick_latest(&models, &p, false), Some("gpt-5.1"));
        // Matches the pattern, but a preview only counts when allowed.
        let p = pattern(r"gpt-(\d+)(?:\.(\d+))?(?:-preview)?");
        assert_eq!(pick_latest(&models, &p, false), Some("gpt-5.1"));
        assert_eq!(pick_latest(&models, &p, true), Some("gpt-5.2-preview"));
    }

    #[test]
    fn anthropics_dated_ids_compare_by_the_captured_version_then_the_date() {
        let models = listed(&[
            "claude-opus-4-20250514",
            "claude-opus-4-1-20250805",
            "claude-opus-4-5-20251101",
            "claude-sonnet-4-5-20250929",
            "claude-opus-4-5-20251201",
        ]);
        let p = pattern(r"claude-opus-(\d+)(?:-(\d))?-\d{8}");
        // 4-5 beats 4-1 and 4 (whose date must not read as a minor), and of
        // the two 4-5s the later date is the greater id.
        assert_eq!(pick_latest(&models, &p, false), Some("claude-opus-4-5-20251201"));
    }

    #[test]
    fn geminis_pro_line_skips_flash_and_previews() {
        let models = listed(&[
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-3-pro-preview",
            "gemini-2.0-pro-exp",
            "gemini-1.5-pro",
        ]);
        let p = pattern(r"gemini-(\d+)(?:\.(\d+))?-pro(?:-preview)?");
        assert_eq!(pick_latest(&models, &p, false), Some("gemini-2.5-pro"));
        assert_eq!(pick_latest(&models, &p, true), Some("gemini-3-pro-preview"));
    }

    #[test]
    fn no_match_is_none_and_a_pattern_without_groups_takes_the_greatest_id() {
        let models = listed(&["a-1", "a-2"]);
        assert_eq!(pick_latest(&models, &pattern("b-.*"), false), None);
        assert_eq!(pick_latest(&models, &pattern("a-.*"), false), Some("a-2"));
        // The whole id must match.
        assert_eq!(pick_latest(&models, &pattern("a"), false), None);
    }

    #[test]
    fn a_route_names_its_model_exactly_one_way_and_its_pattern_must_compile() {
        let route = |provider_model: &str, latest_pattern: &str| ModelRoute {
            alias: "x".to_string(),
            provider_model: provider_model.to_string(),
            latest_pattern: latest_pattern.to_string(),
            ..Default::default()
        };
        assert!(ModelTable::new(&[route("m", "")]).is_ok());
        assert!(ModelTable::new(&[route("", "m-.*")]).is_ok());
        assert!(ModelTable::new(&[route("m", "m-.*")]).err().unwrap().contains("exactly one"));
        assert!(ModelTable::new(&[route("", "")]).err().unwrap().contains("exactly one"));
        assert!(ModelTable::new(&[route("", "(")]).err().unwrap().contains("does not compile"));
    }

    #[test]
    fn an_unknown_alias_is_refused_and_an_unspecified_vendor_is_anthropic() {
        let table = ModelTable::new(&[ModelRoute {
            alias: "claude-sonnet".to_string(),
            provider_model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        }])
        .unwrap();
        assert!(matches!(table.route("gpt-5"), Err(ProviderError::UnknownModelAlias(a)) if a == "gpt-5"));
        assert_eq!(table.route("claude-sonnet").unwrap().vendor, Vendor::Anthropic);
        assert!(matches!(ModelTable::default().route("claude-sonnet"), Err(ProviderError::UnknownModelAlias(_))));
    }

    #[test]
    fn aliases_are_in_order() {
        let table = ModelTable::new(&[
            ModelRoute { alias: "b".to_string(), provider_model: "m2".to_string(), ..Default::default() },
            ModelRoute { alias: "a".to_string(), provider_model: "m1".to_string(), ..Default::default() },
        ])
        .unwrap();
        assert_eq!(table.aliases(), vec!["a", "b"]);
        assert!(ModelTable::default().aliases().is_empty());
    }

    #[tokio::test]
    async fn a_listing_is_reused_within_the_ttl_and_a_failed_refetch_serves_the_last_one() {
        let cache = ListingCache::new(Duration::from_secs(60));
        let first = cache.get(Vendor::Openai, || async { Ok(listed(&["gpt-5"])) }).await.unwrap();
        assert_eq!(first[0].id, "gpt-5");
        let again = cache
            .get(Vendor::Openai, || async { panic!("fetched inside the ttl") })
            .await
            .unwrap();
        assert_eq!(again[0].id, "gpt-5");

        let expired = ListingCache::new(Duration::ZERO);
        expired.get(Vendor::Openai, || async { Ok(listed(&["gpt-5"])) }).await.unwrap();
        let refetched = expired.get(Vendor::Openai, || async { Ok(listed(&["gpt-5.1"])) }).await.unwrap();
        assert_eq!(refetched[0].id, "gpt-5.1");
        let stale = expired
            .get(Vendor::Openai, || async { Err(ProviderError::Credential("down".to_string())) })
            .await
            .unwrap();
        assert_eq!(stale[0].id, "gpt-5.1");
        let none = expired
            .get(Vendor::Google, || async { Err(ProviderError::Credential("down".to_string())) })
            .await;
        assert!(none.is_err());
    }
}
