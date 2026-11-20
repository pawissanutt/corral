//! What the owner's actions on a sample build's parts change, answered as corral-pi answers them
//! through corrald: a child cancelled or given another model, a steer to a child, a job stopped
//! or sent to the background, the session's model and effort (a transcript row says each
//! change), a model's favorite mark, Compact (the plan it kept goes stale), the board's refresh,
//! the answers to a child's
//! question and to a proposed board plan, and the exit's outcome in the build record. Export is
//! refused with the host's reason. Each change reaches every watcher through the host's state.

use corral_protocol::detail::*;
use corral_protocol::session_host::{
    ChildAction, InputDisposition, JobAction, SessionActed, SessionActionDone, SessionCardAnswer,
    SessionChildAct, SessionJobAct, SessionJobActed, SessionModel, SessionModelSet, SessionSend,
    SessionSent,
};
use corral_protocol::session_state::{ExitPhase, InputOrigin};
use serde::Serialize;
use serde_json::{Value, json};

use super::build_ops::{answered, running, words};
use super::parts::Parts;
use super::pi::{Host, Recorded, input_entry, recorded};
use super::pi_rows::settle;
use super::stamp;
use crate::model::Id;
use crate::model::daemon::Feed;

/// Context left after Compact, in tokens.
const COMPACTED: u64 = 6_000;

fn reply(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

/// The running build host `session` addresses and its parts.
fn build<'f>(
    feed: &'f mut Feed,
    session: &str,
    absent: &str,
) -> Result<(Id, &'f mut Host), String> {
    let id = running(feed, session)?;
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .filter(|host| host.parts.is_some())
        .ok_or_else(|| absent.to_owned())?;
    Ok((id, host))
}

fn parts(host: &mut Host) -> Result<&mut Parts, String> {
    host.parts
        .as_deref_mut()
        .ok_or_else(|| "this session's host serves no parts".to_owned())
}

/// A message the host records: the owner's words to a child, a child's reply, a model change.
fn message(at: &str, author: &str, items: Vec<SessionItem>) -> SessionMessage {
    SessionMessage {
        id: String::new(),
        author: author.into(),
        at: Some(at.into()),
        items,
        model: None,
        stop_reason: None,
        usage: None,
        input: None,
        attachments: Vec::new(),
    }
}

fn owner_words(at: &str, words: &str, input: InputFacts) -> SessionMessage {
    SessionMessage {
        input: Some(input),
        ..message(at, "user", vec![SessionItem::Text { text: words.into() }])
    }
}

/// `session.child`: Cancel, or the model a child takes at its next turn. A child that ended
/// and a model the fleet has not approved are refused with the host's reason.
pub(super) fn child(feed: &mut Feed, act: &SessionChildAct) -> Result<Value, String> {
    let ms = feed.now.saturating_mul(1000);
    let at = stamp(feed.now);
    let (id, host) = build(feed, &act.id, "this session runs no child agents")?;
    let parts = parts(host)?;
    let child = parts
        .child_mut(&act.child)
        .ok_or_else(|| format!("no child {} in this session", act.child))?;
    let refused = |why: &str| SessionActed {
        ok: false,
        reason: Some(why.into()),
    };
    let done = SessionActed {
        ok: true,
        reason: None,
    };
    let (acted, cancelled) = match &act.action {
        _ if !child.live() => (refused("Child is not live"), false),
        ChildAction::Cancel => {
            child.detail.state = "cancelled".into();
            child.detail.ended_at = Some(ms);
            child.detail.activity = None;
            child.detail.question = None;
            child.detail.history.push_str(" · cancelled");
            (done, true)
        }
        ChildAction::SetModel { model } if !child.detail.models.contains(model) => {
            (refused("Model is not approved for this fleet"), false)
        }
        ChildAction::SetModel { model } => {
            child.detail.pending_model = (*model != child.detail.model).then(|| model.clone());
            (done, false)
        }
        ChildAction::Other => return Err("the host does not know that child action".into()),
    };
    if cancelled {
        parts.event(
            &at,
            "child_cancelled",
            json!({"child": act.child}).to_string(),
        );
        host.pending
            .retain(|card| card.child.as_deref() != Some(act.child.as_str()));
    }
    settle(feed, id.as_str());
    reply(acted)
}

/// `session.send` to a child: the steer reaches it at its next turn and its transcript holds
/// it; the parent records no message, so the acknowledgement says the host handled it.
pub(super) fn steer(feed: &mut Feed, child: &str, send: &SessionSend) -> Result<Value, String> {
    let at = stamp(feed.now);
    let entry = input_entry(&send.id, &send.cmd_id);
    let (id, host) = build(feed, &send.id, "this session runs no child agents")?;
    let found = parts(host)?
        .child_mut(child)
        .ok_or_else(|| format!("{child} is not a child agent of this session"))?;
    if !found.live() {
        return Err(format!("{child} is not live; it takes no messages"));
    }
    found.detail.steers.push(ChildSteer {
        at: Some(at.clone()),
        text: send.text.clone(),
    });
    let excess = found.detail.steers.len().saturating_sub(32);
    found.detail.steers.drain(..excess);
    let mut said = owner_words(
        &at,
        &send.text,
        InputFacts {
            id: Some(send.cmd_id.clone()),
            origin: InputOrigin::Owner,
            typed_at: send.typed_at.clone(),
            device: None,
            to: Some(child.into()),
        },
    );
    said.id = entry.clone();
    found.transcript.push(said);
    settle(feed, id.as_str());
    reply(SessionSent {
        session: id.to_string(),
        entry,
        disposition: InputDisposition::Handled,
        turn: None,
    })
}

/// `session.job`: Stop ends a running job, keeping its output; Background leaves it running
/// detached. A job that already ended says so.
pub(super) fn job(feed: &mut Feed, act: &SessionJobAct) -> Result<Value, String> {
    let at = stamp(feed.now);
    let (id, host) = build(feed, &act.id, "this session runs no background jobs")?;
    let parts = parts(host)?;
    let found = parts
        .jobs
        .iter_mut()
        .find(|found| found.job.id == act.job)
        .ok_or_else(|| format!("no job {} in this session", act.job))?;
    let acted = if found.job.state != "running" {
        SessionJobActed {
            state: found.job.state.clone(),
            reason: Some("already completed".into()),
        }
    } else {
        match act.action {
            JobAction::Stop => {
                found.newest.push_str("Stopped by the owner.\n");
                found.job.state = "exited".into();
                found.job.ended_at = Some(at.clone());
                found.job.actions.clear();
                found.job.output_bytes = (found.older.len() + found.newest.len()) as u64;
                let lines: Vec<&str> = found.newest.lines().collect();
                found.job.tail = lines[lines.len().saturating_sub(2)..]
                    .iter()
                    .map(|line| (*line).to_owned())
                    .collect();
            }
            JobAction::Background => found.job.actions.retain(|action| action != "background"),
            JobAction::Other => return Err("the host does not know that job action".into()),
        }
        SessionJobActed {
            state: found.job.state.clone(),
            reason: None,
        }
    };
    let action = serde_json::to_value(act.action).unwrap_or(Value::Null);
    let event = json!({"job": act.job, "action": action, "state": acted.state});
    parts.event(&at, "job_control", event.to_string());
    settle(feed, id.as_str());
    reply(acted)
}

/// `session.model`: the session's model and effort from its catalog. The transcript records
/// each change as the host's own row, and the composer reads the new model from the state.
pub(super) fn model(feed: &mut Feed, request: &SessionModel) -> Result<Value, String> {
    let at = stamp(feed.now);
    let id = running(feed, &request.id)?;
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .filter(|host| host.parts.is_some())
        .ok_or("this session does not change its model")?;
    let row = feed
        .assistants
        .get_mut(id.as_str())
        .ok_or("this session does not change its model")?;
    let known = parts(host)?
        .models
        .models
        .iter()
        .any(|choice| choice.id == request.model);
    let (provider, name) = request
        .model
        .split_once('/')
        .filter(|_| known)
        .ok_or_else(|| format!("{} is not in this session's model list", request.model))?;
    let levels = parts(host)?.levels(Some(&request.model));
    if let Some(asked) = request
        .effort
        .as_ref()
        .filter(|asked| !levels.contains(asked))
    {
        return Err(format!("{} has no effort level {asked}", request.model));
    }
    let effort = request
        .effort
        .clone()
        .or_else(|| row.effort.clone().filter(|kept| levels.contains(kept)))
        .or_else(|| levels.iter().find(|level| *level == "medium").cloned())
        .or_else(|| levels.first().cloned());
    let mut items = Vec::new();
    if row.model.as_deref() != Some(request.model.as_str()) {
        items.push(SessionItem::ModelChange {
            provider: provider.into(),
            model: name.into(),
        });
    }
    if let Some(level) = effort
        .as_ref()
        .filter(|level| row.effort.as_ref() != Some(*level))
    {
        items.push(SessionItem::ThinkingLevelChange {
            level: level.clone(),
        });
    }
    let changed = !items.is_empty();
    if changed {
        host.transcript.push(Recorded {
            entry: None,
            message: message(&at, "system", items),
        });
        let event = json!({"model": request.model, "effort": effort});
        parts(host)?.event(&at, "model_changed", event.to_string());
    }
    row.model = Some(request.model.clone());
    row.effort = effort.clone();
    if changed {
        recorded(feed, id.as_str());
    }
    settle(feed, id.as_str());
    reply(SessionModelSet {
        model: Some(request.model.clone()),
        effort,
    })
}

/// Compact: the context is summarized between turns; the plan the host kept goes stale until it
/// is written again.
pub(super) fn compact(
    feed: &mut Feed,
    id: &Id,
    instructions: Option<&str>,
) -> Result<Value, String> {
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .filter(|host| host.parts.is_some())
        .ok_or("this session's host does not compact its context")?;
    if host.busy() {
        return Err("a turn is running; compact once it ends".into());
    }
    let parts = parts(host)?;
    let before = parts.context_tokens;
    parts.context_tokens = COMPACTED.min(before);
    parts.plan.stale = parts.plan.plan.is_some();
    parts.event(
        &at,
        "compaction",
        json!({"tokens_before": before}).to_string(),
    );
    let summary = match instructions {
        Some(asked) => format!("Preview · sample summary of the session so far, as asked: {asked}"),
        None => "Preview · sample summary of the session so far: the guide is renamed, the \
                 compatibility note is in, one review finding waits on the redirect decision."
            .into(),
    };
    host.transcript.push(Recorded {
        entry: None,
        message: message(
            &at,
            "system",
            vec![SessionItem::Compaction {
                summary,
                tokens_before: before,
            }],
        ),
    });
    recorded(feed, id.as_str());
    settle(feed, id.as_str());
    reply(SessionActionDone {
        text: Some(format!(
            "Compacted session context ({before} tokens before compaction)"
        )),
    })
}

/// A model marked or unmarked as a favorite; the session's model is unchanged and the catalog,
/// re-served, carries the mark.
pub(super) fn favorite(
    feed: &mut Feed,
    id: &Id,
    model: &str,
    favorite: bool,
) -> Result<Value, String> {
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .filter(|host| host.parts.is_some())
        .ok_or("this session's host does not keep favorites")?;
    let parts = parts(host)?;
    let choice = parts
        .models
        .models
        .iter_mut()
        .find(|choice| choice.id == model)
        .ok_or_else(|| format!("{model} is not in this session's model list"))?;
    choice.favorite = favorite;
    let event = json!({"model": model, "favorite": favorite});
    parts.event(&at, "model_favorite", event.to_string());
    settle(feed, id.as_str());
    reply(SessionActionDone {
        text: Some(if favorite {
            format!("{model} is a favorite")
        } else {
            format!("{model} is no longer a favorite")
        }),
    })
}

/// `/board`: the host reads the build's board again.
pub(super) fn board_refresh(feed: &mut Feed, id: &Id) -> Result<Value, String> {
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .filter(|host| host.parts.is_some())
        .ok_or("this build has no board")?;
    let parts = parts(host)?;
    let tasks = parts.board.tasks.len();
    let blocked = parts
        .board
        .tasks
        .iter()
        .filter(|task| task.phase == "blocked")
        .count();
    parts.event(&at, "board_refreshed", json!({"tasks": tasks}).to_string());
    settle(feed, id.as_str());
    reply(SessionActionDone {
        text: Some(format!("Board refreshed: {tasks} tasks, {blocked} blocked")),
    })
}

/// The owner's answer to a child's question goes to the child, which carries on; the plan step
/// waiting on it is done and the record keeps the decision. Dismissing it lets the child carry
/// on without an answer.
pub(super) fn answer_child(
    feed: &mut Feed,
    id: &Id,
    index: usize,
    answer: &SessionCardAnswer,
) -> Result<Value, String> {
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    let card = host.pending[index].clone();
    let given = if answer.dismiss {
        None
    } else {
        Some(words(&card, answer)?.join(" · "))
    };
    host.pending.remove(index);
    let parts = parts(host)?;
    if let Some(child) = card
        .child
        .as_deref()
        .and_then(|child| parts.child_mut(child))
    {
        child.detail.question = None;
        child.detail.state = "running".into();
        child.detail.activity = Some("edit".into());
        let reply = match &given {
            Some(words) => {
                child.transcript.push(owner_words(
                    &at,
                    words,
                    InputFacts {
                        id: None,
                        origin: InputOrigin::Owner,
                        typed_at: None,
                        device: None,
                        to: card.child.clone(),
                    },
                ));
                format!("Preview · sample reply. Going with “{words}”; finishing the guide now.")
            }
            None => "Preview · sample reply. No answer came; keeping the old path for now.".into(),
        };
        let mut said = message(&at, "assistant", vec![SessionItem::Text { text: reply }]);
        said.model = Some(child.detail.model.clone());
        child.transcript.push(said);
    }
    if let Some(plan) = parts.plan.plan.as_mut() {
        for step in &mut plan.steps {
            if step.wait.as_deref() == Some(card.id.as_str()) {
                step.state = "done".into();
                step.wait = None;
            }
        }
        plan.revision += 1;
    }
    parts.record.decisions.push(BuildDecision {
        at: at.clone(),
        kind: "question".into(),
        text: card.title.clone(),
        answer: given.clone(),
    });
    parts.event(&at, "card_answered", json!({"card": card.id}).to_string());
    settle(feed, id.as_str());
    answered(if given.is_some() {
        "delivered"
    } else {
        "dismissed"
    })
}

/// The proposed board plan: File and start queues its first task, File only files them to the
/// backlog, Dismiss drops the proposal.
pub(super) fn answer_plan(
    feed: &mut Feed,
    id: &Id,
    index: usize,
    answer: &SessionCardAnswer,
) -> Result<Value, String> {
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    let chosen = if answer.dismiss {
        "dismiss".to_owned()
    } else {
        answer
            .answers
            .first()
            .and_then(|given| given.options.first())
            .cloned()
            .ok_or("choose File and start, File only or Dismiss")?
    };
    if !matches!(chosen.as_str(), "file_start" | "file" | "dismiss") {
        return Err(format!("{chosen} is not an option of this plan"));
    }
    host.pending.remove(index);
    let board = &mut parts(host)?.board;
    let proposal = board.proposed.take();
    let status = match (chosen.as_str(), proposal) {
        ("dismiss", _) | (_, None) => "dismissed",
        (start, Some(plan)) => {
            file(board, &plan, start == "file_start", &at);
            if start == "file_start" {
                "started"
            } else {
                "filed"
            }
        }
    };
    let parts = parts(host)?;
    parts.event(&at, "board_plan", json!({"choice": chosen}).to_string());
    settle(feed, id.as_str());
    answered(status)
}

/// Files `plan`'s tasks on the board, newest first, each named by the board's next id.
fn file(board: &mut BuildBoard, plan: &BoardPlan, start: bool, at: &str) {
    let mut next = board
        .tasks
        .iter()
        .filter_map(|task| task.id.strip_prefix("T-")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    let mut named: Vec<(String, String)> = Vec::new();
    for planned in &plan.tasks {
        next += 1;
        let id = format!("T-{next:03}");
        let depends: Vec<String> = planned
            .depends
            .iter()
            .filter_map(|key| named.iter().find(|(known, _)| known == key))
            .map(|(_, id)| id.clone())
            .collect();
        let phase = match (start, depends.is_empty()) {
            (true, true) => "queued",
            (true, false) => "waiting",
            (false, _) => "backlog",
        };
        named.push((planned.key.clone(), id.clone()));
        board.tasks.insert(
            0,
            BoardTask {
                id,
                title: planned.title.clone(),
                status: phase.into(),
                phase: phase.into(),
                depends,
                detail: None,
                updated_at: at.into(),
            },
        );
    }
}

/// The exit's summary and why Land needs a reason, from the record: open major findings.
pub(super) fn exit_facts(host: &Host) -> Option<(String, Option<String>)> {
    let record = &host.parts.as_deref()?.record;
    let open = record
        .reviews
        .iter()
        .flat_map(|review| &review.findings)
        .filter(|finding| {
            matches!(finding.severity.as_str(), "blocker" | "major") && finding.resolution.is_none()
        })
        .count();
    let summary = format!(
        "{} commits · {} files changed",
        record.commits_total,
        record.diff.as_ref().map_or(0, |diff| diff.files)
    );
    let needs = (open > 0).then(|| match open {
        1 => "1 major finding is open".to_owned(),
        n => format!("{n} major findings are open"),
    });
    Some((summary, needs))
}

/// The record follows the exit: a Keep or a Land with the owner's reason, each phase of the
/// Land on its timeline, and the landed commit.
pub(super) fn exited(host: &mut Host, at: &str, phase: ExitPhase, reason: Option<&str>) {
    let landed = host.exit.as_ref().and_then(|exit| exit.landed.clone());
    let Some(parts) = host.parts.as_deref_mut() else {
        return;
    };
    let outcome = &mut parts.record.outcome;
    if let Some(reason) = reason {
        outcome.override_reason = Some(reason.into());
    }
    let (state, step) = match phase {
        ExitPhase::Kept => ("kept", "keep"),
        ExitPhase::Landed => ("landed", "land"),
        ExitPhase::Rebasing => ("open", "rebase"),
        ExitPhase::Verifying => ("open", "verify"),
        ExitPhase::Landing => ("open", "land"),
        _ => return,
    };
    outcome.state = state.into();
    if phase == ExitPhase::Landed {
        outcome.at = Some(at.into());
        outcome.landed = landed;
    }
    parts.record.timeline.push(BuildTimeline {
        at: at.into(),
        phase: step.into(),
        detail: (phase == ExitPhase::Landed).then(|| "landed".into()),
    });
}
