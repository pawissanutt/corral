//! What corrald serves from the sample's pi hosts: a session's transcript page, read on request
//! as corrald reads the file (the quick scope with no current chat an empty page a send starts
//! one from), the rows that follow each host (a chat's or a build's state and turn, a worker's
//! pause and quiet time), and the host's state on the live session stream of every scope that
//! watches it.

use corral_protocol::detail::{DetailBody, DetailRef, HostPresence, SessionPage};
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
        if scope != crate::model::assistant::QUICK {
            return false;
        }
        // No quick chat is current: an empty page that takes a message, which starts one.
        let target = DetailRef::Session {
            id: scope.to_owned(),
        };
        let body = DetailBody::Session(crate::model::daemon::empty_quick_page());
        put(feed, target, body);
        return true;
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

/// What the rows report follows the hosts: a chat's or a build's state, turn, first waiting card
/// and exit, a worker's pause and whether it is quiet; and every watcher gets the host's state.
pub(super) fn settle(feed: &mut Feed, id: &str) {
    let Some(host) = feed.hosts.get(id) else {
        return;
    };
    let (paused, open, gone, turn) = (host.paused, host.busy(), host.gone, host.turn);
    let exit = host.exit.clone();
    let card = host.pending.first();
    let question_id = card.map(|card| card.id.clone());
    let wait = card.map(|card| WaitReason {
        code: "owner_question".into(),
        text: card.title.clone(),
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
            if let Some(build) = row.build.as_mut() {
                build.exit = exit;
                if let Some(parts) = host.parts.as_deref() {
                    build.children = parts.children_counts();
                    build.diff = parts.record.diff.clone();
                }
            }
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

/// What the owner may do with queued item `n`: withdraw an owner item, and move it among the
/// owner's items.
fn queue_actions(host: &Host, n: usize) -> Vec<String> {
    let owner = |input: &super::pi::Input| input.origin == Origin::Owner;
    if !host.queued.get(n).is_some_and(owner) {
        return Vec::new();
    }
    let mut actions = vec!["withdraw".to_owned()];
    if host.queued[..n].iter().any(owner) {
        actions.push("move_up".into());
    }
    if host.queued[n + 1..].iter().any(owner) {
        actions.push("move_down".into());
    }
    actions
}

/// What a build's host serves beyond what every host does.
const BUILD_CAN: &[&str] = &[
    "model",
    "model_favorite",
    "children",
    "child_control",
    "jobs",
    "job_control",
    "changes",
    "usage",
    "plan",
    "context",
    "commands",
    "plan_mode",
    "plan_mode_doc",
    "board",
    "record",
    "exit",
    "compact",
    "export",
];

/// The state host `id` reports: what it runs, holds and asks, and what it serves. The sample
/// serves sending, steering, follow-ups, its queue and its cards, a worker's rewinds, and every
/// part of a build, whose counts and briefs come from its parts; attachments stay listed with
/// why the preview cannot take them.
fn state(feed: &Feed, id: &str, host: &Host) -> SessionState {
    let row = feed.assistants.get(id);
    let model = row
        .and_then(|row| row.model.clone())
        .or_else(|| feed.agents.get(id).and_then(|row| row.run.model.clone()));
    let mut can: Vec<(&str, Option<String>)> = ["send", "steer", "follow_up", "queue", "cards"]
        .into_iter()
        .map(|id| (id, None))
        .collect();
    match host.role {
        Role::Worker => can.push(("rewind", None)),
        Role::Build => {
            can.extend(BUILD_CAN.iter().map(|id| (*id, None)));
            can.push((
                "attachments",
                Some("the preview does not upload attachments".into()),
            ));
        }
        Role::Chat => {}
    }
    let mut state = SessionState {
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
                    .key()
                    .map_or_else(|| format!("{id}/queued/{n}"), str::to_owned),
                policy: SendPolicy::FollowUp,
                origin: match input.origin {
                    Origin::Owner => InputOrigin::Owner,
                    Origin::Auto => InputOrigin::Auto,
                },
                text: prefix(&input.text),
                bytes: u64::try_from(input.text.len()).unwrap_or(u64::MAX),
                to: None,
                actions: queue_actions(host, n),
            })
            .collect(),
        queue_total: u32::try_from(host.queued.len()).unwrap_or(u32::MAX),
        cards: host
            .pending
            .iter()
            .map(|card| CardBrief {
                id: card.id.clone(),
                kind: card.kind,
                title: card.title.clone(),
                child: card.child.clone(),
                expires_at: card.expires_at.clone(),
            })
            .collect(),
        exit: host.exit.clone(),
        can: can
            .into_iter()
            .map(|(id, why)| Capability {
                id: id.into(),
                unavailable: if host.gone {
                    Some("the host is not running; resume it first".into())
                } else {
                    why
                },
            })
            .collect(),
        ..SessionState::default()
    };
    if let Some(parts) = host.parts.as_deref() {
        parts.brief(&mut state);
    }
    state
}

/// Every scope that watches host `id` gets its current state, as corrald's next page carries it,
/// and every part it serves that was read is read again, as the app's re-read on the host's
/// `live_part` would.
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
    super::parts::reread(feed, id);
}

/// Every host's rows, as corrald reports them once it polled each host.
pub(super) fn settle_all(feed: &mut Feed) {
    let ids: Vec<Id> = feed.hosts.keys().cloned().collect();
    for id in ids {
        settle(feed, &id);
    }
}
