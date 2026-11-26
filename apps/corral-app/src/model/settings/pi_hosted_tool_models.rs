//! Each hosted tool route's model list, read from the host's provider login on selection.
//! A list only offers IDs: it never proves the account may use them for the tool, and the
//! typed ID stays the fallback whatever the list's state.
use corral_protocol::config::{
    PI_HOSTED_TOOL_MODELS_V1, PiHostedTool, PiHostedToolModels, PiHostedToolModelsRequest,
    PiHostedToolModelsStatus, PiHostedToolProvider,
};

use super::pi_hosted_tools::{Editor, Tool};
use crate::model::{
    Id,
    daemon::{Feed, Operation},
};

/// The request listing `route`'s models for `tool`, when the route names a provider.
pub fn request(tool: Tool, route: &str) -> Option<PiHostedToolModelsRequest> {
    let provider = match route {
        "openai" => PiHostedToolProvider::OpenAi,
        "openai-codex" => PiHostedToolProvider::OpenAiCodex,
        "anthropic" => PiHostedToolProvider::Anthropic,
        "anthropic-api" => PiHostedToolProvider::AnthropicApi,
        _ => return None,
    };
    let tool = match tool {
        Tool::Search => PiHostedTool::WebSearch,
        Tool::Image => PiHostedTool::ImageGen,
    };
    Some(PiHostedToolModelsRequest { tool, provider })
}

pub fn listable(feed: &Feed) -> bool {
    feed.capable(PI_HOSTED_TOOL_MODELS_V1)
}

#[derive(Clone, Debug, Default, PartialEq)]
enum Listing {
    #[default]
    Idle,
    /// The connection does not offer model listing.
    Unlisted,
    Unsent,
    Pending(Id),
    Answered(PiHostedToolModels),
    Failed,
}

/// One tool's list for the route it was asked for, and whether the owner types the ID.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Discovery {
    asked: Option<PiHostedToolModelsRequest>,
    listing: Listing,
    pub manual: bool,
}

/// What the model picker offers: an ID and whether the provider listed it.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub id: String,
    pub listed: bool,
}

impl Discovery {
    /// Follows the route shown; whether `wanted` needs a new list. A changed route drops the
    /// list and the request in flight, so its reply cannot land on another route.
    pub fn want(&mut self, wanted: Option<PiHostedToolModelsRequest>) -> bool {
        if self.asked == wanted {
            return false;
        }
        *self = Self {
            asked: wanted,
            ..Self::default()
        };
        wanted.is_some()
    }

    pub fn asked(&self) -> Option<PiHostedToolModelsRequest> {
        self.asked
    }

    pub fn pending(&self) -> Option<&Id> {
        match &self.listing {
            Listing::Pending(key) => Some(key),
            _ => None,
        }
    }

    /// Whether asking again can change anything: a list was asked of a connection that lists.
    pub fn refreshable(&self) -> bool {
        !matches!(self.listing, Listing::Idle | Listing::Unlisted)
    }

    /// Records a request for the route asked; `None` when the connection cannot list.
    pub fn sent(&mut self, sent: Option<(Id, bool)>) {
        self.listing = match sent {
            None => Listing::Unlisted,
            Some((key, true)) => Listing::Pending(key),
            Some((_, false)) => Listing::Unsent,
        };
    }

    /// Takes the reply to the request in flight; any other reply is late and ignored.
    pub fn settle(&mut self, key: &Id, reply: &Operation) {
        if self.pending() != Some(key) {
            return;
        }
        self.listing = match reply.decode::<PiHostedToolModels>() {
            Some(models) => Listing::Answered(models),
            None => Listing::Failed,
        };
    }

    fn listed(&self) -> Option<&[String]> {
        match &self.listing {
            Listing::Answered(PiHostedToolModels {
                status: PiHostedToolModelsStatus::Live,
                models,
            }) if !models.is_empty() => Some(models),
            _ => None,
        }
    }

    /// The picker's choices, or `None` while there is no list to pick from: the listed IDs,
    /// then `current` when the provider did not list it, so a saved or typed ID stays chosen.
    pub fn choices(&self, current: &str) -> Option<Vec<Choice>> {
        let listed = self.listed()?;
        let mut choices: Vec<Choice> = listed
            .iter()
            .map(|id| Choice {
                id: id.clone(),
                listed: true,
            })
            .collect();
        if !current.is_empty() && !listed.iter().any(|id| id == current) {
            choices.push(Choice {
                id: current.into(),
                listed: false,
            });
        }
        Some(choices)
    }

    /// Whether the typed ID field shows: chosen, or no usable list.
    pub fn typing(&self) -> bool {
        self.manual || self.listed().is_none()
    }

    /// What the list's state means for the owner, and whether it is a problem.
    pub fn note(&self, tool: Tool, route: &str) -> (&'static str, bool) {
        let answered = |status| match status {
            PiHostedToolModelsStatus::Unsupported => (
                "This provider does not list models. Enter a model ID.",
                false,
            ),
            PiHostedToolModelsStatus::MissingLogin => (
                "Not logged in to this provider on the host. Enter a model ID, or log in and refresh.",
                true,
            ),
            _ => ("Could not list models. Enter a model ID or refresh.", true),
        };
        match &self.listing {
            Listing::Idle => ("", false),
            Listing::Unlisted => (
                "This connection does not list provider models. Enter a model ID.",
                false,
            ),
            Listing::Unsent => (
                "Could not ask the daemon for models. Enter a model ID or refresh.",
                true,
            ),
            Listing::Pending(_) => ("Listing models… You can enter a model ID meanwhile.", false),
            Listing::Failed => ("Could not list models. Enter a model ID or refresh.", true),
            Listing::Answered(PiHostedToolModels {
                status: PiHostedToolModelsStatus::Live,
                models,
            }) if models.is_empty() => ("The provider listed no models. Enter a model ID.", false),
            Listing::Answered(PiHostedToolModels {
                status: PiHostedToolModelsStatus::Live,
                ..
            }) => (entitlement(tool, route), false),
            Listing::Answered(models) => answered(models.status),
        }
    }
}

impl Editor {
    /// The list `tool`'s shown route needs once the saved settings are read, when it was not
    /// already asked for this route.
    pub fn wanted(&mut self, tool: Tool) -> Option<PiHostedToolModelsRequest> {
        let index = tool as usize;
        let route = tool
            .routes()
            .get(self.draft[index].route)
            .map(|route| route.0);
        let wanted = route
            .filter(|_| self.saved.is_some())
            .and_then(|route| request(tool, route));
        self.models[index].want(wanted).then_some(wanted).flatten()
    }

    /// A model chosen from the picker.
    pub fn pick(&mut self, tool: Tool, id: String) {
        let index = tool as usize;
        self.draft[index].model = id;
        self.models[index].manual = false;
    }

    /// A typed model ID: the field stays shown even when a list arrives.
    pub fn type_model(&mut self, tool: Tool, id: String) {
        let index = tool as usize;
        self.draft[index].model = id;
        self.models[index].manual = true;
    }
}

/// The account's listed models are not a statement about the tool.
fn entitlement(tool: Tool, route: &str) -> &'static str {
    match (tool, route) {
        (Tool::Search, _) => "Models this login lists; web search support is not verified.",
        (Tool::Image, "openai-codex") => {
            "Chat models this subscription lists, to carry image requests; image access is not verified."
        }
        (Tool::Image, _) => "Models this API key lists; image generation is not verified.",
    }
}

#[cfg(test)]
#[path = "pi_hosted_tool_models_tests.rs"]
mod tests;
