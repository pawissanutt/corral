//! Canonical daemon-hosted Ask services. The daemon supplies an authenticated device, owns
//! the supervisor loop and transport journal, and never accepts authority from request JSON.
//! Sends are durable before provider preflight; a disconnected client cannot discard a turn.
use crate::Ctx;
use anyhow::{Context, Result, bail, ensure};
use corral_core::assistant::threads::{ContextRef, Message};
use corral_core::pm_inbox::{ItemKind, ItemState, NewPmItem, OwnerWords};
use corral_protocol::assistant::{AskTarget, AssistantSend, AssistantSessionOp, SendAck};
use corral_protocol::rows::AssistantRow;

pub mod actions;
pub mod attachments;
pub mod input;
mod projection;
mod runtime;
pub use crate::agent::ops::owner_prompt::{Prompt, get as prompt_get, list as prompts};
pub use actions::{ensure_filing_log, file_as_idea, withdraw};
pub use input::FreshConfirmation;
pub(crate) use projection::sessions_with_usage;
pub use projection::{filings, live, sessions, thread};
pub use runtime::{Runtime, RuntimeUpdate, pump, shutdown, start, stop};

pub fn resolve_scope(ctx: &Ctx, scope: &str) -> Result<Option<String>> {
    ctx.store.app_thread_scope(scope)
}

#[cfg(test)]
mod tests;

/// A server-derived principal, deliberately not deserializable from an app request.
#[derive(Debug, Clone)]
pub struct AuthenticatedDevice {
    pub id: String,
    pub name: String,
    pub assistant: bool,
    pub assistant_act: bool,
    pub reply: bool,
    pub act: bool,
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

pub(crate) fn excerpt(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub(crate) fn owner_context(ctx: &Ctx, device: &AuthenticatedDevice) -> Result<Ctx> {
    let mut attributed = ctx.clone();
    attributed.store = ctx.store.reattributed();
    attributed
        .store
        .set_attribution(corral_core::Actor::Owner, &format!("app:{}", device.id))?;
    attributed.store.set_attribution_detail(
        "attribution",
        serde_json::json!({
            "device": device.id, "name": device.name
        }),
    )?;
    Ok(attributed)
}

pub fn prompt_answer(
    ctx: &Ctx,
    device: &AuthenticatedDevice,
    answer: &corral_protocol::assistant::PromptAnswer,
    proof: Option<&FreshConfirmation>,
) -> Result<serde_json::Value> {
    input::prompt_answer(&owner_context(ctx, device)?, device, answer, proof)
}

/// Only an authenticated caller's own existing send key can be withdrawn here. Unknown keys
/// are handled by the daemon's bounded command journal, never an unbounded runtime tombstone.
pub fn cancel(
    ctx: &Ctx,
    device: &AuthenticatedDevice,
    cmd_id: &str,
) -> Result<Option<corral_protocol::assistant::CancelAck>> {
    input::recheck_authority(&ctx.store, &device.id, false)?;
    ensure!(device.assistant, "authority_assistant");
    ensure!(
        valid_key(&device.id) && valid_key(cmd_id),
        "invalid_params: invalid send identity"
    );
    Ok(ctx
        .store
        .cancel_app_send(&device.id, cmd_id)?
        .map(
            |(cancelled, message)| corral_protocol::assistant::CancelAck {
                cancelled,
                entry: Some(message.id.clone()),
                state: Some(projection::message_state(&message)),
            },
        ))
}
/// Exactly-once keyed append. Provider failures never erase this acknowledgement or the words.
pub fn send(ctx: &Ctx, device: &AuthenticatedDevice, request: &AssistantSend) -> Result<SendAck> {
    input::recheck_authority(&ctx.store, &device.id, false)?;
    let attributed = owner_context(ctx, device)?;
    let ctx = &attributed;
    ensure!(device.assistant, "authority_assistant");
    ensure!(
        valid_key(&device.id) && valid_key(&request.cmd_id),
        "invalid send identity"
    );
    ensure!(
        !request.text.trim().is_empty() && request.text.len() <= 16 * 1024,
        "message must contain 1..16384 bytes"
    );
    let _: jiff::Timestamp = request.typed_at.parse().context("invalid typed_at")?;
    ensure!(
        request.context.len() <= 4
            && request
                .context
                .iter()
                .all(|c| c.kind.len() <= 32 && c.id.len() <= 160),
        "context exceeds bounds"
    );
    let target = match request.target {
        AskTarget::Assistant => "assistant",
        AskTarget::Pm => "pm",
        _ => bail!("unsupported Ask target"),
    };
    ensure!(
        target != "pm" || request.attachments.is_empty(),
        "attachments_unsupported_target"
    );
    let (attachments, uploads) = attachments::references(&request.attachments)?;
    let head = ctx.store.app_thread(&request.thread)?;
    let digest = super::payload_sha256(&serde_json::to_value(request)?);
    let (message, replayed) = ctx.store.append_app_message(
        Message {
            id: String::new(),
            session: head.session.clone(),
            seq: 0,
            cmd_id: request.cmd_id.clone(),
            device: device.id.clone(),
            name: excerpt(&device.name, 120),
            target: target.into(),
            text: request.text.clone(),
            typed_at: request.typed_at.clone(),
            at: ctx.store.now().to_string(),
            state: if target == "pm" { "queued_pm" } else { "sent" }.into(),
            input: None,
            pm_item: None,
            saved_task: None,
            reply: None,
            failure: None,
            turn: None,
            generation: None,
            late: false,
            tainted: false,
            model: None,
            attachments,
            context: request
                .context
                .iter()
                .map(|c| ContextRef {
                    kind: c.kind.clone(),
                    id: c.id.clone(),
                    rev: c.rev,
                })
                .collect(),
            rev: 1,
        },
        &digest,
        &uploads,
    )?;
    if !replayed && target == "assistant" {
        let journal = input::journal(
            &ctx.store,
            &message.session,
            device,
            &message.cmd_id,
            &message.text,
            &message.typed_at,
            !message.attachments.is_empty(),
        )?;
        let provenance = input::provenance(&ctx.store, &journal.id)?;
        ctx.store.update_app_message(&message.id, |m| {
            m.input = Some(journal.id);
            m.late = provenance.as_ref().is_some_and(|p| p.late);
            m.tainted = provenance.as_ref().is_some_and(|p| p.tainted);
            if provenance.as_ref().is_some_and(|p| p.late) && m.state == "sent" {
                m.state = "late".into();
            }
            Ok(())
        })?;
        if !matches!(head.state.as_str(), "working" | "starting" | "ready") {
            ctx.store
                .set_app_thread_state(&head.session, "cold", None)?;
        }
    } else if !replayed && target == "pm" && head.state == "cold" {
        // PM input is inbox evidence, never an assistant-citable owner instruction.
        ctx.store
            .set_app_thread_state(&head.session, "pm_wait", None)?;
    }
    // A crash between append and journal is recovered by the supervisor from saved provenance.
    if target == "pm" {
        reconcile_pm(ctx, &message.session)?;
    }
    let current = ctx.store.app_message(&message.id)?;
    Ok(SendAck {
        entry: current.id.clone(),
        state: projection::message_state(&current),
        replayed,
    })
}

pub fn warm(ctx: &Ctx, device: &AuthenticatedDevice, scope: &str) -> Result<()> {
    input::recheck_authority(&ctx.store, &device.id, false)?;
    let attributed = owner_context(ctx, device)?;
    let ctx = &attributed;
    ensure!(device.assistant, "authority_assistant");
    let head = ctx.store.app_thread(scope)?;
    ctx.store.set_app_thread_state(&head.session, "cold", None)
}

/// Restart discovery is independent of app subscriptions. Failed setup waits for an explicit
/// send/warm instead of continuously loading a missing provider in the background.
pub fn pending_scopes(ctx: &Ctx) -> Result<Vec<String>> {
    let mut scopes = Vec::new();
    for session in ctx.store.list_assistants()? {
        if session.ended.is_some() {
            continue;
        }
        let Ok(head) = ctx.store.app_thread_head(&session.id) else {
            continue;
        };
        reconcile_pm(ctx, &session.id)?;
        let assistant_pending = ctx
            .store
            .pending_app_messages(&session.id)?
            .iter()
            .any(|m| m.target == "assistant");
        if !matches!(head.state.as_str(), "failed" | "ended")
            && (matches!(head.state.as_str(), "cold" | "takeover") || assistant_pending)
        {
            scopes.push(session.id);
        }
    }
    Ok(scopes)
}

pub fn session(
    ctx: &Ctx,
    device: &AuthenticatedDevice,
    op: &AssistantSessionOp,
) -> Result<AssistantRow> {
    input::recheck_authority(&ctx.store, &device.id, false)?;
    let attributed = owner_context(ctx, device)?;
    let ctx = &attributed;
    ensure!(device.assistant, "authority_assistant");
    let id = match op {
        AssistantSessionOp::New { task, profile } => {
            if let Some(task) = task {
                ctx.store.head(task)?;
            }
            let profile = profile
                .as_deref()
                .or(ctx.store.config.assistant.agent.as_deref())
                .unwrap_or(&ctx.store.config.pm.agent);
            // Eligibility is canonical; credentials/assets remain deferred until supervisor start.
            ctx.store.config.resolve_profile(
                &corral_core::ProfileChoice {
                    profile: Some(profile.into()),
                    ..Default::default()
                },
                None,
                None,
            )?;
            let session = ctx
                .store
                .create_assistant(profile, task.as_deref(), None, None)?;
            ctx.store.app_thread(&session.id)?;
            session.id
        }
        AssistantSessionOp::End { session } => {
            super::ops::assistant::end(ctx, session)?;
            if ctx.store.app_thread_head(session).is_ok() {
                ctx.store.set_app_thread_state(session, "ended", None)?;
            }
            session.clone()
        }
        AssistantSessionOp::TakeOver { session } => {
            let container = ctx.store.get_assistant_container(session)?;
            let stopping = super::pi::host_alive(&container.state)?;
            if stopping {
                let dir = super::launch::assistant_runtime_dir(&ctx.store, session)
                    .join(format!("g{}", container.state.binding.generation));
                let mut bridge = super::bridge::BridgeClient::connect_dir(
                    &dir,
                    container.state.binding.generation,
                )?;
                bridge.shutdown()?;
            }
            ctx.store.app_thread(session)?;
            ctx.store.set_app_thread_state(
                session,
                if stopping { "takeover" } else { "cold" },
                None,
            )?;
            session.clone()
        }
        AssistantSessionOp::SetProfile { thread, profile } => {
            ctx.store.config.resolve_profile(
                &corral_core::ProfileChoice {
                    profile: Some(profile.clone()),
                    ..Default::default()
                },
                None,
                None,
            )?;
            let head = ctx.store.app_thread(thread)?;
            ensure!(
                head.pending.is_empty() && !matches!(head.state.as_str(), "working" | "starting"),
                "assistant_busy"
            );
            ctx.store.update_assistant(&head.session, |s| {
                s.profile = profile.clone();
                Ok(())
            })?;
            // A running generation cannot silently keep the previous model after this operation.
            let container = ctx.store.get_assistant_container(&head.session)?;
            if super::pi::host_alive(&container.state)? {
                let dir = super::launch::assistant_runtime_dir(&ctx.store, &head.session)
                    .join(format!("g{}", container.state.binding.generation));
                super::bridge::BridgeClient::connect_dir(&dir, container.state.binding.generation)?
                    .shutdown()?;
            }
            ctx.store
                .set_app_thread_state(&head.session, "cold", None)?;
            head.session
        }
        _ => bail!("unsupported assistant session operation"),
    };
    projection::row(ctx, &id)
}

/// PM work is posted through the existing canonical inbox, never directly steered into a PM
/// host. Its keyed item and outcome are reconciled back into the originating durable thread.
pub(crate) fn reconcile_pm(ctx: &Ctx, session: &str) -> Result<()> {
    for message in ctx.store.pending_app_messages(session)? {
        if message.target != "pm" || message.state == "withdrawn" {
            continue;
        }
        let item = if let Some(id) = &message.pm_item {
            ctx.store.pm_inbox_item(id)?
        } else {
            let store = ctx.store.reattributed();
            store.set_attribution(
                corral_core::Actor::Owner,
                &format!("app:{}", message.device),
            )?;
            store.set_attribution_detail(
                "attribution",
                serde_json::json!({
                    "device": message.device, "name": message.name
                }),
            )?;
            let (item, _) = store.pm_inbox_post(NewPmItem {
                from: "owner".into(),
                kind: ItemKind::Question,
                subject: excerpt(&message.text, 120),
                body: message.text.clone(),
                refs: message.context.iter().map(|c| c.id.clone()).collect(),
                requested_action: None,
                owner_words: Some(OwnerWords {
                    input: message.input.clone(),
                    sha256: corral_core::agent::paths::sha256_hex(message.text.as_bytes()),
                    quote: message.text.clone(),
                }),
                key: Some(format!("app:{}:{}", message.device, message.cmd_id)),
            })?;
            ctx.store.update_app_message(&message.id, |m| {
                m.pm_item = Some(item.id.clone());
                m.state = "queued_pm".into();
                Ok(())
            })?;
            item
        };
        if let Some(outcome) = item.outcome {
            ctx.store.update_app_message(&message.id, |m| {
                m.reply = Some(outcome.text);
                m.state = "pm_replied".into();
                Ok(())
            })?;
        } else if item.state == ItemState::Claimed && message.state != "pm_delivered" {
            ctx.store.update_app_message(&message.id, |m| {
                m.state = "pm_delivered".into();
                Ok(())
            })?;
        } else if item.state == ItemState::Withdrawn {
            ctx.store.update_app_message(&message.id, |m| {
                m.state = "withdrawn".into();
                Ok(())
            })?;
        }
    }
    Ok(())
}
