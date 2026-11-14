//! Device-scoped owner text and recovery identities; never keys, tokens or proofs.
use corral_protocol::assistant::{AssistantSend, MESSAGE_TEXT_MAX};
use nsh_core::storage::Storage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_MESSAGES: usize = 128;
const MAX_DRAFTS: usize = 32;
const MAX_COMMANDS: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Message {
    pub request: AssistantSend,
    pub attempted: bool,
    #[serde(default)]
    pub cancel: bool,
    /// Set only for a definitive RPC refusal, never a timeout or transport failure.
    #[serde(default)]
    pub refused: bool,
    #[serde(default)]
    pub error: Option<(String, String)>,
}

impl Message {
    pub(super) fn can_remove_locally(&self) -> bool {
        !self.attempted || self.refused
    }
    pub(super) fn replay_request(&self) -> Option<super::requests::ClientRequest> {
        use super::requests::ClientRequest;
        if self.cancel {
            Some(ClientRequest::AssistantCancel(self.request.cmd_id.clone()))
        } else if self.refused {
            None
        } else {
            Some(ClientRequest::AssistantSend(self.request.clone()))
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Command {
    pub method: String,
    pub params: Value,
    #[serde(default)]
    pub result: Option<corral_protocol::cmd::CmdResult>,
    #[serde(default)]
    pub error: Option<(String, String)>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Saved {
    pub drafts: BTreeMap<String, String>,
    pub messages: BTreeMap<String, Message>,
    pub commands: BTreeMap<String, Command>,
}
pub(super) struct Outbox {
    key: String,
    pub saved: Saved,
}
impl Outbox {
    pub fn load(store: &str, device: &str) -> Result<Self, String> {
        let mut key = String::from("corral.outbox.v1.");
        for byte in store.bytes().chain([0]).chain(device.bytes()) {
            use std::fmt::Write;
            let _ = write!(key, "{byte:02x}");
        }
        let saved = match Storage::get_item(&key) {
            None => Saved::default(),
            Some(text) if text.len() <= MAX_BYTES => serde_json::from_str(&text)
                .map_err(|error| format!("Cannot read this device's saved messages: {error}"))?,
            Some(_) => return Err("This device's saved messages exceed the storage bound.".into()),
        };
        let outbox = Self { key, saved };
        outbox.validate()?;
        Ok(outbox)
    }
    fn validate(&self) -> Result<(), String> {
        if self.saved.messages.len() > MAX_MESSAGES
            || self.saved.drafts.len() > MAX_DRAFTS
            || self.saved.commands.len() > MAX_COMMANDS
            || self
                .saved
                .drafts
                .values()
                .any(|text| text.len() > MESSAGE_TEXT_MAX)
            || self
                .saved
                .messages
                .values()
                .any(|message| !valid_message(&message.request))
        {
            return Err("This device's offline outbox is full.".into());
        }
        Ok(())
    }
    fn persist(&self) -> Result<(), String> {
        self.validate()?;
        let text = serde_json::to_string(&self.saved).map_err(|error| error.to_string())?;
        if text.len() > MAX_BYTES {
            return Err("This device's offline outbox is full (1 MiB).".into());
        }
        Storage::set_item(&self.key, &text)
            .map_err(|error| format!("Cannot save this device's outbox: {error}"))
    }
    pub fn update(&mut self, change: impl FnOnce(&mut Saved)) -> Result<(), String> {
        let previous = self.saved.clone();
        change(&mut self.saved);
        if let Err(error) = self.persist() {
            self.saved = previous;
            return Err(error);
        }
        Ok(())
    }
}

pub(super) fn valid_message(request: &AssistantSend) -> bool {
    use corral_protocol::{
        assistant::{AskTarget, SendAttachment},
        attachment::*,
    };
    !request.text.trim().is_empty()
        && request.text.len() <= MESSAGE_TEXT_MAX
        && request.attachments.len() <= ATTACHMENTS_PER_MESSAGE
        && (request.attachments.is_empty() || request.target != AskTarget::Pm)
        && request.attachments.iter().all(|item| match item {
            SendAttachment::Upload { upload, sha256 } => {
                !upload.is_empty()
                    && upload.len() <= 160
                    && sha256.len() == 64
                    && sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            }
            SendAttachment::Link { url, title } => {
                valid_link(url)
                    && title.as_ref().is_none_or(|title| {
                        title.len() <= ATTACHMENT_NAME_MAX && !title.chars().any(char::is_control)
                    })
            }
            SendAttachment::Other => false,
        })
}

fn valid_link(url: &str) -> bool {
    use corral_protocol::attachment::ATTACHMENT_URL_MAX;
    let authority = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    url.len() <= ATTACHMENT_URL_MAX
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
        && authority.is_some_and(|rest| {
            let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
            !host.is_empty() && !host.contains('@')
        })
}

#[cfg(test)]
mod tests {
    use super::super::requests::ClientRequest;
    use super::*;
    #[test]
    fn persisted_uncertain_cancel_selects_cancel_not_resend() {
        let request: AssistantSend = serde_json::from_value(serde_json::json!({
            "cmd_id": "same-id", "thread": "quick", "target": "assistant", "text": "owner words",
            "typed_at": "2026-11-14T00:00:00Z", "context": []
        }))
        .unwrap();
        let mut message = Message {
            request,
            attempted: true,
            cancel: true,
            refused: false,
            error: Some(("unavailable".into(), "lost reply".into())),
        };
        let recovered: Message =
            serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
        assert!(!recovered.can_remove_locally());
        assert!(
            matches!(recovered.replay_request(), Some(ClientRequest::AssistantCancel(id)) if id == "same-id")
        );
        message.cancel = false;
        assert!(matches!(
            message.replay_request(),
            Some(ClientRequest::AssistantSend(_))
        ));
        message.refused = true;
        assert!(message.can_remove_locally());
        assert!(message.replay_request().is_none());
        assert!(
            serde_json::to_value(&message.request)
                .unwrap()
                .get("attachments")
                .is_none()
        );
    }
}
