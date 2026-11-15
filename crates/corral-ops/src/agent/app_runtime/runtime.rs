//! One supervised real Pi host per claimed conversation. A host is never stopped for idleness.
use super::{AuthenticatedDevice, excerpt};
use crate::Ctx;
use crate::agent::{bridge::BridgeClient, hooks::pm::Overrides, launch, ops::assistant::Claim};
use anyhow::{Context, Result, ensure};
use corral_protocol::assistant::{StopAck, StopOutcome, TurnCompletion};
use corral_protocol::subscription::LiveText;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub struct Runtime {
    session: String,
    generation: u32,
    _claim: Claim,
    #[cfg(unix)]
    host: Option<launch::daemon::DaemonHost>,
    bridge: Option<BridgeClient>,
    cursor: u64,
    started: Instant,
    active: Option<String>,
    exited: bool,
}
impl Runtime {
    pub fn session(&self) -> &str {
        &self.session
    }
}

#[derive(Debug, Default)]
pub struct RuntimeUpdate {
    pub live: Vec<LiveText>,
    pub changed: bool,
    pub exited: bool,
}

// Pause and submission share a lease: a submitted message keeps its real outcome, while a
// pause that wins the lease prevents even the delivery claim from being published.
pub(crate) fn control(ctx: &Ctx, session: &str) -> Result<std::fs::File> {
    corral_core::assistant::validate_id(session)?;
    let dir = ctx.store.dir().join("lock");
    std::fs::create_dir_all(&dir).context("creating assistant control lock directory")?;
    let path = dir.join(format!("assistant-control-{session}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(file)
}

pub(super) fn pause(ctx: &Ctx, session: &str) -> Result<()> {
    let inbox = corral_core::inbox::Inbox::new(
        &ctx.store,
        corral_core::inbox::Recipient::Assistant {
            session: session.into(),
        },
    )?;
    let _delivery = inbox
        .delivery_lock()
        .context("locking assistant inbox for pause")?;
    let _control = control(ctx, session)?;
    let head = ctx.store.app_thread(session)?;
    ensure!(
        ctx.store.get_assistant(session)?.ended.is_none() && head.state != "ended",
        "assistant_ended"
    );
    ensure!(head.state != "failed", "assistant_failed");
    ensure!(head.state != "paused", "already_paused");
    let container = ctx.store.get_assistant_container(session)?;
    let generation = container.state.binding.generation;
    ensure!(
        container
            .state
            .generation(generation)
            .is_some_and(|g| g.daemon_hosted),
        "not_daemon_hosted"
    );
    if super::super::pi::host_alive(&container.state)? {
        let dir = launch::assistant_runtime_dir(&ctx.store, session).join(format!("g{generation}"));
        let mut client = BridgeClient::connect_dir(&dir, generation)?;
        let state = client.get_state()?;
        if state["turn"]["open"] == true {
            let turn = state["turn"]["current"]
                .as_u64()
                .context("host state has no current turn")?;
            // Idle/turn-not-current replies mean the observed turn already settled.
            client.cancel(Some(turn))?;
        }
    }
    ctx.store.set_app_thread_state(session, "paused", None)
}

pub(super) fn resume(ctx: &Ctx, session: &str) -> Result<()> {
    let _control = control(ctx, session)?;
    let head = ctx.store.app_thread(session)?;
    ensure!(
        ctx.store.get_assistant(session)?.ended.is_none() && head.state != "ended",
        "assistant_ended"
    );
    ensure!(head.state == "paused", "not_paused");
    let container = ctx.store.get_assistant_container(session)?;
    let state = if super::super::pi::host_alive(&container.state)? {
        "ready"
    } else {
        "cold"
    };
    ctx.store.set_app_thread_state(session, state, None)?;
    crate::availability::continuation::activate_ready(
        ctx,
        &corral_core::agent::ContainerRef::Assistant {
            session: session.into(),
        },
    )
}

fn unavailable(ctx: &Ctx, session: &str, reason: &str) -> Result<()> {
    let reason = excerpt(reason, 2048);
    let paused = ctx.store.app_thread_head(session)?.state == "paused";
    for message in ctx.store.pending_app_messages(session)? {
        if message.target != "assistant" {
            continue;
        }
        if paused && !matches!(message.state.as_str(), "working" | "delivered") {
            continue;
        }
        ctx.store.update_app_message(&message.id, |m| {
            if matches!(m.state.as_str(), "working" | "delivered") {
                m.state = "uncertain".into();
                m.failure = Some(format!(
                    "Turn interrupted; not resent automatically. {}",
                    excerpt(&reason, 1800)
                ));
            } else {
                m.state = "waiting".into();
                m.failure = Some(reason.clone());
            }
            Ok(())
        })?;
    }
    ctx.store
        .set_app_thread_state_unless(session, "failed", Some(reason), &["paused", "ended"])
}

pub fn start(ctx: &Ctx, scope: &str) -> Result<Runtime> {
    let head = ctx.store.app_thread(scope)?;
    let _input = super::super::ops::assistant::claim_input(ctx, &head.session)?
        .context("assistant_input_busy: start is deferred")?;
    ensure!(
        crate::session_control::automation_may_start(
            ctx,
            &corral_core::inbox::Recipient::Assistant {
                session: head.session.clone()
            },
        )?,
        "assistant_input_deferred: start is deferred"
    );
    let result = start_inner(ctx, &head.session);
    if let Err(error) = &result {
        let busy_claim = error
            .downcast_ref::<super::super::pi::PiRefusal>()
            .is_some_and(|refusal| refusal.code == "assistant_claimed");
        let stopping = head.state == "takeover"
            && (busy_claim
                || super::super::pi::host_alive(
                    &ctx.store.get_assistant_container(&head.session)?.state,
                )?);
        if stopping {
            ctx.store.set_app_thread_state(
                &head.session,
                "takeover",
                Some("Waiting for the explicitly stopped host to release this session".into()),
            )?;
        } else {
            unavailable(ctx, &head.session, &format!("{error:#}"))?;
        }
    }
    result
}

#[cfg(unix)]
fn start_inner(ctx: &Ctx, session: &str) -> Result<Runtime> {
    let container = ctx.store.get_assistant_container(session)?;
    let current = container.state.binding.generation;
    if super::super::pi::host_alive(&container.state)? {
        ensure!(
            container
                .state
                .generation(current)
                .is_some_and(|g| g.daemon_hosted),
            "assistant_in_pane: use explicit take-over before daemon launch"
        );
        let claim = super::super::ops::assistant::claim(&ctx.store, session)?;
        ctx.store
            .set_app_thread_state_unless(session, "ready", None, &["paused", "ended"])?;
        return Ok(Runtime {
            session: session.into(),
            generation: current,
            _claim: claim,
            host: None,
            bridge: None,
            cursor: 0,
            started: Instant::now(),
            active: None,
            exited: false,
        });
    }
    // No live host can attest what happened after a written submit. Do not repeat model effects.
    for message in ctx.store.pending_app_messages(session)? {
        if matches!(message.state.as_str(), "working" | "delivered") {
            ctx.store.update_app_message(&message.id, |m| {
                m.state = "uncertain".into();
                m.failure =
                    Some("Host stopped before this turn settled; not resent automatically".into());
                Ok(())
            })?;
        }
    }
    let (prepared, claim) = launch::daemon::resume_claimed(ctx, session, &Overrides::default())?;
    let host = launch::daemon::spawn(&prepared)?;
    let generation = host.generation;
    ctx.store
        .set_app_thread_state_unless(session, "starting", None, &["paused", "ended"])?;
    Ok(Runtime {
        session: session.into(),
        generation,
        _claim: claim,
        host: Some(host),
        bridge: None,
        cursor: 0,
        started: Instant::now(),
        active: None,
        exited: false,
    })
}

#[cfg(not(unix))]
fn start_inner(_ctx: &Ctx, _session: &str) -> Result<Runtime> {
    anyhow::bail!("daemon assistant runtime requires Unix")
}

fn connect(ctx: &Ctx, runtime: &mut Runtime) -> Result<bool> {
    if runtime.bridge.is_some() {
        return Ok(true);
    }
    let dir = launch::assistant_runtime_dir(&ctx.store, &runtime.session)
        .join(format!("g{}", runtime.generation));
    match BridgeClient::connect_dir(&dir, runtime.generation) {
        Ok(mut client) => {
            let hello = client.hello()?;
            if hello["bound"] != true || hello["remote_ready"] != true {
                ensure!(
                    runtime.started.elapsed() < Duration::from_secs(30),
                    "assistant host setup did not become ready"
                );
                return Ok(false);
            }
            runtime.bridge = Some(client);
            ctx.store.set_app_thread_state_unless(
                &runtime.session,
                "ready",
                None,
                &["paused", "ended"],
            )?;
            Ok(true)
        }
        Err(error) if runtime.started.elapsed() < Duration::from_secs(30) => {
            let _ = error;
            Ok(false)
        }
        Err(error) => Err(error).context("assistant host did not open its authenticated bridge"),
    }
}

fn context(ctx: &Ctx, message: &corral_core::assistant::threads::Message) -> Result<String> {
    let mut text = String::new();
    for reference in &message.context {
        match reference.kind.as_str() {
            "task" => {
                let brief = crate::assistant::task_brief(&ctx.store, &reference.id)?;
                text.push_str(&excerpt(&brief, 4 * 1024));
                text.push('\n');
            }
            "plan" => {
                let task = ctx.store.head(&reference.id)?;
                text.push_str(&format!(
                    "Plan reference {}: {} (read-only)\n",
                    task.id,
                    excerpt(&task.title, 200)
                ));
            }
            "session" => {
                let session = ctx.store.get_assistant(&reference.id)?;
                text.push_str(&format!(
                    "Assistant session {} focus {} (read-only)\n",
                    session.id, session.focus
                ));
            }
            _ => text.push_str(&format!(
                "Context reference {}:{}; no grounded excerpt available\n",
                reference.kind, reference.id
            )),
        }
    }
    Ok(excerpt(&text, 16 * 1024))
}

fn bound_json_text(text: &mut String, budget: usize) {
    let mut used = 0;
    let mut end = text.len();
    for (index, ch) in text.char_indices() {
        let cost = match ch {
            '"' | '\\' | '\n' | '\r' | '\t' => 2,
            ch if ch < ' ' => 6,
            ch => ch.len_utf8(),
        };
        if used + cost > budget {
            end = index;
            break;
        }
        used += cost;
    }
    text.truncate(end);
}

fn deliver(ctx: &Ctx, runtime: &mut Runtime) -> Result<bool> {
    if !crate::session_control::automation_may_start(
        ctx,
        &corral_core::inbox::Recipient::Assistant {
            session: runtime.session.clone(),
        },
    )? {
        return Ok(false);
    }
    let Some(client) = runtime.bridge.as_mut() else {
        return Ok(false);
    };
    let state = client.get_state()?;
    if state["turn"]["open"].as_bool() == Some(true) {
        return Ok(false);
    }
    if runtime.active.is_some() {
        return Ok(false);
    }
    let Some(message) = ctx
        .store
        .pending_app_messages(&runtime.session)?
        .into_iter()
        .find(|m| {
            m.target == "assistant" && matches!(m.state.as_str(), "sent" | "waiting" | "late")
        })
    else {
        return Ok(false);
    };
    // Reconstruct only from durable authenticated provenance, never from host request fields.
    let device = super::input::device_for_message(&ctx.store, &message)?;
    let input = match &message.input {
        Some(id) => id.clone(),
        None => {
            let input = super::input::journal(
                &ctx.store,
                &message.session,
                &device,
                &message.cmd_id,
                &message.text,
                &message.typed_at,
                !message.attachments.is_empty(),
            )?;
            let provenance = super::input::provenance(&ctx.store, &input.id)?;
            ctx.store.update_app_message(&message.id, |m| {
                m.input = Some(input.id.clone());
                m.late = provenance.as_ref().is_some_and(|p| p.late);
                m.tainted = provenance.as_ref().is_some_and(|p| p.tainted);
                Ok(())
            })?;
            input.id
        }
    };
    let materialized = match super::attachments::materialize(
        ctx,
        &runtime.session,
        runtime.generation,
        &message,
    ) {
        Ok(files) => files,
        Err(error) => {
            ctx.store.update_app_message(&message.id, |m| {
                if matches!(m.state.as_str(), "sent" | "waiting" | "late") {
                    m.state = "failed".into();
                    m.turn = None;
                    m.failure = Some(format!(
                        "attachment_unavailable: {}",
                        excerpt(&error.to_string(), 1024)
                    ));
                }
                Ok(())
            })?;
            return Ok(true);
        }
    };
    let turn = state["turn"]["current"]
        .as_u64()
        .context("host state has no current turn")?
        + 1;
    let mut grounded = context(ctx, &message)?;
    let mut args = json!({ "entry": message.id, "input": input,
        "turn": turn, "text": message.text, "device": message.device,
        "via": format!("app:{}", message.device), "typed_at": message.typed_at, "context": "" });
    if !materialized.metadata.is_empty() {
        args["attachments"] = json!(materialized.metadata);
    }
    let base = serde_json::to_vec(&args)?.len();
    ensure!(
        base <= 62 * 1024,
        "owner input exceeds authenticated bridge frame"
    );
    bound_json_text(&mut grounded, 62 * 1024 - base);
    args["context"] = Value::String(grounded);
    // Persist before writing to the socket; if that write becomes uncertain, restart cannot
    // silently run a second turn. Same-host bridge keys settle a retry without double execution.
    let mut claimed = false;
    ctx.store.update_app_message(&message.id, |m| {
        if matches!(m.state.as_str(), "sent" | "waiting" | "late") {
            m.state = "delivered".into();
            m.turn = Some(turn);
            m.failure = None;
            claimed = true;
            m.generation = Some(runtime.generation);
        }
        Ok(())
    })?;
    if !claimed {
        return Ok(false);
    }
    let result = client.request("owner_turn", args);
    drop(materialized);
    let reply = match result {
        Ok(reply) => reply,
        Err(error) => {
            if let Some(refusal) = error.downcast_ref::<crate::agent::bridge::BridgeError>()
                && matches!(
                    refusal.code.as_str(),
                    "attachment_unsupported" | "bad_attachment"
                )
            {
                ctx.store.update_app_message(&message.id, |m| {
                    m.state = "failed".into();
                    m.turn = None;
                    m.failure = Some(format!(
                        "{}: {}",
                        refusal.code,
                        excerpt(&refusal.reason, 1024)
                    ));
                    Ok(())
                })?;
                return Ok(true);
            }
            return Err(error);
        }
    };
    ensure!(
        reply["disposition"] != "rejected",
        "assistant rejected owner turn"
    );
    runtime.active = Some(message.id.clone());
    ctx.store.update_app_message(&message.id, |m| {
        m.state = "working".into();
        Ok(())
    })?;
    ctx.store.set_app_thread_state_unless(
        &runtime.session,
        "working",
        None,
        &["paused", "ended"],
    )?;
    Ok(true)
}

fn event(
    ctx: &Ctx,
    runtime: &mut Runtime,
    event: &Value,
    update: &mut RuntimeUpdate,
) -> Result<()> {
    let kind = event["kind"].as_str().unwrap_or_default();
    let Some(id) = event["entry"].as_str() else {
        return Ok(());
    };
    let Ok(message) = ctx.store.app_message(id) else {
        return Ok(());
    };
    ensure!(
        message.session == runtime.session && message.input.as_deref() == event["input"].as_str(),
        "owner stream identity mismatch"
    );
    let turn = event["turn"].as_u64().context("owner event has no turn")?;
    ensure!(message.turn == Some(turn), "owner stream turn mismatch");
    match kind {
        "owner_delta" => {
            let offset = event["offset"]
                .as_u64()
                .context("owner delta has no offset")? as usize;
            let text = event["text"].as_str().context("owner delta has no text")?;
            let old = message.reply.as_deref().unwrap_or_default();
            if offset < old.len() {
                return Ok(());
            }
            ensure!(
                offset == old.len(),
                "owner stream gap; canonical transcript reconciliation required"
            );
            ensure!(
                old.len() + text.len() <= corral_core::assistant::threads::REPLY_MAX,
                "answer exceeds bounded turn capacity"
            );
            ctx.store.update_app_message(id, |m| {
                m.reply.get_or_insert_with(String::new).push_str(text);
                Ok(())
            })?;
            update.live.push(LiveText {
                entry: format!("{id}/answer/0"),
                turn,
                offset: offset as u64,
                text: text.into(),
                done: false,
            });
            runtime.active = Some(id.into());
            update.changed = true;
        }
        "owner_done" | "owner_error" => {
            let failure = event["error"]
                .as_str()
                .or_else(|| event["message"].as_str())
                .map(|s| excerpt(s, 2048))
                .or_else(|| (event["aborted"] == true).then(|| "Stopped by owner".into()));
            let completion = if event["aborted"] == true {
                TurnCompletion::Aborted
            } else if kind == "owner_error" || failure.is_some() {
                TurnCompletion::Failed
            } else {
                TurnCompletion::Completed
            };
            super::turns::settle(ctx, &message, completion)?;
            // A zero-text settlement still needs a canonical answer carrying its outcome.
            ctx.store.update_app_message(id, |m| {
                m.state = "done".into();
                m.reply.get_or_insert_with(String::new);
                m.failure = failure;
                m.model = event["model"].as_str().map(str::to_owned);
                Ok(())
            })?;
            runtime.active = None;
            ctx.store.set_app_thread_state_unless(
                &runtime.session,
                "ready",
                None,
                &["paused", "ended"],
            )?;
            update.live.push(LiveText {
                entry: format!("{id}/answer/0"),
                turn,
                offset: message.reply.as_ref().map_or(0, |s| s.len() as u64),
                text: String::new(),
                done: true,
            });
            update.changed = true;
        }
        _ => {}
    }
    Ok(())
}

/// One bounded bridge event page and at most one new turn. Never waits for model completion.
pub fn pump(ctx: &Ctx, runtime: &mut Runtime) -> Result<RuntimeUpdate> {
    // Try delivery before control, matching Pause without blocking the supervisor scan.
    let inbox = corral_core::inbox::Inbox::new(
        &ctx.store,
        corral_core::inbox::Recipient::Assistant {
            session: runtime.session.clone(),
        },
    )?;
    let Some(_delivery) = ctx.store.try_lock(&format!("deliver-{}", inbox.key()))? else {
        return Ok(RuntimeUpdate::default());
    };
    let _control = control(ctx, &runtime.session)?;
    let mut update = RuntimeUpdate::default();
    if runtime.exited {
        update.exited = true;
        return Ok(update);
    }
    #[cfg(unix)]
    if let Some(host) = &mut runtime.host
        && let Some(status) = host
            .child
            .try_wait()
            .context("reaping daemon assistant host")?
    {
        if !host.diagnostics_complete() && runtime.started.elapsed() < Duration::from_secs(30) {
            return Ok(update);
        }
        runtime.exited = true;
        update.exited = true;
        let head = ctx.store.app_thread_head(&runtime.session)?;
        if ctx.store.get_assistant(&runtime.session)?.ended.is_some() {
            ctx.store
                .set_app_thread_state(&runtime.session, "ended", None)?;
        } else if matches!(head.state.as_str(), "cold" | "takeover") {
            // Explicit profile replacement requested a clean restart, not a provider failure.
            ctx.store
                .set_app_thread_state(&runtime.session, "cold", None)?;
        } else {
            unavailable(
                ctx,
                &runtime.session,
                &format!("Assistant host exited: {status}\n{}", host.diagnostics()),
            )?;
        }
        super::super::ops::assistant::suspend(
            ctx,
            &runtime.session,
            runtime.generation,
            "daemon host exited",
        )?;
        return Ok(update);
    }
    let work = (|| -> Result<()> {
        if !connect(ctx, runtime)? {
            return Ok(());
        }
        let page = runtime
            .bridge
            .as_mut()
            .context("assistant bridge missing")?
            .events_since(runtime.cursor)?;
        ensure!(page.gap.is_none(), "assistant stream event history gap");
        for item in page.events {
            event(ctx, runtime, &item, &mut update)?;
        }
        runtime.cursor = page.next;
        if !page.more {
            update.changed |= deliver(ctx, runtime)?;
        }
        super::reconcile_pm(ctx, &runtime.session)?;
        Ok(())
    })();
    if let Err(error) = work {
        unavailable(ctx, &runtime.session, &format!("{error:#}"))?;
        return Err(error);
    }
    Ok(update)
}

pub fn stop(ctx: &Ctx, device: &AuthenticatedDevice, scope: &str, turn: u64) -> Result<StopAck> {
    super::input::recheck_authority(&ctx.store, &device.id, false)?;
    ensure!(device.assistant, "authority_assistant");
    let head = ctx.store.app_thread(scope)?;
    let _control = control(ctx, &head.session)?;
    let container = ctx.store.get_assistant_container(&head.session)?;
    let generation = container.state.binding.generation;
    ensure!(
        container
            .state
            .generation(generation)
            .is_some_and(|g| g.daemon_hosted),
        "not a daemon-hosted assistant"
    );
    let dir =
        launch::assistant_runtime_dir(&ctx.store, &head.session).join(format!("g{generation}"));
    let reply = BridgeClient::connect_dir(&dir, generation)?.cancel(Some(turn))?;
    Ok(stop_ack(&head.session, turn, &reply))
}

fn stop_ack(session: &str, turn: u64, reply: &Value) -> StopAck {
    let reason = reply["reason"].as_str();
    let outcome = if reply["cancelled"] == true && reply["turn"].as_u64() == Some(turn) {
        StopOutcome::Accepted
    } else if reply["cancelled"] == false
        && (reason == Some("idle")
            || (reason == Some("turn_not_current")
                && reply["turn"].as_u64().is_some_and(|current| current > turn)))
    {
        StopOutcome::AlreadyStopped
    } else {
        StopOutcome::Failed
    };
    StopAck {
        session: session.into(),
        turn,
        outcome,
        reason: reason
            .map(|reason| excerpt(reason, 1024))
            .or_else(|| (outcome == StopOutcome::Failed).then(|| "invalid_stop_response".into())),
    }
}

pub fn shutdown(ctx: &Ctx, runtime: &mut Runtime) -> Result<()> {
    let _control = control(ctx, &runtime.session)?;
    if let Some(client) = &mut runtime.bridge {
        client.shutdown()?;
    }
    ctx.store
        .set_app_thread_state_unless(&runtime.session, "stopping", None, &["paused", "ended"])
}
