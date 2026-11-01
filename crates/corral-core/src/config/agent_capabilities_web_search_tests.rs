use super::*;

#[test]
fn subscription_search_selection_round_trips_without_changing_fetch_policy() {
    use crate::agent::WebSearchProvider;
    assert!(
        toml::from_str::<AgentCapabilities>("[worker.web]\nsearch_provider = 'openai-api'")
            .is_err()
    );
    let config: AgentCapabilities =
        toml::from_str("[worker.web]\nsearch_provider = 'subscription'\nhosts = []").unwrap();
    assert_eq!(
        toml::from_str::<AgentCapabilities>(&toml::to_string(&config).unwrap()).unwrap(),
        config
    );
    let web = config.resolve(AgentRole::Worker, None).web.unwrap();
    assert_eq!(web.search_provider, Some(WebSearchProvider::Subscription));
    assert!(web.hosts.is_empty());
    let default = AgentCapabilities::default()
        .resolve(AgentRole::Worker, None)
        .web
        .unwrap();
    assert_eq!(default.search_provider, None);
    assert!(
        serde_json::to_value(default)
            .unwrap()
            .get("search_provider")
            .is_none()
    );
}
