//! Workspace Pi settings for the explicit local fixture, independent of `corral.toml`.
use corral_protocol::config::{PI_HOSTED_TOOLS, PiHostedToolsConfig, PiHostedToolsRequest};

use crate::model::{
    Id,
    daemon::{ClientError, Feed, Operation},
};

fn preferences(web_search: Option<String>, image_gen: Option<String>) -> PiHostedToolsConfig {
    let revision = super::sample::sha(&format!("{web_search:?}/{image_gen:?}"), 16);
    PiHostedToolsConfig {
        revision,
        web_search,
        image_gen,
    }
}

fn valid(selector: Option<&str>, providers: &[&str]) -> bool {
    selector.is_none_or(|value| {
        value == "off"
            || (value.len() <= 256
                && value.split_once('/').is_some_and(|(provider, id)| {
                    providers.contains(&provider)
                        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                        && id
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
                        && !["sk-", "sess-", "eyj"].iter().any(|prefix| {
                            id.get(..prefix.len())
                                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
                        })
                }))
    })
}

pub fn pi_hosted_tools(feed: &mut Feed, key: Id, request: PiHostedToolsRequest) {
    let current = feed
        .sample_pi_hosted_tools
        .clone()
        .unwrap_or_else(|| preferences(None, None));
    let result = match request.change {
        None => Ok(current),
        Some(change) if change.revision != current.revision => {
            Err("pi.toml changed; reload the hosted tools settings before saving")
        }
        Some(change)
            if !valid(
                change.web_search.as_deref(),
                &["openai-codex", "openai", "anthropic", "anthropic-api"],
            ) =>
        {
            Err("pi.toml hosted_tools.web_search must be off or a search provider/model selector")
        }
        Some(change) if !valid(change.image_gen.as_deref(), &["openai", "openai-codex"]) => {
            Err("pi.toml hosted_tools.image_gen must be off or an image provider/model selector")
        }
        Some(change) => {
            let saved = preferences(change.web_search, change.image_gen);
            feed.sample_pi_hosted_tools = Some(saved.clone());
            Ok(saved)
        }
    };
    feed.operations.insert(
        key,
        Operation {
            method: PI_HOSTED_TOOLS.into(),
            loading: false,
            value: result
                .as_ref()
                .ok()
                .and_then(|config| serde_json::to_value(config).ok()),
            error: result.err().map(|message| ClientError {
                code: "invalid_params".into(),
                message: message.into(),
            }),
            refusal: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use corral_protocol::config::{PI_HOSTED_TOOLS_V1, PiHostedToolsChange};

    use super::*;

    #[test]
    fn pi_hosted_tools_fixture_saves_reopens_and_refuses_stale_revision() {
        let mut feed = super::super::feed(0, String::new(), String::new(), None);
        assert!(feed.ready());
        assert!(feed.capable(PI_HOSTED_TOOLS_V1));
        let mut request = |id: &str, change| {
            let key = Id::from(id);
            pi_hosted_tools(&mut feed, key.clone(), PiHostedToolsRequest { change });
            feed.operations[&key].clone()
        };
        let before = request("read", None)
            .decode::<PiHostedToolsConfig>()
            .unwrap();
        assert_eq!((before.web_search, before.image_gen), (None, None));
        let change = PiHostedToolsChange {
            revision: before.revision,
            web_search: Some("openai-codex/codex-fixture".into()),
            image_gen: Some("openai/gpt-image-1".into()),
        };
        let saved = request("save", Some(change.clone()))
            .decode::<PiHostedToolsConfig>()
            .unwrap();
        assert_ne!(saved.revision, change.revision);
        assert_eq!(saved.web_search, change.web_search);
        assert_eq!(saved.image_gen, change.image_gen);
        let stale = request(
            "stale",
            Some(PiHostedToolsChange {
                web_search: Some("off".into()),
                ..change
            }),
        );
        assert!(stale.error.is_some());
        assert_eq!(
            request("reopen", None).decode::<PiHostedToolsConfig>(),
            Some(saved)
        );
    }
}
