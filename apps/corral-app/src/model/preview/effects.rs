//! Chat, device and subscription effects against the sample, on the unchanged chat wire.

use super::{act, detail, fleet, sessions, stamp};
use crate::model::{
    Id,
    daemon::*,
    devices::{DeviceOp, Pairing},
    effect::Effect,
};
use corral_protocol::{app::*, assistant::*, detail::*, rows::*};
use serde_json::{Value, json};

pub fn effect(feed: &mut Feed, effect: &Effect) -> Result<(), String> {
    match effect {
        Effect::Act(command) => act(feed, command),
        Effect::Detail(request) => detail(feed, &request.target),
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
        Effect::Session { key, .. } => operation(feed, key, effect),
        Effect::Stop(_) | Effect::Warm(_) => execute_operation(feed, effect).map(|_| ()),
        Effect::Prompt { answer, .. } => operation(feed, &Id::from(&answer.cmd_id), effect),
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
            if feed
                .detail(&DetailRef::Session {
                    id: session.to_string(),
                })
                .is_none()
            {
                return Err(format!("Unknown preview session {session}"));
            }
            if *on {
                feed.topics
                    .insert(format!("session:{session}"), TopicState::default());
                fleet::watch(feed, session.as_str());
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
        Effect::Stop(_) => ASSISTANT_STOP,
        Effect::Warm(_) => ASSISTANT_WARM,
        Effect::Prompt { .. } => PROMPT_ANSWER,
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
    if let Effect::Prompt { answer, .. } = effect {
        feed.commands.insert(
            answer.cmd_id.clone().into(),
            CommandState {
                device: Some("preview-device".into()),
                method: PROMPT_ANSWER.into(),
                request: serde_json::to_value(answer).map_err(|error| error.to_string())?,
                status: if result.is_ok() {
                    CommandStatus::Done
                } else {
                    CommandStatus::Refused
                },
                error,
                entry: None,
            },
        );
    }
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
    let assistant = if request.thread == QUICK_THREAD {
        "A-7"
    } else {
        &request.thread
    };
    if feed
        .assistants
        .get(assistant)
        .is_some_and(|row| row.state == AssistantState::Ended)
    {
        return Err("Preview assistant session has ended".into());
    }
    if request.target == AskTarget::Other {
        return Err("Unknown preview message recipient".into());
    }
    let pm = request.target == AskTarget::Pm;
    let id = format!("preview-message:{}", request.cmd_id);
    let at = stamp(feed.now);
    let mut entry = sessions::entry(
        &id,
        &at,
        if pm {
            EntryKind::PmMessage
        } else {
            EntryKind::Message
        },
        &request.text,
    );
    entry.typed_at = Some(request.typed_at.clone());
    entry.links = request.context.clone();
    entry.state = Some(if pm {
        MessageState::PmDelivered
    } else {
        MessageState::Delivered
    });
    let notice = sessions::entry(
        &format!("{id}:notice"),
        &at,
        EntryKind::Notice,
        "Preview · message recorded locally. No model ran and no reply was generated.",
    );
    let scopes: Vec<String> = if request.thread == QUICK_THREAD || request.thread == "A-7" {
        vec![QUICK_THREAD.into(), "A-7".into()]
    } else {
        vec![request.thread.clone()]
    };
    for scope in scopes {
        if let Some(thread) = feed.threads.get_mut(scope.as_str()) {
            thread.entries.insert(id.clone().into(), entry.clone());
            thread
                .entries
                .insert(notice.id.clone().into(), notice.clone());
        }
        feed.drafts.remove(scope.as_str());
    }
    feed.commands.insert(
        request.cmd_id.clone().into(),
        CommandState {
            device: Some("preview-device".into()),
            method: ASSISTANT_SEND.into(),
            request: serde_json::to_value(request).map_err(|error| error.to_string())?,
            status: CommandStatus::Done,
            error: None,
            entry: Some(id.into()),
        },
    );
    feed.retire_echoed_sends();
    Ok(())
}

fn execute_operation(feed: &mut Feed, effect: &Effect) -> Result<Value, String> {
    match effect {
        Effect::Session { op, .. } => assistant_session(feed, op),
        Effect::Warm(request) => {
            require_thread(feed, &request.thread)?;
            let id = if request.thread == QUICK_THREAD {
                "A-7"
            } else {
                &request.thread
            };
            let row = feed
                .assistants
                .get_mut(id)
                .ok_or("Preview assistant session does not exist")?;
            if row.state == AssistantState::Ended {
                return Err("Preview assistant session ended".into());
            }
            row.state = AssistantState::Ready;
            serde_json::to_value(row).map_err(|error| error.to_string())
        }
        Effect::Stop(request) => {
            let row = feed
                .assistants
                .get_mut(request.session.as_str())
                .ok_or("Preview assistant session does not exist")?;
            let running = row.turn == Some(request.turn) && row.state == AssistantState::Working;
            if running {
                row.state = AssistantState::Ready;
                row.turn = None;
                row.rev += 1;
            }
            serde_json::to_value(StopAck {
                session: request.session.clone(),
                turn: request.turn,
                outcome: if running {
                    StopOutcome::Accepted
                } else {
                    StopOutcome::AlreadyStopped
                },
                reason: Some("Preview · local turn only".into()),
            })
            .map_err(|error| error.to_string())
        }
        Effect::Prompt { answer, .. } => {
            let open = feed.threads.values().any(|thread| {
                thread.entries.values().any(|entry| {
                    matches!(&entry.card, Some(EntryCard::Prompt { key, rev, .. })
                        if *key == answer.key && *rev == answer.rev)
                })
            });
            if !open {
                return Err("No such local preview permission prompt".into());
            }
            Ok(json!({"accepted": true, "preview": true}))
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
            let next = feed
                .assistants
                .keys()
                .filter_map(|id| id.strip_prefix("A-").and_then(|number| number.parse::<u64>().ok()))
                .max()
                .unwrap_or(17)
                + 1;
            let id = format!("A-{next}");
            sessions::assistant(
                feed,
                &id,
                "New session",
                task.as_deref(),
                profile.clone().or_else(|| Some("omp-sol".into())),
            );
            id
        }
        AssistantSessionOp::End { session }
        | AssistantSessionOp::Pause { session }
        | AssistantSessionOp::Resume { session }
        | AssistantSessionOp::TakeOver { session } => session.clone(),
        AssistantSessionOp::SetProfile { thread, .. } => {
            if thread == QUICK_THREAD {
                "A-7".into()
            } else {
                thread.clone()
            }
        }
        AssistantSessionOp::Other => {
            return Err("Unsupported preview assistant session operation".into());
        }
    };
    if let AssistantSessionOp::SetProfile { profile, .. } = op
        && !feed.settings.as_ref().is_some_and(|settings| {
            settings.config["profiles"]
                .as_object()
                .is_some_and(|profiles| profiles.contains_key(profile))
        })
    {
        return Err("Preview profile does not exist".into());
    }
    let row = feed
        .assistants
        .get_mut(id.as_str())
        .ok_or("Preview assistant session does not exist")?;
    match op {
        AssistantSessionOp::End { .. } => {
            row.state = AssistantState::Ended;
            row.live_in = AssistantLocation::None;
            row.turn = None;
        }
        AssistantSessionOp::Pause { .. } => row.state = AssistantState::Paused,
        AssistantSessionOp::Resume { .. } => row.state = AssistantState::Ready,
        AssistantSessionOp::TakeOver { .. } => {
            row.live_in = AssistantLocation::Daemon;
            row.state = AssistantState::Ready;
        }
        AssistantSessionOp::SetProfile { profile, .. } => row.profile = Some(profile.clone()),
        AssistantSessionOp::New { .. } | AssistantSessionOp::Other => {}
    }
    row.rev += 1;
    serde_json::to_value(row).map_err(|error| error.to_string())
}

fn device(feed: &mut Feed, op: &DeviceOp) -> Result<Value, String> {
    match op {
        DeviceOp::PairBegin
        | DeviceOp::Pair { .. }
        | DeviceOp::PairConfirm { .. }
        | DeviceOp::Enrol { .. } => Err(
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
