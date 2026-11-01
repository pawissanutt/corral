use serde::{Deserialize, Serialize};

/// Search transport selection. Absence preserves the existing SearXNG behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProvider {
    Searxng,
    Subscription,
}

/// Resolved web policy, shared by snapshots and the host launch payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebSettings {
    pub hosts: Vec<String>,
    pub max_bytes: u64,
    pub timeout_ms: u32,
    pub max_redirects: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searxng_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_provider: Option<WebSearchProvider>,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            hosts: vec!["*".into()],
            max_bytes: 1_048_576,
            timeout_ms: 15_000,
            max_redirects: 5,
            searxng_url: None,
            search_provider: None,
        }
    }
}
