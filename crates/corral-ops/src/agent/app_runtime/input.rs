//! Daemon-only owner input journal. Device identity is supplied by the authenticated server,
//! never deserialized from host/model payloads. Provenance and input publish atomically before
//! delivery; late/future-dated input stays visible but cannot authorize a model owner action.
use anyhow::{Context, Result, ensure};
use corral_core::agent::paths::sha256_hex;
use corral_core::assistant::{
    AssistantContainer, NewOwnerInput, OWNER_INPUT_TEXT_MAX, OwnerInput, OwnerInputSource,
};
use corral_core::{Store, records::RecordArea};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::AuthenticatedDevice;

const INPUTS: RecordArea<Provenance> = RecordArea::new("app_owner_inputs");
const COMMANDS: RecordArea<Journaled> = RecordArea::new("app_owner_input_commands");
/// A message typed more than five minutes ago must be explicitly retyped to authorize actions.
pub const FRESH_SECONDS: i64 = 300;
const FUTURE_SKEW_SECONDS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub device: String,
    pub name: String,
    pub cmd_id: String,
    pub typed_at: Timestamp,
    pub received_at: Timestamp,
    pub assistant_act: bool,
    pub late: bool,
    pub tainted: bool,
    #[serde(default)]
    pub attachments: bool,
    pub sha256: String,
}

impl Provenance {
    pub fn confirm_class(&self) -> bool {
        self.attachments
    }
}

#[derive(Serialize, Deserialize)]
struct Journaled {
    entry: OwnerInput,
    typed_at: Timestamp,
}

pub fn provenance(store: &Store, input: &str) -> Result<Option<Provenance>> {
    INPUTS.load(store, input)
}

/// Complete daemon input retained for chat filing after the bounded citable journal evicts
/// it. This does not restore owner_action authority: citation still requires the live journal.
pub fn owner_input(store: &Store, session: &str, id: &str) -> Result<OwnerInput> {
    let assistant = store.get_assistant_container(session)?;
    if let Some(entry) = assistant
        .owner_inputs
        .into_iter()
        .find(|entry| entry.id == id)
    {
        return Ok(entry);
    }
    let origin = provenance(store, id)?.context("not_owner_input: unknown owner words")?;
    let key = sha256_hex(format!("{session}\0{}\0{}", origin.device, origin.cmd_id).as_bytes());
    let saved = COMMANDS
        .load(store, &key)?
        .context("not_owner_input: owner words are unavailable")?;
    ensure!(
        saved.entry.id == id,
        "not_owner_input: input belongs to another session"
    );
    Ok(saved.entry)
}

pub(crate) fn save_provenance(store: &Store, entry: &OwnerInput, value: &Provenance) -> Result<()> {
    INPUTS.save(store, &entry.id, value)
}

pub(crate) fn fresh(typed_at: Timestamp, now: Timestamp) -> (bool, bool) {
    let age = now.as_second().saturating_sub(typed_at.as_second());
    (age > FRESH_SECONDS, age < -FUTURE_SKEW_SECONDS)
}

pub(crate) fn require_device(device: &AuthenticatedDevice) -> Result<()> {
    ensure!(
        device.assistant,
        "authority_assistant: device cannot use the assistant"
    );
    ensure!(
        !device.id.is_empty(),
        "authenticated device identity is required"
    );
    Ok(())
}

/// Append once per session/device/cmd_id, before queuing anything to a remote model host.
/// Replays preserve the original typed time and authority even if device settings changed.
/// Generation zero is valid while a cold thread waits for its first host launch.
pub fn journal(
    store: &Store,
    session: &str,
    device: &AuthenticatedDevice,
    cmd_id: &str,
    text: &str,
    typed_at: &str,
    attachments: bool,
) -> Result<OwnerInput> {
    require_device(device)?;
    ensure!(
        !cmd_id.is_empty() && cmd_id.len() <= 200,
        "invalid message command id"
    );
    ensure!(
        !text.trim().is_empty() && text.len() <= OWNER_INPUT_TEXT_MAX,
        "owner input must be 1..{OWNER_INPUT_TEXT_MAX} bytes"
    );
    let typed_at: Timestamp = typed_at.parse().context("parsing owner input typed_at")?;
    let sha256 = sha256_hex(text.as_bytes());
    let key = sha256_hex(format!("{session}\0{}\0{cmd_id}", device.id).as_bytes());
    store.update_assistant_container(session, |assistant| {
        if let Some(saved) = COMMANDS.load(store, &key)? {
            ensure!(
                saved.entry.sha256 == sha256
                    && saved.typed_at == typed_at
                    && provenance(store, &saved.entry.id)?
                        .is_some_and(|p| p.attachments == attachments),
                "conflict: message command id was reused with different input"
            );
            return Ok(saved.entry);
        }
        let now = Timestamp::now();
        let (late, tainted) = fresh(typed_at, now);
        let entry = assistant.journal_owner_input(
            session,
            NewOwnerInput {
                generation: assistant.state.binding.generation,
                turn: None,
                source: OwnerInputSource::Pane,
                sha256: sha256.clone(),
                text: text.into(),
                truncated: false,
                command: None,
                answers: None,
                request: format!("app:{key}"),
            },
        );
        INPUTS.save(
            store,
            &entry.id,
            &Provenance {
                device: device.id.clone(),
                name: device.name.clone(),
                cmd_id: cmd_id.into(),
                typed_at,
                received_at: now,
                assistant_act: device.assistant_act,
                late,
                tainted,
                attachments,
                sha256: sha256.clone(),
            },
        )?;
        COMMANDS.save(
            store,
            &key,
            &Journaled {
                entry: entry.clone(),
                typed_at,
            },
        )?;
        Ok(entry)
    })
}

/// Recheck both recorded authority and the live device registry immediately before effects.
/// Pane inputs retain their pre-app behavior; a remote generation can cite only server inputs.
pub fn recheck(
    store: &Store,
    assistant: &AssistantContainer,
    input: &OwnerInput,
) -> Result<Option<Provenance>> {
    let remote = assistant
        .state
        .current_generation()
        .is_some_and(|g| g.daemon_hosted);
    let Some(origin) = provenance(store, &input.id)? else {
        ensure!(
            !remote,
            "not_owner_input: remote host input has no authenticated provenance"
        );
        return Ok(None);
    };
    let (late, tainted) = fresh(origin.typed_at, Timestamp::now());
    ensure!(
        origin.assistant_act
            && !origin.late
            && !origin.tainted
            && !late
            && !tainted
            && origin.sha256 == input.sha256
            && sha256_hex(input.text.as_bytes()) == origin.sha256,
        "not_owner_input: device input is stale, tainted or lacks assistant_act authority"
    );
    recheck_authority(store, &origin.device, true)?;
    Ok(Some(origin))
}

/// Permission prompts request explicit fresh owner consent, not implicit model authority.
/// A normal web device may open one without assistant_act; the subsequent grant still needs
/// an act-authorized, device-bound fresh UI proof for the exact operation and scope.
pub(crate) fn permission_origin(store: &Store, input: &OwnerInput) -> Result<Provenance> {
    ensure!(
        matches!(
            input.source,
            OwnerInputSource::Pane | OwnerInputSource::Dialog
        ) && !input.truncated,
        "not_owner_input: permission requires complete owner text"
    );
    let origin =
        provenance(store, &input.id)?.context("permission requires authenticated device input")?;
    let (late, tainted) = fresh(origin.typed_at, Timestamp::now());
    ensure!(
        !origin.late
            && !origin.tainted
            && !late
            && !tainted
            && origin.sha256 == input.sha256
            && sha256_hex(input.text.as_bytes()) == origin.sha256,
        "not_owner_input: permission input is stale or tainted"
    );
    recheck_authority(store, &origin.device, false)?;
    Ok(origin)
}

/// Reconstruct a principal from the enrolled registry, never from the message's display name
/// or model-controlled event fields. This also repairs append-before-journal crash windows.
pub fn device_for_message(
    store: &Store,
    message: &corral_core::assistant::threads::Message,
) -> Result<AuthenticatedDevice> {
    let device = device_by_id(store, &message.device)?;
    require_device(&device)?;
    Ok(device)
}

/// Canonical replay defense; the daemon still owns the shared dev/cmd action journal.
pub fn prompt_replay(
    store: &Store,
    device: &str,
    answer: &corral_protocol::assistant::PromptAnswer,
) -> Result<Option<serde_json::Value>> {
    crate::agent::ops::owner_prompt::replay(store, device, answer)
}

pub fn prompt_result(
    store: &Store,
    device: &str,
    cmd_id: &str,
) -> Result<Option<serde_json::Value>> {
    crate::agent::ops::owner_prompt::result(store, device, cmd_id)
}

pub fn device_by_id(store: &Store, id: &str) -> Result<AuthenticatedDevice> {
    #[derive(Deserialize)]
    struct Authority {
        assistant: bool,
        assistant_act: bool,
        act: bool,
        reply: bool,
    }
    #[derive(Deserialize)]
    struct Device {
        name: String,
        revoked_at: Option<String>,
        authority: Option<Authority>,
    }
    const DEVICES: RecordArea<Device> = RecordArea::new("devices");
    let record = DEVICES
        .load(store, id)?
        .context("not_owner_input: device is no longer enrolled")?;
    let authority = record
        .authority
        .context("not_owner_input: device authority is unavailable")?;
    ensure!(
        record.revoked_at.is_none(),
        "not_owner_input: device was revoked"
    );
    Ok(AuthenticatedDevice {
        id: id.into(),
        name: record.name,
        assistant: authority.assistant,
        assistant_act: authority.assistant_act,
        reply: authority.reply,
        act: authority.act,
    })
}

/// Decode only daemon-owned authority fields; missing grants fail closed for remote inputs.
pub(crate) fn recheck_authority(store: &Store, device: &str, assistant_act: bool) -> Result<()> {
    let authority = device_by_id(store, device)?;
    ensure!(
        authority.assistant && (!assistant_act || (authority.assistant_act && authority.act)),
        "not_owner_input: device authority was revoked"
    );
    Ok(())
}

pub(crate) fn recheck_direct(store: &Store, device: &str) -> Result<()> {
    let authority = device_by_id(store, device)?;
    ensure!(authority.act, "authority_act: device authority was revoked");
    Ok(())
}

/// Non-wire proof capability. Only the daemon may construct this after validating and
/// consuming its device-bound preview proof for this exact prompt revision and choice.
pub struct FreshConfirmation {
    device: String,
    key: String,
    rev: u32,
    choice: corral_protocol::assistant::PromptChoice,
    at: Timestamp,
}

impl FreshConfirmation {
    pub fn validated(
        device: &str,
        key: &str,
        rev: u32,
        choice: corral_protocol::assistant::PromptChoice,
    ) -> Self {
        Self {
            device: device.into(),
            key: key.into(),
            rev,
            choice,
            at: Timestamp::now(),
        }
    }

    pub(crate) fn require(
        &self,
        device: &str,
        answer: &corral_protocol::assistant::PromptAnswer,
    ) -> Result<()> {
        let age = Timestamp::now()
            .as_second()
            .saturating_sub(self.at.as_second());
        ensure!(
            self.device == device
                && self.key == answer.key
                && self.rev == answer.rev
                && self.choice == answer.choice
                && (0..=60).contains(&age),
            "confirmation_required: fresh device-bound prompt proof required"
        );
        Ok(())
    }
}

/// The daemon validates/consumes the wire proof before invoking this helper.
pub fn prompt_answer(
    ctx: &crate::Ctx,
    device: &AuthenticatedDevice,
    answer: &corral_protocol::assistant::PromptAnswer,
    proof: Option<&FreshConfirmation>,
) -> Result<serde_json::Value> {
    crate::agent::ops::owner_prompt::answer(ctx, device, answer, proof)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_and_future_typing_cannot_be_fresh_authority() {
        let now: Timestamp = "2026-11-14T12:00:00Z".parse().unwrap();
        assert_eq!(
            fresh("2026-11-14T11:54:59Z".parse().unwrap(), now),
            (true, false)
        );
        assert_eq!(
            fresh("2026-11-14T12:00:31Z".parse().unwrap(), now),
            (false, true)
        );
        assert_eq!(
            fresh("2026-11-14T11:59:59Z".parse().unwrap(), now),
            (false, false)
        );
    }
}

#[cfg(test)]
#[test]
fn prompt_capability_cannot_cross_device_revision_or_choice() {
    use corral_protocol::assistant::{PromptAnswer, PromptChoice};
    let proof = FreshConfirmation::validated("dev-native", "A-1/P1", 1, PromptChoice::Confirm);
    let mut answer = PromptAnswer {
        cmd_id: "answer".into(),
        key: "A-1/P1".into(),
        rev: 1,
        choice: PromptChoice::Confirm,
        confirm: None,
    };
    assert!(proof.require("dev-native", &answer).is_ok());
    assert!(proof.require("dev-other", &answer).is_err());
    answer.rev = 2;
    assert!(proof.require("dev-native", &answer).is_err());
    answer.rev = 1;
    answer.choice = PromptChoice::Once;
    assert!(proof.require("dev-native", &answer).is_err());
}
