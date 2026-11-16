use super::{act, change, detail, fleet, sessions, stamp};
use crate::model::{
    Id,
    act::Confirmable,
    daemon::*,
    devices::{DeviceOp, Pairing},
    effect::Effect,
};
use corral_protocol::{app::*, assistant::*, detail::*, rows::*};
use serde_json::{Value, json};

pub fn confirmation(feed: &Feed, subject: &Confirmable) -> Result<ActPreview, String> {
    let session = feed
        .session
        .as_ref()
        .ok_or("Preview device is not enrolled")?;
    let (class, descriptions) = match subject {
        Confirmable::Act(act) => {
            if act.command.app_class() == ActClass::Refused {
                return Err("This command is unavailable in preview".into());
            }
            (
                act.command.app_class(),
                act.command_rows()
                    .into_iter()
                    .map(|(label, value)| format!("{label}: {value}"))
                    .collect::<Vec<_>>(),
            )
        }
        Confirmable::Prompt { answer, .. } => {
            if !session.authority.assistant {
                return Err("Preview assistant authority is disabled".into());
            }
            (
                ActClass::Confirm,
                vec![format!(
                    "Answer permission prompt {} revision {} with {:?}",
                    answer.key, answer.rev, answer.choice
                )],
            )
        }
        Confirmable::DeviceAuthority { change, .. } => {
            if !session.authority.admin {
                return Err("Preview admin authority is disabled".into());
            }
            (
                ActClass::Admin,
                vec![format!(
                    "Change preview device {} authority: reply={}, act={}, assistant={}, assistant_act={}, admin={}",
                    change.device,
                    change.authority.reply,
                    change.authority.act,
                    change.authority.assistant,
                    change.authority.assistant_act,
                    change.authority.admin
                )],
            )
        }
    };
    let mut effects = vec![
        "PREVIEW ONLY · changes synthetic local records; no daemon or external side effects".into(),
    ];
    effects.extend(descriptions);
    Ok(ActPreview {
        preview_id: format!("preview:{}:{}", subject.key(), feed.changes_head),
        digest: format!(
            "preview-local:{}:{}",
            subject.key(),
            feed.authority_revision
        ),
        class,
        effects,
        expires_at: stamp(feed, feed.now.saturating_add(PREVIEW_TTL_SECS)),
        operation_token: Some(format!("preview-operation:{}", subject.key())),
    })
}

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
        Effect::Device(op) => {
            let key = match op {
                DeviceOp::Revoke { key, .. } | DeviceOp::SetAuthority { key, .. } => {
                    Some(key.clone())
                }
                _ => None,
            };
            if let Some(key) = key {
                operation(feed, &key, effect)
            } else {
                device(feed, op).map(|_| ())
            }
        }
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
                at: Some(stamp(feed, feed.now)),
            });
            feed.changes_unseen = Some((
                through,
                feed.changes
                    .values()
                    .filter(|line| line.seq > through)
                    .count() as u64,
            ));
            Ok(())
        }
        Effect::Recover(key) => {
            if feed.commands.contains_key(key) || feed.operations.contains_key(key) {
                Ok(())
            } else {
                Err("Preview has no command under that identity; nothing was retried".into())
            }
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
        return if let Some(error) = &operation.error {
            Err(error.message.clone())
        } else {
            Ok(())
        };
    }
    let method = match effect {
        Effect::Session { .. } => ASSISTANT_SESSION,
        Effect::Stop(_) => ASSISTANT_STOP,
        Effect::Warm(_) => ASSISTANT_WARM,
        Effect::Prompt { .. } => PROMPT_ANSWER,
        Effect::Device(DeviceOp::Revoke { .. }) => corral_protocol::device::DEVICE_REVOKE,
        Effect::Device(DeviceOp::SetAuthority { .. }) => DEVICE_SET_AUTHORITY,
        _ => "preview.local",
    };
    let result = execute_operation(feed, effect);
    feed.operations.insert(
        key.clone(),
        Operation {
            method: method.into(),
            loading: false,
            value: result.as_ref().ok().cloned(),
            error: result.as_ref().err().map(|message| ClientError {
                code: "preview_refused".into(),
                message: message.clone(),
            }),
        },
    );
    if let Effect::Prompt { answer, .. } = effect {
        let error = result.as_ref().err().map(|message| ClientError {
            code: "preview_refused".into(),
            message: message.clone(),
        });
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
                result: None,
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
    let assistant = if request.thread == "quick" {
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
    let authority = &feed
        .session
        .as_ref()
        .ok_or("Preview device is not enrolled")?
        .authority;
    match request.target {
        AskTarget::Assistant if !authority.assistant => {
            return Err("Preview assistant authority is disabled".into());
        }
        AskTarget::Pm if !authority.reply => {
            return Err("Preview reply authority is disabled".into());
        }
        AskTarget::Other => return Err("Unknown preview message recipient".into()),
        _ => {}
    }
    if request.target == AskTarget::Pm {
        sessions::append(feed, "pm", &request.text)?;
    }
    let id = format!("preview-message:{}", request.cmd_id);
    let at = stamp(feed, feed.now);
    let mut entry = sessions::entry(
        &id,
        &at,
        if request.target == AskTarget::Pm {
            EntryKind::PmMessage
        } else {
            EntryKind::Message
        },
        &request.text,
    );
    entry.typed_at = Some(request.typed_at.clone());
    entry.links = request.context.clone();
    entry.state = Some(if request.target == AskTarget::Pm {
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
    let scopes: Vec<String> = if request.thread == "quick" || request.thread == "A-7" {
        vec!["quick".into(), "A-7".into()]
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
            result: None,
            error: None,
            entry: Some(id.into()),
        },
    );
    feed.retire_echoed_sends();
    change(
        feed,
        None,
        "message recorded locally".into(),
        ChangeKind::Comment,
    );
    Ok(())
}

fn execute_operation(feed: &mut Feed, effect: &Effect) -> Result<Value, String> {
    match effect {
        Effect::Session { op, .. } => assistant_session(feed, op),
        Effect::Warm(request) => {
            require_thread(feed, &request.thread)?;
            let id = if request.thread == "quick" {
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
            let row = feed
                .inbox
                .get_mut(answer.key.as_str())
                .ok_or("No such local preview permission prompt")?;
            if row.kind != InboxKind::Prompt
                || row.question_rev != Some(answer.rev)
                || row.state != InboxState::Open
            {
                return Err("conflict: preview permission prompt changed".into());
            }
            row.state = InboxState::Cancelled;
            row.answerable = false;
            row.rev += 1;
            Ok(json!({"accepted": true, "preview": true}))
        }
        Effect::Device(op) => device(feed, op),
        _ => Err("This preview effect has no operation reply".into()),
    }
}

fn assistant_session(feed: &mut Feed, op: &AssistantSessionOp) -> Result<Value, String> {
    if !feed
        .session
        .as_ref()
        .is_some_and(|session| session.authority.assistant)
    {
        return Err("Preview assistant authority is disabled".into());
    }
    let id = match op {
        AssistantSessionOp::New { task, profile } => {
            if let Some(task) = task
                && feed.board.task(task).is_none()
            {
                return Err("Preview focus task does not exist".into());
            }
            let next = feed
                .assistants
                .keys()
                .filter_map(|id| {
                    id.strip_prefix("A-")
                        .and_then(|number| number.parse::<u64>().ok())
                })
                .max()
                .unwrap_or(18)
                + 1;
            let id = format!("A-{next}");
            sessions::assistant(
                feed,
                &id,
                "New session",
                task.as_deref(),
                profile.clone().or_else(|| Some("pi-sol xhigh".into())),
            );
            id
        }
        AssistantSessionOp::End { session }
        | AssistantSessionOp::Pause { session }
        | AssistantSessionOp::Resume { session }
        | AssistantSessionOp::TakeOver { session } => session.clone(),
        AssistantSessionOp::SetProfile { thread, .. } => {
            if thread == "quick" {
                "A-7".into()
            } else {
                thread.clone()
            }
        }
        _ => return Err("Unsupported preview assistant session operation".into()),
    };
    if let AssistantSessionOp::SetProfile { profile, .. } = op
        && !feed.settings.values().any(|settings| {
            settings
                .assistant_models
                .iter()
                .any(|choice| &choice.name == profile)
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
        _ => {}
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
            Ok(json!({"cancelled":true}))
        }
        DeviceOp::Forget => Err("Leave preview before forgetting a real browser credential".into()),
        DeviceOp::PushSubscribe { .. } | DeviceOp::PushUnsubscribe => {
            Err("Preview cannot register operating-system notifications".into())
        }
        DeviceOp::SetAuthority { change, .. } => {
            if !feed
                .session
                .as_ref()
                .is_some_and(|session| session.authority.admin)
            {
                return Err("Preview admin authority is disabled".into());
            }
            let row = feed
                .devices
                .iter_mut()
                .find(|row| row.id == change.device)
                .ok_or("Preview device not found")?;
            row.authority = Some(change.authority.clone());
            if let Some(session) = &mut feed.session
                && session.device.id == change.device
            {
                session.authority = change.authority.clone();
                session.device.authority = Some(change.authority.clone());
                feed.authority_revision += 1;
            }
            Ok(json!({"device": change.device, "authority":change.authority}))
        }
        DeviceOp::Revoke { device, .. } => {
            if !feed
                .session
                .as_ref()
                .is_some_and(|session| session.authority.admin)
            {
                return Err("Preview admin authority is disabled".into());
            }
            let at = stamp(feed, feed.now);
            let row = feed
                .devices
                .iter_mut()
                .find(|row| &row.id == device)
                .ok_or("Preview device not found")?;
            row.revoked = Some(at[..10].into());
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
