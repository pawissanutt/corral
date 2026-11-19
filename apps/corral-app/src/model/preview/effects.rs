//! Chat, device and subscription effects against the sample, on the unchanged chat wire. A chat
//! message goes to the thread's pi host, as corrald submits it; the thread shows it once the
//! host recorded it.

use super::{act, detail, fleet, pi, pi_control, sessions, stamp};
use crate::model::{
    Id,
    daemon::*,
    devices::{DeviceOp, Pairing},
    effect::{Effect, QueueRequest},
};
use corral_protocol::{app::*, assistant::*, rows::*};
use corral_types::Origin;
use serde_json::{Value, json};

pub fn effect(feed: &mut Feed, effect: &Effect) -> Result<(), String> {
    match effect {
        Effect::Act(command) => act(feed, command),
        Effect::Detail(request) => detail(feed, &request.target, request.page.as_deref()),
        Effect::Draft { thread, text } => {
            require_thread(feed, thread)?;
            if text.is_empty() {
                feed.drafts.remove(thread.as_str());
            } else {
                feed.drafts.insert(thread.clone().into(), text.clone());
            }
            Ok(())
        }
        Effect::Send(request) => send(feed, request),
        Effect::Session { key, .. }
        | Effect::SessionSend { key, .. }
        | Effect::BuildStart { key, .. }
        | Effect::SessionCardAnswer { key, .. }
        | Effect::SessionQueue { key, .. }
        | Effect::SessionAction { key, .. }
        | Effect::BuildRemove { key, .. } => operation(feed, key, effect),
        Effect::Warm(_) => execute_operation(feed, effect).map(|_| ()),
        Effect::Device(op) => match op {
            DeviceOp::Revoke { key, .. } => operation(feed, key, effect),
            _ => device(feed, op).map(|_| ()),
        },
        Effect::Thread { scope, open } => {
            require_thread(feed, scope)?;
            if *open {
                feed.topics
                    .insert(format!("thread:{scope}"), TopicState::default());
            }
            Ok(())
        }
        Effect::Watch { session, on } => {
            if pi::resolve(feed, session).is_none() && !feed.agents.contains_key(session.as_str()) {
                return Err(format!("Unknown preview session {session}"));
            }
            // Several views can watch one scope and the sample does not count them, so a scope
            // once watched stays fed; a later watcher gets the running turn again.
            if *on {
                feed.topics
                    .insert(format!("session:{session}"), TopicState::default());
                pi::watch(feed, session.as_str());
                fleet::watch(feed, session.as_str());
                if let Some(host) = pi::resolve(feed, session) {
                    super::pi_rows::settle(feed, &host);
                }
            }
            Ok(())
        }
        Effect::Changes(query) => {
            let covered = query
                .before
                .is_none_or(|before| before <= feed.changes_head.saturating_add(1))
                && query.after.is_none_or(|after| after <= feed.changes_head);
            if !covered {
                return Err("Preview change cursor is beyond local history".into());
            }
            feed.changes_older = None;
            Ok(())
        }
        Effect::Seen(request) => {
            if request.through > feed.changes_head {
                return Err("Preview seen cursor is beyond local history".into());
            }
            let through = feed
                .seen
                .as_ref()
                .map_or(request.through, |seen| seen.through.max(request.through));
            feed.seen_received(SeenCursor {
                through,
                at: Some(stamp(feed.now)),
            });
            feed.changes_unseen = Some((through, feed.changes.range(through + 1..).count() as u64));
            Ok(())
        }
        Effect::RetryTopic(topic) => {
            let state = feed
                .topics
                .get_mut(topic)
                .ok_or_else(|| format!("Preview has no local topic {topic}"))?;
            *state = TopicState::default();
            Ok(())
        }
        Effect::CancelSend(key) => {
            if feed
                .commands
                .get(key)
                .is_some_and(|command| command.status == CommandStatus::Done)
                || feed.threads.values().any(|thread| {
                    thread
                        .entries
                        .values()
                        .any(|entry| entry.id == format!("preview-message:{key}"))
                })
            {
                feed.not_withdrawn.insert(key.clone());
                return Err("Preview message already delivered; it was not withdrawn".into());
            }
            feed.commands.remove(key);
            feed.cancelling.remove(key);
            Ok(())
        }
        Effect::SessionItem { session, token } => Err(format!(
            "Preview transcript {session} has no continuation {token}; all seeded items are complete"
        )),
        _ => Err(
            "Preview effect belongs to application navigation, not local data simulation".into(),
        ),
    }
}

/// Operation correlation is assigned by the application; preserve it instead of minting another ID.
pub fn operation(feed: &mut Feed, key: &Id, effect: &Effect) -> Result<(), String> {
    if let Some(operation) = feed.operations.get(key) {
        return match &operation.error {
            Some(error) => Err(error.message.clone()),
            None => Ok(()),
        };
    }
    let method = match effect {
        Effect::Session { .. } => ASSISTANT_SESSION,
        Effect::SessionSend { .. } => corral_protocol::session_host::SESSION_SEND,
        Effect::BuildStart { .. } => corral_protocol::build::BUILD_START,
        Effect::SessionCardAnswer { .. } => corral_protocol::session_host::SESSION_CARD_ANSWER,
        Effect::SessionQueue { request, .. } => request.method(),
        Effect::SessionAction { .. } => corral_protocol::session_host::SESSION_ACTION,
        Effect::BuildRemove { .. } => corral_protocol::build::BUILD_REMOVE,
        Effect::Warm(_) => ASSISTANT_WARM,
        Effect::Device(DeviceOp::Revoke { .. }) => corral_protocol::device::DEVICE_REVOKE,
        _ => "preview.local",
    };
    let result = execute_operation(feed, effect);
    let error = result.as_ref().err().map(|message| ClientError {
        code: "preview_refused".into(),
        message: message.clone(),
    });
    feed.operations.insert(
        key.clone(),
        Operation {
            method: method.into(),
            loading: false,
            value: result.as_ref().ok().cloned(),
            error: error.clone(),
            refusal: None,
        },
    );
    result.map(|_| ())
}

fn require_thread(feed: &Feed, scope: &str) -> Result<(), String> {
    if feed.threads.contains_key(scope) {
        Ok(())
    } else {
        Err(format!("Preview thread {scope} does not exist"))
    }
}

fn send(feed: &mut Feed, request: &AssistantSend) -> Result<(), String> {
    if feed.send_acceptance.get(request.cmd_id.as_str()) == Some(&SendAcceptance::Accepted) {
        return Ok(());
    }
    let result = send_inner(feed, request);
    let acceptance = match &result {
        Ok(()) => SendAcceptance::Accepted,
        Err(message) => SendAcceptance::Refused(ClientError {
            code: "preview_refused".into(),
            message: message.clone(),
        }),
    };
    feed.send_acceptance
        .insert(request.cmd_id.clone().into(), acceptance);
    result
}

fn send_inner(feed: &mut Feed, request: &AssistantSend) -> Result<(), String> {
    require_thread(feed, &request.thread)?;
    if request.text.trim().is_empty() {
        return Err("Preview message is empty".into());
    }
    if !request.attachments.is_empty() {
        return Err("Preview sends do not upload or deliver attachments; remove them first".into());
    }
    let id = chat_for(feed, &request.thread)?;
    let entry = pi::input_entry(&id, &request.cmd_id);
    feed.drafts.remove(request.thread.as_str());
    feed.commands.insert(
        request.cmd_id.clone().into(),
        CommandState {
            device: Some("preview-device".into()),
            method: ASSISTANT_SEND.into(),
            request: serde_json::to_value(request).map_err(|error| error.to_string())?,
            status: CommandStatus::Done,
            error: None,
            entry: Some(entry.clone().into()),
        },
    );
    // As corrald starts a stopped chat host before it submits the message.
    pi::revive(feed, &id);
    pi::deliver(feed, &id, &request.text, Origin::Owner, Some(entry));
    feed.retire_echoed_sends();
    Ok(())
}

/// The chat a message to `thread` reaches: a named session, or the quick thread's current one;
/// once the quick thread has ended, the next message starts a new session.
pub(super) fn chat_for(feed: &mut Feed, thread: &str) -> Result<String, String> {
    if thread != QUICK_THREAD {
        return match feed.assistants.get(thread) {
            Some(row) if row.state == AssistantState::Ended => {
                Err("chat thread ended; start a new session".into())
            }
            Some(_) => Ok(thread.to_owned()),
            None => Err("Preview assistant session does not exist".into()),
        };
    }
    let current = crate::model::assistant::quick_id(feed).filter(|id| {
        feed.assistants
            .get(id.as_str())
            .is_some_and(|row| row.state != AssistantState::Ended)
    });
    if let Some(id) = current {
        return Ok(id);
    }
    let profile = crate::model::assistant::quick_id(feed)
        .and_then(|id| feed.assistants.get(id.as_str()))
        .and_then(|row| row.profile.clone());
    let id = next_chat(feed);
    sessions::assistant(feed, &id, "Quick thread", None, profile, true);
    Ok(id)
}

fn next_chat(feed: &Feed) -> String {
    let next = feed
        .assistants
        .keys()
        .filter_map(|id| {
            id.strip_prefix("A-")
                .and_then(|number| number.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(17)
        + 1;
    format!("A-{next}")
}

fn execute_operation(feed: &mut Feed, effect: &Effect) -> Result<Value, String> {
    match effect {
        Effect::Session { op, .. } => assistant_session(feed, op),
        Effect::SessionSend { send, .. } => super::session_ops::send(feed, send),
        Effect::BuildStart { start, .. } => super::session_ops::start(feed, start),
        Effect::SessionCardAnswer { answer, .. } => super::build_ops::answer(feed, answer),
        Effect::SessionQueue { request, .. } => match request {
            QueueRequest::Withdraw(key) => super::build_ops::withdraw(feed, key),
            QueueRequest::Move(step) => super::build_ops::move_item(feed, step),
        },
        Effect::SessionAction { request, .. } => super::build_ops::action(feed, request),
        Effect::BuildRemove { remove, .. } => super::build_ops::remove(feed, remove),
        Effect::Warm(request) => {
            require_thread(feed, &request.thread)?;
            let id = pi::resolve(feed, &request.thread)
                .ok_or("Preview assistant session does not exist")?;
            if feed
                .assistants
                .get(id.as_str())
                .is_none_or(|row| row.state == AssistantState::Ended)
            {
                return Err("Preview assistant session ended".into());
            }
            pi::revive(feed, &id);
            let row = feed
                .assistants
                .get(id.as_str())
                .ok_or("Preview assistant session does not exist")?;
            serde_json::to_value(row).map_err(|error| error.to_string())
        }
        Effect::Device(op) => device(feed, op),
        _ => Err("This preview effect has no operation reply".into()),
    }
}

fn assistant_session(feed: &mut Feed, op: &AssistantSessionOp) -> Result<Value, String> {
    let id = match op {
        AssistantSessionOp::New { task, profile } => {
            if let Some(task) = task
                && super::record(feed, task).is_none()
            {
                return Err("Preview focus task does not exist".into());
            }
            let id = next_chat(feed);
            sessions::assistant(
                feed,
                &id,
                "New session",
                task.as_deref(),
                profile.clone().or_else(|| Some("omp-sol".into())),
                false,
            );
            id
        }
        AssistantSessionOp::End { session }
        | AssistantSessionOp::Pause { session }
        | AssistantSessionOp::Resume { session } => pi::resolve(feed, session)
            .map(|id| id.to_string())
            .ok_or("Preview assistant session does not exist")?,
        AssistantSessionOp::SetProfile { thread, .. } => pi::resolve(feed, thread)
            .map(|id| id.to_string())
            .ok_or("Preview assistant session does not exist")?,
        AssistantSessionOp::TakeOver { .. } | AssistantSessionOp::Other => {
            return Err(
                "invalid: chat has no take-over operation; use the same session directly".into(),
            );
        }
    };
    match op {
        AssistantSessionOp::SetProfile { profile, .. } => {
            let known = feed.settings.as_ref().is_some_and(|settings| {
                settings.config["profiles"]
                    .as_object()
                    .is_some_and(|profiles| profiles.contains_key(profile))
            });
            if !known {
                return Err("Preview profile does not exist".into());
            }
            if let Some(row) = feed.assistants.get_mut(id.as_str()) {
                row.profile = Some(profile.clone());
            }
        }
        AssistantSessionOp::End { .. } => {
            pi::end(feed, &id);
            if let Some(row) = feed.assistants.get_mut(id.as_str()) {
                row.state = AssistantState::Ended;
                row.live_in = AssistantLocation::None;
            }
        }
        AssistantSessionOp::Pause { .. } => {
            pi_control::control(feed, &id, crate::model::agents::Control::Pause)?;
        }
        AssistantSessionOp::Resume { .. } => {
            pi_control::control(feed, &id, crate::model::agents::Control::Resume)?;
        }
        _ => {}
    }
    let row = feed
        .assistants
        .get_mut(id.as_str())
        .ok_or("Preview assistant session does not exist")?;
    row.rev += 1;
    serde_json::to_value(row).map_err(|error| error.to_string())
}

fn device(feed: &mut Feed, op: &DeviceOp) -> Result<Value, String> {
    match op {
        // The pairing page's sample offer; no device can claim it, as no daemon is listening.
        DeviceOp::PairBegin => {
            let code = "corral-pair-preview-sample";
            feed.pairing = Pairing::Offered {
                pair_id: "preview-pair".into(),
                code: code.into(),
                url: Some(format!("http://127.0.0.1:8080/#pair={code}")),
                expires_at: feed.now + 600,
                reach: vec![("LAN".into(), "127.0.0.1:7778".into())],
            };
            Ok(json!({"pair_id": "preview-pair"}))
        }
        DeviceOp::Pair { .. } | DeviceOp::PairConfirm { .. } | DeviceOp::Enrol { .. } => Err(
            "Preview cannot pair or enrol real devices; leave preview to use Settings › Connection"
                .into(),
        ),
        DeviceOp::PairCancel { .. } => {
            feed.pairing = Pairing::Ended {
                line: "Preview · pairing cancelled locally".into(),
            };
            Ok(json!({"cancelled": true}))
        }
        DeviceOp::Forget => Err("Leave preview before forgetting a real browser credential".into()),
        DeviceOp::PushSubscribe { .. } | DeviceOp::PushUnsubscribe => {
            Err("Preview cannot register operating-system notifications".into())
        }
        DeviceOp::Revoke { device, .. } => {
            let at = stamp(feed.now);
            let row = feed
                .devices
                .iter_mut()
                .find(|row| &row.id == device)
                .ok_or("Preview device not found")?;
            row.revoked = Some(at.chars().take(10).collect());
            if feed
                .session
                .as_ref()
                .is_some_and(|session| &session.device.id == device)
            {
                feed.revoked(device.clone());
            }
            Ok(json!({"revoked": device}))
        }
    }
}
