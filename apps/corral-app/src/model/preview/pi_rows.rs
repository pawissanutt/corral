//! What corrald serves from the sample's pi hosts: a session's transcript page, read on request
//! as corrald reads the file, a chat's thread projected from its transcript, the rows that
//! follow each host (a chat's or a build's state and turn, a worker's pause and quiet time), and
//! the host's state on the live session stream of every scope that watches it.

use corral_protocol::assistant::{EntryKind, MessageState, TurnCompletion};
use corral_protocol::detail::{
    CardKind, DetailBody, DetailRef, HostPresence, SessionItem, SessionPage,
};
use corral_protocol::rows::{AssistantLocation, AssistantState, WaitReason};
use corral_protocol::session_state::{
    Activity, Capability, CardBrief, InputOrigin, QueuedInput, SendPolicy, SessionMode,
    SessionState, UsageBrief,
};
use corral_types::Origin;

use super::pi::{Host, Role, resolve, scopes};
use super::{put, stamp};
use crate::model::Id;
use crate::model::daemon::Feed;
/// The session page corrald reads from the host's transcript. A message recorded from a socket
/// input is served under its input id; any other under its entry.
pub(super) fn page(feed: &mut Feed, scope: &str) -> bool {
    let Some(id) = resolve(feed, scope) else {
        return false;
    };
    let Some(host) = feed.hosts.get(id.as_str()) else {
        return false;
    };
    let (role, task, run) = match host.role {
        Role::Chat => ("assistant", None, None),
        Role::Build => ("build", None, None),
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
            .enumerate()
            .map(|(n, recorded)| {
                let mut message = recorded.message.clone();
                if message.id.is_empty() {
                    message.id = format!("{id}/entry/{n}");
                }
                message
            })
            .collect(),
        older: None,
        replyable: !host.gone,
        reply_refusal: host
            .gone
            .then(|| "session has ended; relaunch it before sending controls".into()),
        host: if host.gone {
            HostPresence::NotRunning
        } else {
            HostPresence::Running
        },
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

/// What the rows report follows the hosts: a chat's or a build's state, turn and pending
/// question, a worker's pause and whether it is quiet; and every watcher gets the host's state.
pub(super) fn settle(feed: &mut Feed, id: &str) {
    let Some(host) = feed.hosts.get(id) else {
        return;
    };
    let (paused, open, gone, turn) = (host.paused, host.busy(), host.gone, host.turn);
    let question = host.pending.first();
    let question_id = question.map(|question| question.id.clone());
    let wait = question.map(|question| WaitReason {
        code: "owner_question".into(),
        text: question.text.clone(),
        retry_at: None,
    });
    match host.role {
        Role::Chat | Role::Build => {
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
            row.question_id = question_id;
            row.updated = Some(stamp(feed.now));
            row.rev += 1;
        }
        Role::Worker => {
            if let Some(row) = feed.agents.get_mut(id) {
                row.paused = (!gone).then_some(paused);
            }
            if !gone {
                super::pi_script::quiet(feed, id, !open);
            }
        }
    }
    publish_state(feed, id);
}

/// The first 200 bytes of `text`, cut at a character boundary, as the host lists queue items.
fn prefix(text: &str) -> String {
    let mut end = text.len().min(200);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The state host `id` reports: what it runs, holds and asks, and what it serves. The sample
/// serves sending, steering and follow-ups, a worker's rewinds and a build's child agents.
fn state(feed: &Feed, id: &str, host: &Host) -> SessionState {
    let row = feed.assistants.get(id);
    let model = row
        .and_then(|row| row.model.clone())
        .or_else(|| feed.agents.get(id).and_then(|row| row.run.model.clone()));
    let mut can = vec!["send", "steer", "follow_up"];
    match host.role {
        Role::Worker => can.push("rewind"),
        Role::Build => can.push("children"),
        Role::Chat => {}
    }
    SessionState {
        mode: SessionMode::Headless,
        turn: host.turn,
        open: host.busy(),
        paused: host.paused,
        activity: host.busy().then(|| Activity {
            phase: "writing".into(),
            since: stamp(feed.now),
        }),
        model,
        effort: row.and_then(|row| row.effort.clone()),
        usage: UsageBrief {
            cost_usd: row.and_then(|row| row.cost_usd),
            ..UsageBrief::default()
        },
        queue: host
            .queued
            .iter()
            .enumerate()
            .map(|(n, input)| QueuedInput {
                key: input
                    .entry
                    .as_deref()
                    .and_then(|entry| entry.split_once("/input/"))
                    .map_or_else(|| format!("{id}/queued/{n}"), |(_, key)| key.to_owned()),
                policy: SendPolicy::FollowUp,
                origin: match input.origin {
                    Origin::Owner => InputOrigin::Owner,
                    Origin::Auto => InputOrigin::Auto,
                },
                text: prefix(&input.text),
                bytes: u64::try_from(input.text.len()).unwrap_or(u64::MAX),
                to: None,
                actions: Vec::new(),
            })
            .collect(),
        queue_total: u32::try_from(host.queued.len()).unwrap_or(u32::MAX),
        cards: host
            .pending
            .iter()
            .map(|question| CardBrief {
                id: question.id.clone(),
                kind: CardKind::Question,
                title: question.text.clone(),
                child: None,
                expires_at: None,
            })
            .collect(),
        can: can
            .into_iter()
            .map(|id| Capability {
                id: id.into(),
                unavailable: host
                    .gone
                    .then(|| "the host is not running; resume it first".into()),
            })
            .collect(),
        ..SessionState::default()
    }
}

/// Every scope that watches host `id` gets its current state, as corrald's next page carries it.
fn publish_state(feed: &mut Feed, id: &str) {
    let Some(host) = feed.hosts.get(id) else {
        return;
    };
    let current = state(feed, id, host);
    for scope in scopes(feed, id) {
        feed.live
            .entry(scope)
            .or_default()
            .replace_state(Some(Box::new(current.clone())));
    }
}

/// Every host's rows, as corrald reports them once it polled each host.
pub(super) fn settle_all(feed: &mut Feed) {
    let ids: Vec<Id> = feed.hosts.keys().cloned().collect();
    for id in ids {
        settle(feed, &id);
    }
}
