//! The startup model is saved independently of the corral.toml draft.
use corral_protocol::config::{PI_STARTUP_V1, PiStartupChange, PiStartupConfig};

use crate::model::{
    Id,
    daemon::{Feed, Operation},
};

pub const NOTE: &str = "Used for fresh terminal chats and new Build sessions without a model override. Clear the field and save to use the built-in default.";

pub fn unavailable(feed: &Feed) -> Option<&'static str> {
    if !feed.ready() {
        Some("Connect to a supported daemon to edit this setting.")
    } else if !feed.capable(PI_STARTUP_V1) {
        Some("This daemon does not expose the Pi startup setting.")
    } else {
        None
    }
}

#[derive(Clone, PartialEq)]
struct Waiting {
    key: Id,
    saving: bool,
    draft_at_send: String,
}

#[derive(Clone, Default, PartialEq)]
pub struct Editor {
    pub saved: Option<PiStartupConfig>,
    pub draft: String,
    waiting: Option<Waiting>,
    problem: Option<String>,
}

impl Editor {
    pub fn pending(&self) -> Option<&Id> {
        self.waiting.as_ref().map(|waiting| &waiting.key)
    }

    pub fn change(&self) -> Option<PiStartupChange> {
        let saved = self.saved.as_ref()?;
        let draft = self.draft.trim();
        (self.waiting.is_none() && draft != saved.model.as_deref().unwrap_or_default()).then(|| {
            PiStartupChange {
                revision: saved.revision.clone(),
                model: (!draft.is_empty()).then(|| draft.to_owned()),
            }
        })
    }

    pub fn sent(&mut self, key: Id, saving: bool, sent: bool) {
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
        if let Some(value) = reply.decode::<PiStartupConfig>() {
            if self.draft == waiting.draft_at_send {
                self.draft = value.model.clone().unwrap_or_default();
            }
            self.saved = Some(value);
        } else {
            self.problem = Some(reply.error.map_or_else(
                || "The daemon returned no setting.".into(),
                |error| error.message,
            ));
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
        } else if let Some(saved) = &self.saved {
            (
                format!(
                    "Saved: {}",
                    saved.model.as_deref().unwrap_or("Built-in default")
                ),
                false,
            )
        } else {
            (String::new(), false)
        }
    }
}
