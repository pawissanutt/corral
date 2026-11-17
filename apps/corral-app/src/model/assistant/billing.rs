//! Billing labels use frozen session connections, never the current profile's configuration.
pub(super) fn connection_label(connection: &str) -> &str {
    match connection {
        "anthropic-api" => "Anthropic · API (metered)",
        "anthropic-subscription" => "Anthropic · subscription",
        "openai-api" => "OpenAI · API (metered)",
        "openai-subscription" => "OpenAI · subscription",
        "amazon-bedrock" => "Amazon Bedrock · metered",
        "llama.cpp" => "llama.cpp · local",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{State, live_thread_header};
    use crate::model::daemon::Feed;
    use corral_protocol::rows::AssistantRow;

    #[test]
    fn session_connection_is_not_reinterpreted_from_the_profile() {
        let mut feed = Feed::default();
        let row: AssistantRow = serde_json::from_value(serde_json::json!({
            "id":"A-1", "kind":"session", "title":"test", "state":"ready",
            "live_in":"daemon", "profile":"same-model-profile", "connection":"anthropic-api",
            "focus":null, "pending":0, "turn":1, "cost_usd":null, "cost_basis":null,
            "wait":null, "rev":1,
        }))
        .unwrap();
        feed.assistants.insert("A-1".into(), row);
        let connection = |feed: &Feed| {
            live_thread_header(&State::default(), feed, "A-1")
                .facts
                .into_iter()
                .find(|fact| fact.label == "Connection")
                .unwrap()
                .value
        };
        let paid = connection(&feed);
        feed.assistants.get_mut("A-1").unwrap().connection = Some("anthropic-subscription".into());
        let subscription = connection(&feed);
        assert_ne!(paid, subscription);
        assert_eq!(
            paid,
            vec![crate::model::text::span("Anthropic · API (metered)")]
        );
        feed.assistants.get_mut("A-1").unwrap().connection = None;
        assert_eq!(connection(&feed), vec![crate::model::text::span("unknown")]);
    }
}
