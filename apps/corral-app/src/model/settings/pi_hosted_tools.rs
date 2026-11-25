//! The hosted tools selection is saved independently of the corral.toml draft.
use corral_protocol::config::{PI_HOSTED_TOOLS_V1, PiHostedToolsChange, PiHostedToolsConfig};

use crate::model::{
    Id,
    daemon::{Feed, Operation},
};

pub const NOTE: &str = "Takes effect when a host starts or resumes.";

pub fn unavailable(feed: &Feed) -> Option<&'static str> {
    if !feed.ready() {
        Some("Connect to a supported daemon to edit this setting.")
    } else if !feed.capable(PI_HOSTED_TOOLS_V1) {
        Some("This daemon does not expose the Pi hosted tools settings.")
    } else {
        None
    }
}

#[derive(Clone, PartialEq)]
struct Waiting {
    key: Id,
    saving: bool,
    draft_at_send: [Selector; 2],
}

#[derive(Clone, Default, PartialEq)]
pub struct Editor {
    pub saved: Option<PiHostedToolsConfig>,
    pub draft: [Selector; 2],
    waiting: Option<Waiting>,
    problem: Option<String>,
    just_saved: bool,
}

impl Editor {
    pub fn pending(&self) -> Option<&Id> {
        self.waiting.as_ref().map(|waiting| &waiting.key)
    }

    pub fn change(&self) -> Option<PiHostedToolsChange> {
        let saved = self.saved.as_ref()?;
        let web_search = self.draft[0].value(Tool::Search).ok()?;
        let image_gen = self.draft[1].value(Tool::Image).ok()?;
        (self.waiting.is_none() && (web_search != saved.web_search || image_gen != saved.image_gen))
            .then(|| PiHostedToolsChange {
                revision: saved.revision.clone(),
                web_search,
                image_gen,
            })
    }

    pub fn sent(&mut self, key: Id, saving: bool, sent: bool) {
        self.just_saved = false;
        self.waiting = sent.then(|| Waiting {
            key,
            saving,
            draft_at_send: self.draft.clone(),
        });
        self.problem = (!sent).then(|| "Could not send the request to the daemon.".into());
    }

    pub fn settle(&mut self, reply: Operation) {
        let Some(waiting) = self.waiting.take() else {
            return;
        };
        if let Some(value) = reply.decode::<PiHostedToolsConfig>() {
            for (tool, selector) in [
                (Tool::Search, value.web_search.as_deref()),
                (Tool::Image, value.image_gen.as_deref()),
            ] {
                let index = tool as usize;
                if self.draft[index] == waiting.draft_at_send[index] {
                    self.draft[index] = Selector::read(tool, selector);
                }
            }
            self.saved = Some(value);
            self.just_saved = waiting.saving;
        } else {
            let message = match reply.error.as_ref().map(|error| error.message.as_str()) {
                Some("pi.toml changed; reload the hosted tools settings before saving") => {
                    "pi.toml changed. Reload before saving."
                }
                Some(
                    "pi.toml contains invalid TOML"
                    | "pi.toml hosted_tools must be a table"
                    | "pi.toml hosted_tools contains an unknown field"
                    | "pi.toml hosted_tools.web_search must be off or a search provider/model selector"
                    | "pi.toml hosted_tools.image_gen must be off or an image provider/model selector",
                ) => "Invalid hosted tools configuration. Check pi.toml on the host and reload.",
                _ if waiting.saving => "Could not save hosted tools. Reload and try again.",
                _ => "Could not read hosted tools. Check the daemon connection and reload.",
            };
            self.problem = Some(message.into());
        }
    }

    pub fn status(&self) -> (String, bool) {
        if let Some(waiting) = &self.waiting {
            (
                (if waiting.saving {
                    "Saving…"
                } else {
                    "Reading…"
                })
                .into(),
                false,
            )
        } else if let Some(problem) = &self.problem {
            (problem.clone(), true)
        } else if self.just_saved {
            ("Hosted tools saved.".into(), false)
        } else if self.saved.is_some() {
            ("Loaded from workspace preferences.".into(), false)
        } else {
            (String::new(), false)
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tool {
    Search,
    Image,
}

const OFF: Route = Route("off", "Off");
const API: Route = Route("openai", "OpenAI API (metered key)");

impl Tool {
    pub fn routes(self) -> &'static [Route] {
        match self {
            Self::Search => &[
                Route("", "Existing web setting (clear override)"),
                OFF,
                Route("openai-codex", "OpenAI subscription (recommended)"),
                API,
                Route("anthropic", "Claude subscription"),
                Route("anthropic-api", "Claude API (metered key)"),
            ],
            Self::Image => &[
                Route("", "Default: disabled (clear override)"),
                OFF,
                API,
                Route("openai-codex", "OpenAI subscription (experimental)"),
            ],
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Search => "web-search",
            Self::Image => "image-gen",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Search => "Web search",
            Self::Image => "Image generation",
        }
    }

    pub fn model_label(self, route: Route) -> &'static str {
        match (self, route.0) {
            (Self::Search, _) => "Search model",
            (Self::Image, "openai-codex") => "Carrier model",
            (Self::Image, _) => "Image model",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Route(pub &'static str, pub &'static str);

impl Route {
    pub fn needs_model(self) -> bool {
        !matches!(self.0, "" | "off")
    }
    pub fn guidance(self, tool: Tool) -> String {
        let carrier = if tool == Tool::Image && self.0 == "openai-codex" {
            " — the backend picks the image model; the carrier is a chat model."
        } else {
            ""
        };
        format!(
            "On the host: corral-pi login --provider {}{carrier}",
            self.0
        )
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct Selector {
    pub route: usize,
    pub model: String,
}

impl Selector {
    fn read(tool: Tool, selector: Option<&str>) -> Self {
        let selector = selector.unwrap_or_default();
        let (route, model) = selector.split_once('/').unwrap_or((selector, ""));
        Self {
            route: tool
                .routes()
                .iter()
                .position(|r| r.0 == route)
                .unwrap_or_default(),
            model: model.into(),
        }
    }

    pub fn value(&self, tool: Tool) -> Result<Option<String>, &'static str> {
        let error = match tool {
            Tool::Search => "Enter a valid search model ID, not a URL or credential.",
            Tool::Image => "Enter a valid image or carrier model ID, not a URL or credential.",
        };
        let route = tool.routes().get(self.route).ok_or(error)?;
        if !route.needs_model() {
            return Ok((route.0 == "off").then(|| "off".into()));
        }
        // Keep in sync with the engine's selector grammar; syntax does not prove entitlement.
        let id = self.model.as_str();
        if route.0.len() + 1 + id.len() > 256
            || !id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            || ["sk-", "sess-", "eyj"]
                .iter()
                .any(|p| id.get(..p.len()).is_some_and(|s| s.eq_ignore_ascii_case(p)))
        {
            return Err(error);
        }
        Ok(Some(format!("{}/{id}", route.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::ClientError;
    use serde_json::json;

    #[test]
    fn selector_grammar_matches_engine_for_search_and_images() {
        for tool in [Tool::Search, Tool::Image] {
            let mut draft = Selector::read(tool, Some("openai/a_B-1.2"));
            assert_eq!(draft.value(tool), Ok(Some("openai/a_B-1.2".into())));
            for invalid in [
                "",
                " a",
                "a ",
                "a/b",
                "https://example",
                "é",
                "a\0",
                "-a",
                "SK-abc",
                "Sess-abc",
                "eYJabc",
            ] {
                draft.model = invalid.into();
                assert!(draft.value(tool).is_err());
            }
            draft.model = "x".repeat(249);
            assert!(draft.value(tool).is_ok());
            draft.model.push('x');
            assert!(draft.value(tool).is_err());
        }
        assert!(
            Tool::Image
                .routes()
                .iter()
                .all(|route| !route.0.starts_with("anthropic"))
        );
    }

    #[test]
    fn revisioned_save_distinguishes_clear_and_off_and_stale_keeps_draft() {
        let reply = |value, error| Operation {
            method: "config.pi_hosted_tools".into(),
            loading: false,
            value,
            error,
            refusal: None,
        };
        let mut editor = Editor::default();
        editor.sent(Id::from("read".to_owned()), false, true);
        editor.settle(reply(
            Some(json!({"revision":"r1", "web_search":"openai/m", "image_gen":null})),
            None,
        ));
        assert!(editor.change().is_none());
        editor.draft = [
            Selector::default(),
            Selector::read(Tool::Image, Some("off")),
        ];
        let change = editor.change().unwrap();
        assert_eq!(change.revision, "r1");
        assert_eq!(change.web_search, None);
        assert_eq!(change.image_gen.as_deref(), Some("off"));
        editor.sent(Id::from("save".to_owned()), true, true);
        assert!(editor.change().is_none());
        editor.settle(reply(
            None,
            Some(ClientError {
                code: "invalid_params".into(),
                message: "pi.toml changed; reload the hosted tools settings before saving".into(),
            }),
        ));
        assert!(editor.status().1);
        assert_eq!(editor.change(), Some(change));
    }
}
