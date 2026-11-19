//! Device-scoped owner text and recovery identities; never keys, tokens or proofs.
use super::durable;
use corral_protocol::assistant::{AssistantSend, MESSAGE_TEXT_MAX};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_MESSAGES: usize = 128;
const MAX_DRAFTS: usize = 32;

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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Saved {
    pub drafts: BTreeMap<String, String>,
    pub messages: BTreeMap<String, Message>,
    #[serde(default)]
    pub cancelled: std::collections::BTreeSet<String>,
}
pub(super) struct Outbox {
    key: String,
    pub revision: u64,
    pub saved: Saved,
}
impl Outbox {
    pub async fn load_async(store: &str, device: &str) -> Result<Self, String> {
        let key = namespace_key(store, device);
        let legacy_key = format!(
            "corral.outbox.v1.{}",
            key.trim_start_matches("namespace/").trim_end_matches('/')
        );
        let legacy = nsh_core::storage::Storage::get_item(&legacy_key);
        let (saved, revision) = durable::transaction(|records| {
            if records.contains_key(&format!("forgotten/{key}")) {
                return Err("This local identity was forgotten. Pair or enrol again before saving owner words.".into());
            }
            if records.contains_key(&format!("{key}meta")) {
                return decode_records(records, &key);
            }
            let migrated = format!("migrated/{key}");
            let saved = if records.contains_key(&migrated) { Saved::default() }
                else if let Some(text) = &legacy {
                    if text.len() > MAX_BYTES { return Err("Legacy outbox exceeds 1 MiB; owner words were not discarded.".into()); }
                    serde_json::from_str(text).map_err(|error| format!("Cannot migrate saved owner words: {error}"))?
                } else { Saved::default() };
            validate(&saved)?;
            records.extend(encode_records(&saved, &key)?);
            records.insert(migrated, Value::Bool(true));
            Ok(saved)
        }).await?;
        nsh_core::storage::Storage::remove_item(&legacy_key);
        let outbox = Self {
            key,
            saved,
            revision,
        };
        outbox.validate()?;
        Ok(outbox)
    }
    fn validate(&self) -> Result<(), String> {
        validate(&self.saved)
    }
    pub async fn refresh(&mut self) -> Result<bool, String> {
        let (revision, records) = durable::read().await?;
        if revision == self.revision {
            return Ok(false);
        }
        let saved = decode_records(&records, &self.key)?;
        validate(&saved)?;
        self.saved = saved;
        self.revision = revision;
        Ok(true)
    }
    pub async fn update_async(&mut self, mut change: impl FnMut(&mut Saved)) -> Result<(), String> {
        self.update_checked(|saved| {
            change(saved);
            Ok(())
        })
        .await
    }
    pub async fn update_checked(
        &mut self,
        mut change: impl FnMut(&mut Saved) -> Result<(), String>,
    ) -> Result<(), String> {
        let key = &self.key;
        let (saved, revision) = durable::transaction(|records| {
            if records.contains_key(&format!("forgotten/{key}")) {
                return Err("This identity was forgotten by another app instance. Nothing was sent or saved.".into());
            }
            let mut saved = decode_records(records, key)?;
            change(&mut saved)?;
            validate(&saved)?;
            let revision = records.get(&format!("{key}meta")).and_then(|meta| meta["revision"].as_u64()).unwrap_or(0);
            let mut encoded = encode_records(&saved, key)?;
            encoded.insert(format!("{key}meta"), serde_json::json!({
                "revision": revision.checked_add(1).ok_or("Namespace revision overflow")?,
                "bytes": serde_json::to_vec(&saved).map_err(|e|e.to_string())?.len()
            }));
            records.retain(|record, _| !record.starts_with(key));
            records.extend(encoded);
            prune_settled(records, key)?;
            Ok(saved)
        }).await?;
        self.saved = saved;
        self.revision = revision;
        Ok(())
    }
    #[cfg(test)]
    pub fn load(store: &str, device: &str) -> Result<Self, String> {
        durable::wait(Self::load_async(store, device))
    }
    #[cfg(test)]
    pub fn update(&mut self, change: impl FnMut(&mut Saved)) -> Result<(), String> {
        durable::wait(self.update_async(change))
    }
}

pub(super) fn namespace_key(store: &str, device: &str) -> String {
    use std::fmt::Write;
    let mut key = String::from("namespace/");
    for byte in store.bytes().chain([0]).chain(device.bytes()) {
        let _ = write!(key, "{byte:02x}");
    }
    key.push('/');
    key
}

fn decode_records(records: &durable::Records, key: &str) -> Result<Saved, String> {
    let read = |kind: &str| -> Result<BTreeMap<String, Value>, String> {
        let prefix = format!("{key}{kind}/");
        Ok(records
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix))
            .map(|(key, value)| (key[prefix.len()..].to_owned(), value.clone()))
            .collect())
    };
    Ok(Saved {
        drafts: serde_json::from_value(
            serde_json::to_value(read("draft")?).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?,
        messages: serde_json::from_value(
            serde_json::to_value(read("message")?).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?,
        cancelled: read("cancelled")?.into_keys().collect(),
    })
}

fn encode_records(saved: &Saved, key: &str) -> Result<durable::Records, String> {
    let mut records = BTreeMap::new();
    for (id, draft) in &saved.drafts {
        records.insert(format!("{key}draft/{id}"), Value::String(draft.clone()));
    }
    for (id, message) in &saved.messages {
        records.insert(
            format!("{key}message/{id}"),
            serde_json::to_value(message).map_err(|e| e.to_string())?,
        );
    }
    for id in &saved.cancelled {
        records.insert(format!("{key}cancelled/{id}"), Value::Bool(true));
    }
    let bytes = serde_json::to_vec(saved)
        .map_err(|error| error.to_string())?
        .len();
    records.insert(format!("{key}meta"), serde_json::json!({"bytes":bytes}));
    Ok(records)
}

fn validate(saved: &Saved) -> Result<(), String> {
    if saved.messages.len() > MAX_MESSAGES
        || saved.drafts.len() > MAX_DRAFTS
        || saved
            .drafts
            .values()
            .any(|text| text.len() > MESSAGE_TEXT_MAX)
        || saved
            .messages
            .values()
            .any(|message| !valid_message(&message.request))
    {
        return Err("This device's offline outbox is full.".into());
    }
    if serde_json::to_vec(saved)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_BYTES
    {
        return Err("This device's offline outbox is full (1 MiB).".into());
    }
    Ok(())
}

fn prune_settled(records: &mut durable::Records, active: &str) -> Result<(), String> {
    // Records of the retired command journal are obsolete in every namespace; owner words stay.
    records.retain(|key, _| !key.contains("/command/"));
    let empty: Vec<_> = records
        .keys()
        .filter(|key| key.ends_with("/meta") && !key.starts_with(active))
        .filter(|key| {
            let prefix = key.trim_end_matches("meta");
            !records
                .keys()
                .any(|other| other != *key && other.starts_with(prefix))
        })
        .cloned()
        .collect();
    for key in empty {
        records.remove(&key);
    }
    let metadata: Vec<_> = records
        .keys()
        .filter(|key| {
            key.starts_with("namespace/") && key.ends_with("/meta") && !key.starts_with(active)
        })
        .cloned()
        .collect();
    for key in metadata {
        let saved = decode_records(records, key.trim_end_matches("meta"))?;
        let bytes = serde_json::to_vec(&saved)
            .map_err(|error| error.to_string())?
            .len();
        if let Some(meta) = records.get_mut(&key)
            && meta["bytes"].as_u64() != Some(bytes as u64)
        {
            let revision = meta["revision"]
                .as_u64()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("Namespace revision overflow")?;
            *meta = serde_json::json!({"bytes":bytes,"revision":revision});
        }
    }
    Ok(())
}

pub(super) fn valid_message(request: &AssistantSend) -> bool {
    use corral_protocol::{assistant::SendAttachment, attachment::*};
    !request.text.trim().is_empty()
        && request.text.len() <= MESSAGE_TEXT_MAX
        && request.attachments.len() <= ATTACHMENTS_PER_MESSAGE
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

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod transaction_tests;
