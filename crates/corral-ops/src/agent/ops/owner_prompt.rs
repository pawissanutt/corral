//! Device-bound remote permission/confirmation prompts. Hosts can describe a permission
//! request and poll its result, never answer it. Confirmation prompts originate only from
//! canonical owner_action resolution, not from host-supplied command prose. Daemon answers
//! require current device authority and a consumed fresh confirmation capability.
use anyhow::{Context, Result, ensure};
use corral_core::agent::paths::sha256_hex;
use corral_core::agent::write_permissions::{PermissionChoice, PermissionPath};
use corral_core::agent::{AGENT_RECORD_VERSION, ReceiptState, RequestIdentity, RequestReceipt};
use corral_core::assistant::{NewOwnerInput, OwnerInputSource};
use corral_core::{Store, records::RecordArea};
use corral_protocol::assistant::{PromptAnswer, PromptChoice};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::app_runtime::{
    AuthenticatedDevice,
    input::{self, FreshConfirmation},
};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::envelope::Envelope;
use crate::assistant::permissions::drain_permission_audit;

const PROMPTS: RecordArea<Prompt> = RecordArea::new("app_owner_prompts");
const INDEX: RecordArea<Vec<String>> = RecordArea::new("app_owner_prompt_index");
const PROMPTS_MAX: usize = 64;
const ANSWERS: RecordArea<AnswerReceipt> = RecordArea::new("app_owner_prompt_answers");

#[derive(Serialize, Deserialize)]
struct AnswerReceipt {
    digest: String,
    result: Value,
    settled_at: Timestamp,
    retain_until: Timestamp,
}

fn answer_key(device: &str, cmd_id: &str) -> String {
    sha256_hex(format!("{device}\0{cmd_id}").as_bytes())
}

fn answer_digest(answer: &PromptAnswer) -> Result<String> {
    Ok(sha256_hex(&serde_json::to_vec(&(
        &answer.key,
        answer.rev,
        answer.choice,
    ))?))
}

pub fn result(store: &Store, device: &str, cmd_id: &str) -> Result<Option<Value>> {
    Ok(ANSWERS
        .load(store, &answer_key(device, cmd_id))?
        .map(|saved| saved.result))
}

pub fn replay(store: &Store, device: &str, answer: &PromptAnswer) -> Result<Option<Value>> {
    let Some(saved) = ANSWERS.load(store, &answer_key(device, &answer.cmd_id))? else {
        return Ok(None);
    };
    ensure!(
        saved.digest == answer_digest(answer)?,
        "payload_mismatch: prompt command id reused"
    );
    Ok(Some(saved.result))
}
const PREVIEW_MAX: usize = 8 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub key: String,
    pub revision: u32,
    pub session: String,
    pub generation: u32,
    pub device: String,
    pub input: String,
    pub kind: String,
    pub request: String,
    pub command: Option<String>,
    pub descriptor: Value,
    pub created: Timestamp,
    pub expires_at: Timestamp,
    pub state: String,
    pub authorized: bool,
    pub answer_input: Option<String>,
    pub answer_cmd: Option<String>,
    pub answer_choice: Option<PromptChoice>,
    identity: Option<RequestIdentity>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Request {
    Open {
        kind: String,
        #[serde(flatten)]
        fields: Value,
    },
    Get {
        key: String,
    },
}

fn bad(message: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, message).into()
}

fn key(session: &str, generation: u32, kind: &str, request: &str) -> Result<String> {
    let digest = sha256_hex(format!("{generation}\0{kind}\0{request}").as_bytes());
    let n = u64::from_str_radix(&digest[..16], 16).context("encoding prompt key")?;
    Ok(format!("{session}/P{n}"))
}

fn status(prompt: &Prompt) -> Value {
    let state = if prompt.state == "open" && Timestamp::now() > prompt.expires_at {
        "expired"
    } else {
        &prompt.state
    };
    json!({"key": prompt.key, "revision": prompt.revision, "state": state,
        "kind": prompt.kind, "authorized": prompt.authorized, "input": prompt.answer_input,
        "descriptor": prompt.descriptor, "command": prompt.command, "expires_at": prompt.expires_at})
}

pub fn get(store: &Store, key: &str) -> Result<Prompt> {
    PROMPTS.load(store, key)?.context("unknown owner prompt")
}

pub fn list(store: &Store, session: &str) -> Result<Vec<Prompt>> {
    INDEX
        .load(store, session)?
        .unwrap_or_default()
        .iter()
        .map(|key| get(store, key))
        .collect()
}

fn save_new(store: &Store, prompt: Prompt) -> Result<Prompt> {
    if let Some(saved) = PROMPTS.load(store, &prompt.key)? {
        ensure!(
            saved.request == prompt.request
                && saved.kind == prompt.kind
                && saved.device == prompt.device
                && saved.input == prompt.input
                && saved.descriptor == prompt.descriptor
                && saved.command == prompt.command,
            "payload_mismatch: prompt identity reused"
        );
        return Ok(saved);
    }
    let mut keys = INDEX.load(store, &prompt.session)?.unwrap_or_default();
    if keys.len() >= PROMPTS_MAX {
        let removable = keys.iter().position(|key| {
            get(store, key).is_ok_and(|p| p.state != "open" || p.expires_at < Timestamp::now())
        });
        let index = removable.context("too many unanswered owner prompts")?;
        // Retain the keyed terminal record for retry/recovery; only the active projection shrinks.
        keys.remove(index);
    }
    keys.push(prompt.key.clone());
    PROMPTS.save(store, &prompt.key, &prompt)?;
    INDEX.save(store, &prompt.session, &keys)?;
    Ok(prompt)
}

/// Called only after owner_action has resolved its exact canonical command and cited input.
pub(super) fn confirmation(
    ctx: &Ctx,
    session: &str,
    request: &str,
    command: &str,
    cited: &str,
) -> Result<String> {
    ctx.store.update_assistant_container(session, |assistant| {
        let entry = assistant.owner_input(cited).context("not_owner_input")?;
        let origin = input::recheck(&ctx.store, assistant, entry)?
            .context("confirmation requires device input")?;
        let now = Timestamp::now();
        let prompt = Prompt {
            key: key(
                session,
                assistant.state.binding.generation,
                "confirmation",
                request,
            )?,
            revision: 1,
            session: session.into(),
            generation: assistant.state.binding.generation,
            device: origin.device,
            input: cited.into(),
            kind: "confirmation".into(),
            request: request.into(),
            command: Some(command.into()),
            descriptor: json!({"command": command}),
            created: now,
            expires_at: now.checked_add(jiff::SignedDuration::from_secs(60))?,
            state: "open".into(),
            authorized: false,
            answer_input: None,
            answer_cmd: None,
            answer_choice: None,
            identity: None,
        };
        Ok(save_new(&ctx.store, prompt)?.key)
    })
}

/// Attachment confirmations are authority only for the exact input/command that the device
/// saw. Request IDs have no stored owner_action receipt until confirmation is complete.
pub(crate) fn check_attachment_confirmation(
    store: &Store,
    input: &corral_core::assistant::OwnerInput,
    answer: &corral_core::assistant::OwnerInput,
    request: &str,
    command: &str,
) -> Result<()> {
    let session = input
        .id
        .split('/')
        .next()
        .context("not_owner_input: invalid input identity")?;
    let prompt = get(
        store,
        &key(session, answer.generation, "confirmation", request)?,
    )?;
    ensure!(
        prompt.state == "answered"
            && prompt.input == input.id
            && prompt.request == request
            && prompt.command.as_deref() == Some(command)
            && prompt.answer_input.as_deref() == Some(answer.id.as_str()),
        "not_owner_input: confirmation belongs to a different exact input or command"
    );
    Ok(())
}

/// Authenticated host open/get. Polls do not consume the bounded host receipt journal.
pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let request: Request = serde_json::from_value(envelope.payload.clone())
        .map_err(|_| bad("owner_prompt expects open or get"))?;
    let admitted = match admission::admit(ctx, envelope, Mode::Lookup)? {
        Admission::New(value) => value,
        Admission::Replay(stored) => {
            return crate::agent::receipts::respond(
                ctx,
                &admission::container_of(&envelope.identity),
                *stored,
                true,
            );
        }
    };
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("owner_prompt requires an assistant")?;
    let prompt = ctx.store.update_assistant_container(session, |assistant| {
        admission::recheck_assistant(assistant, false, envelope.identity.generation)?;
        ensure!(
            assistant
                .state
                .current_generation()
                .is_some_and(|g| g.daemon_hosted),
            "owner_prompt is remote-only"
        );
        crate::assistant::workflow::boundary::require_ordinary(&ctx.store, session, assistant)?;
        match request {
            Request::Get { key } => {
                let prompt = get(&ctx.store, &key)?;
                ensure!(
                    prompt.session == session && prompt.generation == envelope.identity.generation,
                    "prompt belongs to another host generation"
                );
                Ok(prompt)
            }
            Request::Open { kind, fields } if kind == "confirmation" => {
                let request = fields["request"]
                    .as_str()
                    .context("confirmation request required")?;
                let prompt = get(
                    &ctx.store,
                    &key(session, envelope.identity.generation, &kind, request)?,
                )?;
                ensure!(
                    prompt.session == session
                        && prompt.generation == envelope.identity.generation
                        && prompt.command.as_deref() == fields["command"].as_str(),
                    "confirmation does not match canonical owner_action"
                );
                Ok(prompt)
            }
            Request::Open { kind, mut fields } if kind == "permission" => {
                let cited = fields["input"]
                    .as_str()
                    .context("permission requires initiating input")?
                    .to_owned();
                let entry = assistant.owner_input(&cited).context("not_owner_input")?;
                let origin = input::permission_origin(&ctx.store, entry)?;
                let preview = fields["preview"]
                    .as_str()
                    .context("permission preview required")?;
                ensure!(
                    preview.len() <= PREVIEW_MAX
                        && fields["preview_sha256"].as_str()
                            == Some(sha256_hex(preview.as_bytes()).as_str()),
                    "permission preview digest mismatch or oversized preview"
                );
                let scopes: Vec<PermissionPath> = serde_json::from_value(
                    fields.get("scopes").cloned().unwrap_or_else(|| json!([])),
                )?;
                ensure!(scopes.len() <= 4, "too many permission scopes");
                let descriptor = fields.clone();
                let object = fields
                    .as_object_mut()
                    .context("permission fields required")?;
                for field in ["input", "preview", "scopes"] {
                    object.remove(field);
                }
                object.insert("action".into(), json!("decide"));
                object.insert("choice".into(), json!("deny"));
                object.remove("scope");
                let new = super::write_permission::validate_remote(&envelope.identity, fields)?;
                let now = Timestamp::now();
                save_new(
                    &ctx.store,
                    Prompt {
                        key: key(
                            session,
                            envelope.identity.generation,
                            "permission",
                            &new.request,
                        )?,
                        revision: 1,
                        session: session.into(),
                        generation: envelope.identity.generation,
                        device: origin.device,
                        input: cited,
                        kind,
                        request: new.request,
                        command: None,
                        descriptor,
                        created: now,
                        expires_at: now.checked_add(jiff::SignedDuration::from_secs(60))?,
                        state: "open".into(),
                        authorized: false,
                        answer_input: None,
                        answer_cmd: None,
                        answer_choice: None,
                        identity: Some(envelope.identity.clone()),
                    },
                )
            }
            _ => Err(bad("unknown owner prompt kind")),
        }
    })?;
    let at = Timestamp::now();
    Ok(CallResponse::Receipt {
        receipt: RequestReceipt {
            version: AGENT_RECORD_VERSION,
            request_id: admitted.origin.request_id,
            op: admitted.origin.op,
            payload_sha256: admitted.origin.payload_sha256,
            identity: admitted.origin.identity,
            state: ReceiptState::Committed,
            result: status(&prompt),
            executor: None,
            effect: None,
            evidence: Vec::new(),
            post_pending: Vec::new(),
            at,
            committed_at: Some(at),
        },
        replayed: false,
    })
}

/// Daemon-only answer. Positive choices consume an independently validated preview proof.
/// Permission grants and their owner input publish with the prompt decision atomically.
pub fn answer(
    ctx: &Ctx,
    device: &AuthenticatedDevice,
    answer: &PromptAnswer,
    proof: Option<&FreshConfirmation>,
) -> Result<Value> {
    input::require_device(device)?;
    if let Some(saved) = replay(&ctx.store, &device.id, answer)? {
        return Ok(saved);
    }
    ensure!(device.reply, "authority_reply");
    let session = get(&ctx.store, &answer.key)?.session;
    let result = ctx
        .store
        .update_assistant_container(&session, |assistant| {
            if let Some(saved) = replay(&ctx.store, &device.id, answer)? {
                return Ok(saved);
            }
            let mut prompt = get(&ctx.store, &answer.key)?;
            ensure!(
                prompt.device == device.id,
                "prompt belongs to a different device"
            );
            if prompt.state == "answered" {
                ensure!(
                    prompt.answer_cmd.as_deref() == Some(answer.cmd_id.as_str())
                        && prompt.answer_choice == Some(answer.choice)
                        && answer.rev.checked_add(1) == Some(prompt.revision),
                    "conflict: prompt already answered"
                );
                return Ok(status(&prompt));
            }
            ensure!(
                prompt.revision == answer.rev,
                "conflict: stale prompt revision"
            );
            ensure!(
                Timestamp::now() <= prompt.expires_at,
                "expired: prompt requires fresh presence"
            );
            admission::recheck_assistant(assistant, false, prompt.generation)?;
            let affirmative = matches!(
                answer.choice,
                PromptChoice::Once
                    | PromptChoice::Hour
                    | PromptChoice::Session
                    | PromptChoice::Confirm
            );
            if affirmative {
                ensure!(device.act, "authority_act");
                if prompt.kind == "confirmation" {
                    ensure!(device.assistant_act, "authority_assistant_act");
                }
                input::recheck_direct(&ctx.store, &device.id)?;
                proof
                    .context("confirmation_required: fresh device-bound prompt proof required")?
                    .require(&device.id, answer)?;
            }
            input::recheck_authority(
                &ctx.store,
                &device.id,
                affirmative && prompt.kind == "confirmation",
            )?;
            ensure!(
                input::device_by_id(&ctx.store, &device.id)?.reply,
                "authority_reply: device authority was revoked"
            );
            let initiating = assistant
                .owner_input(&prompt.input)
                .context("not_owner_input: initiating input evicted")?;
            let mut origin = input::provenance(&ctx.store, &initiating.id)?
                .context("missing device input provenance")?;
            let now = Timestamp::now();
            let entry = if prompt.kind == "confirmation" {
                ensure!(
                    matches!(answer.choice, PromptChoice::Confirm | PromptChoice::Decline),
                    "confirmation requires confirm or decline"
                );
                let text = if affirmative { "y" } else { "n" };
                assistant.journal_owner_input(
                    &session,
                    NewOwnerInput {
                        generation: prompt.generation,
                        turn: None,
                        source: OwnerInputSource::Confirmation,
                        sha256: sha256_hex(text.as_bytes()),
                        text: text.into(),
                        truncated: false,
                        command: prompt.command.clone(),
                        answers: Some(prompt.request.clone()),
                        request: format!("app-prompt:{}", answer.cmd_id),
                        binding: None,
                    },
                )
            } else {
                let choice = match answer.choice {
                    PromptChoice::Once => PermissionChoice::Once,
                    PromptChoice::Hour => PermissionChoice::Hour,
                    PromptChoice::Session => PermissionChoice::Session,
                    PromptChoice::Deny => PermissionChoice::Deny,
                    _ => return Err(bad("permission requires once, hour, session or deny")),
                };
                let mut payload = prompt.descriptor.clone();
                let object = payload
                    .as_object_mut()
                    .context("invalid permission descriptor")?;
                let scope = if matches!(choice, PermissionChoice::Hour | PermissionChoice::Session)
                {
                    Some(
                        object
                            .get("scopes")
                            .and_then(Value::as_array)
                            .and_then(|v| v.first())
                            .cloned()
                            .context("no grant scope offered")?,
                    )
                } else {
                    None
                };
                for field in ["input", "preview", "scopes"] {
                    object.remove(field);
                }
                object.insert("action".into(), json!("decide"));
                object.insert("choice".into(), json!(choice));
                object.insert("scope".into(), scope.unwrap_or(Value::Null));
                let new = super::write_permission::validate_remote(
                    prompt
                        .identity
                        .as_ref()
                        .context("permission identity missing")?,
                    payload,
                )?;
                crate::assistant::workflow::boundary::require_ordinary(
                    &ctx.store, &session, assistant,
                )?;
                assistant
                    .record_permission(&session, new, now)
                    .map_err(|_| {
                        bad("grants_full: revoke an active grant before allowing another")
                    })?
                    .0
            };
            origin.typed_at = now;
            origin.received_at = now;
            origin.cmd_id = answer.cmd_id.clone();
            origin.late = false;
            origin.tainted = false;
            origin.assistant_act = device.assistant_act;
            origin.attachments = false;
            origin.sha256 = entry.sha256.clone();
            input::save_provenance(&ctx.store, &entry, &origin)?;
            prompt.state = "answered".into();
            prompt.revision += 1;
            prompt.authorized = affirmative;
            prompt.answer_input = Some(entry.id);
            prompt.answer_cmd = Some(answer.cmd_id.clone());
            prompt.answer_choice = Some(answer.choice);
            PROMPTS.save(&ctx.store, &prompt.key, &prompt)?;
            let result = status(&prompt);
            ANSWERS.save(
                &ctx.store,
                &answer_key(&device.id, &answer.cmd_id),
                &AnswerReceipt {
                    digest: answer_digest(answer)?,
                    result: result.clone(),
                    settled_at: now,
                    retain_until: now
                        .checked_add(jiff::SignedDuration::from_secs(7 * 24 * 3600))?,
                },
            )?;
            Ok(result)
        })?;
    // Canonical audit outbox remains durable if logging is temporarily unavailable.
    if let Err(error) = drain_permission_audit(&ctx.store, &session) {
        eprintln!("warning: {session}'s prompt audit remains queued: {error:#}");
    }
    Ok(result)
}
