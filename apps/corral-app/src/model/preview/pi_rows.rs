//! What corrald serves from the sample's pi hosts: a session's transcript page, read on request
//! as corrald reads the file, a chat's thread projected from its transcript, and the rows that
//! follow each host (a chat's state and turn, a worker's pause and quiet time).

use corral_protocol::assistant::{EntryKind, MessageState, TurnCompletion};
use corral_protocol::detail::{DetailBody, DetailRef, SessionItem, SessionPage};
use corral_protocol::rows::{AssistantLocation, AssistantState, WaitReason};

use super::pi::{Role, resolve};
use super::put;
use crate::model::Id;
use crate::model::daemon::Feed;
/// The session page corrald reads from the host's transcript.
pub(super) fn page(feed: &mut Feed, scope: &str) -> bool {
    let Some(id) = resolve(feed, scope) else {
        return false;
    };
    let Some(host) = feed.hosts.get(id.as_str()) else {
        return false;
    };
    let (role, task, run) = match host.role {
        Role::Chat => ("assistant", None, None),
        Role::Worker => {
            let (task, run) = id.split_once('_').unwrap_or((id.as_str(), ""));
            ("worker", Some(task.to_owned()), Some(run.to_owned()))
        }
    };
    let body = DetailBody::Session(SessionPage {
        id: id.to_string(),
        role: role.into(),
        task,
        run,
        machine: Some("preview-workstation".into()),
        messages: host
            .transcript
            .iter()
            .map(|recorded| recorded.message.clone())
            .collect(),
        older: None,
        replyable: !host.gone,
        reply_refusal: host
            .gone
            .then(|| "session has ended; relaunch it before sending controls".into()),
    });
    let target = DetailRef::Session {
        id: scope.to_owned(),
    };
    put(feed, target.clone(), body);
    let reread = feed.live.get(scope).map(|live| live.reread);
    feed.settle_session(&target, reread);
    true
}

/// A chat's thread as corrald projects it from the transcript: the owner's messages and the
/// assistant's answers. The quick thread shows its current session's.
pub(super) fn thread(feed: &mut Feed, id: &str) {
    let Some(host) = feed.hosts.get(id).filter(|host| host.role == Role::Chat) else {
        return;
    };
    let mut entries = std::collections::BTreeMap::new();
    for (index, recorded) in host.transcript.iter().enumerate() {
        let message = &recorded.message;
        let owner = message.author == "user";
        if !owner && message.author != "assistant" {
            continue;
        }
        let text = message
            .items
            .iter()
            .filter_map(|item| match item {
                SessionItem::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() || text.starts_with("[ask_user answer") {
            continue;
        }
        let entry = recorded
            .entry
            .clone()
            .unwrap_or_else(|| format!("{id}/replay/{index:04}"));
        let at = message.at.clone().unwrap_or_default();
        let mut row = super::sessions::entry(
            &entry,
            &at,
            if owner {
                EntryKind::Message
            } else {
                EntryKind::Answer
            },
            &text,
        );
        row.device = owner.then(|| "preview-device".into());
        row.via = None;
        row.state = owner.then_some(MessageState::Delivered);
        row.model = message.model.clone();
        row.completion = (!owner).then_some(match message.stop_reason.as_deref() {
            Some("aborted") => TurnCompletion::Aborted,
            _ => TurnCompletion::Completed,
        });
        entries.insert(Id::from(entry), row);
    }
    let quick = crate::model::assistant::quick_id(feed).is_some_and(|quick| quick == id);
    let mut scopes = vec![Id::from(id)];
    if quick {
        scopes.push(Id::from(crate::model::assistant::QUICK));
    }
    for scope in scopes {
        let thread = feed.threads.entry(scope).or_default();
        thread.entries = entries.clone();
    }
    feed.retire_echoed_sends();
}

/// What the rows report follows the hosts: a chat's state and turn, a worker's pause and
/// whether it is quiet.
pub(super) fn settle(feed: &mut Feed, id: &str) {
    let Some(host) = feed.hosts.get(id) else {
        return;
    };
    let (paused, open, gone, turn) = (host.paused, host.busy(), host.gone, host.turn);
    let wait = host.pending.first().map(|question| WaitReason {
        code: "owner_question".into(),
        text: question.text.clone(),
        retry_at: None,
    });
    match host.role {
        Role::Chat => {
            let Some(row) = feed.assistants.get_mut(id) else {
                return;
            };
            if row.state != AssistantState::Ended {
                row.state = if gone {
                    AssistantState::Cold
                } else if paused {
                    AssistantState::Paused
                } else if wait.is_some() {
                    AssistantState::Waiting
                } else if open {
                    AssistantState::Working
                } else {
                    AssistantState::Ready
                };
                row.live_in = if gone {
                    AssistantLocation::None
                } else {
                    AssistantLocation::Daemon
                };
            }
            row.turn = (turn > 0).then_some(turn);
            row.wait = wait;
            row.rev += 1;
        }
        Role::Worker => {
            if gone {
                feed.paused.remove(id);
            } else {
                feed.paused.insert(Id::from(id), paused);
                super::pi_script::quiet(feed, id, !open);
            }
        }
    }
}

/// Every host's rows, as corrald reports them once it polled each host.
pub(super) fn settle_all(feed: &mut Feed) {
    let ids: Vec<Id> = feed.hosts.keys().cloned().collect();
    for id in ids {
        settle(feed, &id);
    }
}
