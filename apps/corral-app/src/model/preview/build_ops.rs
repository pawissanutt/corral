//! The cards a sample pi host waits on, its queue, its session actions, and a build's exit and
//! removal, answered as corral-pi and corrald answer them: a question card takes one answer per
//! question and delivers them as the next turn's input; a child's question and a proposed board
//! plan are answered on the build's parts (`parts_ops.rs`); the exit choice (Finish) checks the
//! option's availability and Land's reason, then walks rebase, gate and land one phase every
//! other clock tick, reporting its steps and the gate's log tail as the host does and the build
//! record following it; Compact and the board's refresh run, Export
//! is refused with the host's reason; the queue withdraws an owner's item with its whole text and
//! moves it among the owner's items; and a stopped build's removal reports what prune removes,
//! refusing while its host runs.

use corral_protocol::build::{BuildRemove, BuildRemoved};
use corral_protocol::detail::{CardKind, CardOption, CardQuestion, SessionCard};
use corral_protocol::rows::AssistantKind;
use corral_protocol::session_host::{
    QueueMoved, QueueWithdrawn, SessionAction, SessionActionDone, SessionActionRequest,
    SessionCardAnswer, SessionCardAnswered, SessionQueueKey, SessionQueueMove, WithdrawOutcome,
};
use corral_protocol::session_state::{
    ExitBrief, ExitChoice, ExitPhase, LandBrief, LandGate, LandStep, LandStepId, LandStepState,
};
use corral_types::Origin;
use serde::Serialize;
use serde_json::{Value, json};

use super::parts_ops;
use super::pi::{deliver, question_card, resolve};
use super::pi_rows::settle;
use super::sample::sha;
use super::stamp;
use crate::model::Id;
use crate::model::daemon::Feed;

fn reply(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

pub(super) fn answered(status: &str) -> Result<Value, String> {
    reply(SessionCardAnswered {
        status: status.into(),
    })
}

/// The running host `session` addresses, as corrald resolves and refuses it.
pub(super) fn running(feed: &Feed, session: &str) -> Result<Id, String> {
    let id = resolve(feed, session).ok_or_else(|| format!("{session} has no Pi host"))?;
    match feed.hosts.get(id.as_str()) {
        Some(host) if !host.gone => Ok(id),
        _ => Err("no running Pi host; resume the session first".into()),
    }
}

fn option(id: &str, label: &str, detail: &str) -> CardOption {
    CardOption {
        id: id.into(),
        label: label.into(),
        detail: Some(detail.into()),
        unavailable: None,
        needs_text: None,
        destructive: false,
    }
}

/// The sample build's own question to the owner: two choices for the guide, each with options
/// and words.
pub(super) fn api_question(at: &str) -> SessionCard {
    let question = |header: &str, text: &str, options: Vec<CardOption>| CardQuestion {
        header: Some(header.into()),
        text: text.into(),
        options,
        initial: None,
        multi: false,
        free_text: true,
    };
    SessionCard {
        id: "q-b1".into(),
        kind: CardKind::Question,
        title: "Preview · sample question: two choices for the guide".into(),
        meta: None,
        body: None,
        body_more: false,
        questions: vec![
            question(
                "Label",
                "Which label should the guide's examples use?",
                vec![
                    option("parity", "The parity label", "new projects see one name"),
                    option(
                        "both",
                        "Both labels",
                        "the old name in brackets for one release",
                    ),
                ],
            ),
            question(
                "Notes",
                "Where should the migration notes live?",
                vec![
                    option(
                        "migrations",
                        "docs/migrations/",
                        "beside the other upgrade notes",
                    ),
                    option("guide", "In the guide", "one page to read"),
                ],
            ),
        ],
        child: None,
        tool: Some("ask_user".into()),
        opened_at: at.into(),
        expires_at: None,
    }
}

/// Answers card `answer.card`: a question delivers its answers, the exit choice starts it; a
/// card no longer waiting is `missing`.
pub(super) fn answer(feed: &mut Feed, answer: &SessionCardAnswer) -> Result<Value, String> {
    let id = running(feed, &answer.id)?;
    let Some((index, kind)) = feed.hosts.get(id.as_str()).and_then(|host| {
        host.pending
            .iter()
            .position(|card| card.id == answer.card)
            .map(|index| (index, host.pending[index].kind))
    }) else {
        return answered("missing");
    };
    match kind {
        CardKind::Question => question(feed, &id, index, answer),
        CardKind::Exit => exit(feed, &id, index, answer),
        CardKind::ChildQuestion => parts_ops::answer_child(feed, &id, index, answer),
        CardKind::BoardPlan => parts_ops::answer_plan(feed, &id, index, answer),
        _ => {
            if let Some(host) = feed.hosts.get_mut(id.as_str()) {
                host.pending.remove(index);
            }
            settle(feed, &id);
            answered(if answer.dismiss {
                "dismissed"
            } else {
                "answered"
            })
        }
    }
}

/// The words each answer gives, by its options' labels and the owner's text.
pub(super) fn words(card: &SessionCard, answer: &SessionCardAnswer) -> Result<Vec<String>, String> {
    if answer.answers.len() != card.questions.len() {
        return Err(format!(
            "{} asks {} questions; answer each once",
            card.id,
            card.questions.len()
        ));
    }
    card.questions
        .iter()
        .zip(&answer.answers)
        .map(|(question, given)| {
            let mut parts: Vec<String> = given
                .options
                .iter()
                .filter_map(|id| question.options.iter().find(|option| option.id == *id))
                .map(|option| option.label.clone())
                .collect();
            parts.extend(
                given
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned),
            );
            if parts.is_empty() {
                return Err(format!("“{}” has no answer", question.text));
            }
            Ok(parts.join(" — "))
        })
        .collect()
}

fn question(
    feed: &mut Feed,
    id: &Id,
    index: usize,
    answer: &SessionCardAnswer,
) -> Result<Value, String> {
    if answer.dismiss {
        return answered("dismissed");
    }
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    let card = host.pending[index].clone();
    let words = words(&card, answer)?;
    host.pending.remove(index);
    let given: Vec<Value> = answer
        .answers
        .iter()
        .map(|given| json!({"options": given.options, "text": given.text}))
        .collect();
    host.transcript
        .push(question_card(&at, &card, "answered", Value::Array(given)));
    deliver(
        feed,
        id.as_str(),
        &format!("[ask_user answer {}] {}", card.id, words.join(" · ")),
        Origin::Owner,
        None,
    );
    answered("delivered")
}

fn brief(phase: ExitPhase, choice: Option<ExitChoice>, base: Option<String>) -> ExitBrief {
    ExitBrief {
        phase,
        detail: None,
        choice,
        landed: None,
        base_branch: base,
        repair: None,
        land: None,
    }
}

fn land_step(id: LandStepId, state: LandStepState, at: Option<&str>) -> LandStep {
    LandStep {
        id,
        state,
        note: None,
        started_at: at.map(str::to_owned),
        ended_at: None,
    }
}

/// Land's round as the sample host reports it once Land starts: the rebase running.
fn land_round(at: &str) -> LandBrief {
    LandBrief {
        steps: vec![
            land_step(LandStepId::Rebase, LandStepState::Running, Some(at)),
            land_step(LandStepId::Gate, LandStepState::Waiting, None),
            land_step(LandStepId::Land, LandStepState::Waiting, None),
        ],
        gate: None,
    }
}

/// The exit walked into its phase at `at`: the step that ends takes its end and note, the next
/// one starts. The gate (`attempt`) shows a few log lines while it runs and none once it passed.
fn walk(exit: &mut ExitBrief, at: &str, session: &str, attempt: &str) {
    let phase = exit.phase;
    let landed = exit.landed.clone().unwrap_or_default();
    let Some(land) = exit.land.as_mut() else {
        return;
    };
    let (ended, note, started) = match phase {
        ExitPhase::Verifying => (
            LandStepId::Rebase,
            sha(&format!("{session}/rebased"), 7),
            Some(LandStepId::Gate),
        ),
        ExitPhase::Landing => (
            LandStepId::Gate,
            format!("{attempt} passed"),
            Some(LandStepId::Land),
        ),
        ExitPhase::Landed => (LandStepId::Land, landed.chars().take(7).collect(), None),
        _ => return,
    };
    for step in &mut land.steps {
        if step.id == ended {
            step.state = LandStepState::Done;
            step.ended_at = Some(at.into());
            step.note = Some(note.clone());
        } else if Some(step.id) == started {
            step.state = LandStepState::Running;
            step.started_at = Some(at.into());
        }
    }
    land.gate = match phase {
        ExitPhase::Verifying => Some(LandGate {
            attempt: Some(attempt.into()),
            log: Some(format!("/var/tmp/corral/{session}/gate-{attempt}.log")),
            command: Some("scripts/gate.sh".into()),
            index: Some(1),
            of: Some(1),
            tail: [
                "+ cargo fmt --check",
                "+ cargo clippy --workspace --all-targets -- -D warnings",
                "    Checking corral-protocol v0.1.0",
                "+ cargo test --workspace",
                "     Running unittests src/lib.rs",
            ]
            .map(str::to_owned)
            .to_vec(),
        }),
        _ => land.gate.take().map(|gate| LandGate {
            command: None,
            index: None,
            of: None,
            tail: Vec::new(),
            ..gate
        }),
    };
}

fn base(feed: &Feed, id: &str) -> Option<String> {
    feed.assistants
        .get(id)
        .and_then(|row| row.build.as_ref())
        .and_then(|build| build.base.clone())
}

/// The exit choice: an unavailable option and a Land without its reason are refused before
/// anything runs; Land starts the rebase, Keep keeps the branch, Iterate (and Not now's
/// dismissal) goes back to the session.
fn exit(
    feed: &mut Feed,
    id: &Id,
    index: usize,
    answer: &SessionCardAnswer,
) -> Result<Value, String> {
    let base = base(feed, id.as_str());
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    let card = host.pending[index].clone();
    let chosen = if answer.dismiss {
        "iterate"
    } else {
        answer
            .answers
            .first()
            .and_then(|given| given.options.first())
            .map(String::as_str)
            .ok_or("choose keep, land or iterate")?
    };
    let option = card
        .questions
        .first()
        .and_then(|question| question.options.iter().find(|option| option.id == chosen))
        .ok_or_else(|| format!("{chosen} is not an exit option"))?;
    if let Some(why) = &option.unavailable {
        return Err(format!("{}: {why}", option.label));
    }
    let reason = answer
        .answers
        .first()
        .and_then(|given| given.text.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if let (Some(why), None) = (&option.needs_text, reason) {
        return Err(format!("{} needs your reason: {why}", option.label));
    }
    host.pending.remove(index);
    host.exit = match chosen {
        "land" => Some(ExitBrief {
            land: Some(land_round(&at)),
            ..brief(ExitPhase::Rebasing, Some(ExitChoice::Land), base)
        }),
        "keep" => Some(brief(ExitPhase::Kept, Some(ExitChoice::Keep), base)),
        _ => None,
    };
    if let Some(phase) = host.exit.as_ref().map(|exit| exit.phase) {
        parts_ops::exited(host, &at, phase, reason);
    }
    settle(feed, id.as_str());
    answered(if chosen == "land" {
        "started"
    } else {
        "answered"
    })
}

/// The build's exit choice, as `build_finish` docks it.
fn exit_card(feed: &Feed, id: &str, at: &str) -> Result<SessionCard, String> {
    let build = feed
        .assistants
        .get(id)
        .filter(|row| row.kind == AssistantKind::Build)
        .and_then(|row| row.build.as_ref())
        .ok_or("only a build session has an exit")?;
    let base = build.base.clone().unwrap_or_else(|| "main".into());
    let branch = build.branch.clone().unwrap_or_else(|| "its branch".into());
    let (summary, needs) = feed
        .hosts
        .get(id)
        .and_then(parts_ops::exit_facts)
        .unwrap_or_else(|| ("no commits yet".into(), None));
    let mut land = option(
        "land",
        &format!("Land on {base}"),
        &format!("rebase, full gate, fast-forward {base}"),
    );
    land.needs_text = needs;
    land.destructive = true;
    let title = format!("How should {} leave this session?", build.name);
    Ok(SessionCard {
        id: format!("exit-{id}"),
        kind: CardKind::Exit,
        title: title.clone(),
        meta: Some("landing needs an approving review and the full gate".into()),
        body: Some(format!("Preview · sample summary: {summary} on {branch}")),
        body_more: false,
        questions: vec![CardQuestion {
            header: None,
            text: title,
            options: vec![
                option(
                    "keep",
                    "Keep on the branch",
                    &format!("the branch stays in {}", build.worktree),
                ),
                land,
                option("iterate", "Keep iterating", "back to the session"),
            ],
            initial: None,
            multi: false,
            free_text: false,
        }],
        child: None,
        tool: None,
        opened_at: at.into(),
        expires_at: None,
    })
}

/// A session action: Finish docks the exit choice; Compact and the board's refresh run on the
/// build's parts; Export and a shell command are refused with the host's reason.
pub(super) fn action(feed: &mut Feed, request: &SessionActionRequest) -> Result<Value, String> {
    let id = running(feed, &request.id)?;
    match &request.action {
        SessionAction::ExitOpen => {}
        SessionAction::Compact { instructions } => {
            return parts_ops::compact(feed, &id, instructions.as_deref());
        }
        SessionAction::BoardRefresh => return parts_ops::board_refresh(feed, &id),
        SessionAction::Export => return Err("this session's host does not export sessions".into()),
        SessionAction::Shell { .. } => {
            return Err("this session's host runs no owner shell commands".into());
        }
        SessionAction::Other => return Err("the host does not know that action".into()),
    }
    let at = stamp(feed.now);
    let card = exit_card(feed, id.as_str(), &at)?;
    let base = base(feed, id.as_str());
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    let running = host.exit.as_ref().is_some_and(|exit| {
        matches!(
            exit.phase,
            ExitPhase::Rebasing | ExitPhase::Verifying | ExitPhase::Repairing | ExitPhase::Landing
        )
    });
    if running {
        return Err("the exit is already running".into());
    }
    if !host
        .pending
        .iter()
        .any(|found| found.kind == CardKind::Exit)
    {
        host.pending.push(card);
    }
    host.exit = Some(brief(ExitPhase::Pending, None, base));
    settle(feed, id.as_str());
    reply(SessionActionDone { text: None })
}

/// One clock tick: every other tick a landing build's exit takes its next phase.
pub(super) fn tick(feed: &mut Feed) {
    if feed.now % 2 != 0 {
        return;
    }
    let moving: Vec<Id> = feed
        .hosts
        .iter()
        .filter(|(_, host)| {
            host.exit.as_ref().is_some_and(|exit| {
                matches!(
                    exit.phase,
                    ExitPhase::Rebasing | ExitPhase::Verifying | ExitPhase::Landing
                )
            })
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in moving {
        let at = stamp(feed.now);
        if let Some(host) = feed.hosts.get_mut(id.as_str())
            && let Some(exit) = host.exit.as_mut()
        {
            exit.phase = match exit.phase {
                ExitPhase::Rebasing => ExitPhase::Verifying,
                ExitPhase::Verifying => ExitPhase::Landing,
                _ => {
                    exit.landed = Some(sha(&format!("{id}/landed"), 40));
                    ExitPhase::Landed
                }
            };
            let phase = exit.phase;
            let attempt = host
                .parts
                .as_deref()
                .map_or(0, |parts| parts.record.verification.len())
                + 1;
            walk(exit, &at, id.as_str(), &format!("G{attempt}"));
            parts_ops::exited(host, &at, phase, None);
        }
        settle(feed, id.as_str());
    }
}

/// Withdraws an owner's queued item with its whole text.
pub(super) fn withdraw(feed: &mut Feed, request: &SessionQueueKey) -> Result<Value, String> {
    let id = running(feed, &request.id)?;
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    let found = host.queued.iter().position(|input| {
        input.origin == Origin::Owner && input.key() == Some(request.key.as_str())
    });
    let withdrawn = match found {
        Some(at) => {
            let input = host.queued.remove(at);
            QueueWithdrawn {
                outcome: WithdrawOutcome::Withdrawn,
                turn: Some(host.turn),
                text: Some(input.text),
            }
        }
        None => QueueWithdrawn {
            outcome: WithdrawOutcome::UnknownItem,
            turn: None,
            text: None,
        },
    };
    settle(feed, id.as_str());
    reply(withdrawn)
}

/// Moves an owner's queued item past the next owner item in `delta`'s direction.
pub(super) fn move_item(feed: &mut Feed, request: &SessionQueueMove) -> Result<Value, String> {
    let id = running(feed, &request.id)?;
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    let owner = |at: &usize| {
        host.queued
            .get(*at)
            .is_some_and(|input| input.origin == Origin::Owner)
    };
    let from = host.queued.iter().position(|input| {
        input.origin == Origin::Owner && input.key() == Some(request.key.as_str())
    });
    let to = from.and_then(|from| {
        if request.delta < 0 {
            (0..from).rev().find(owner)
        } else {
            (from + 1..host.queued.len()).find(owner)
        }
    });
    let moved = match (from, to) {
        (Some(from), Some(to)) => {
            host.queued.swap(from, to);
            true
        }
        _ => false,
    };
    settle(feed, id.as_str());
    reply(QueueMoved { moved })
}

/// Removes a stopped sample build, or says what that would remove (`dry_run`). A build that ran
/// in the project's checkout keeps it.
pub(super) fn remove(feed: &mut Feed, request: &BuildRemove) -> Result<Value, String> {
    let id = request.id.as_str();
    let build = feed
        .assistants
        .get(id)
        .filter(|row| row.kind == AssistantKind::Build)
        .and_then(|row| row.build.clone())
        .ok_or_else(|| format!("no build session {id}"))?;
    if feed.hosts.get(id).is_some_and(|host| !host.gone) {
        return Err("this build's host is running; stop it before removing it".into());
    }
    let (would, done) = if request.dry_run {
        ("would remove", "would delete")
    } else {
        ("removed", "deleted")
    };
    let mut lines = vec![format!("Preview · sample prune of {id}")];
    let branch = build.branch.clone().filter(|_| !build.here);
    if !build.here {
        lines.push(format!("{would} worktree {}", build.worktree));
    }
    if let Some(branch) = &branch {
        lines.push(format!("{done} branch {branch}"));
    }
    lines.push(format!("{would} record {id}"));
    if !request.dry_run {
        feed.assistants.remove(id);
        feed.hosts.remove(id);
        feed.live.remove(id);
    }
    reply(BuildRemoved {
        worktree: !build.here,
        branch: branch.is_some(),
        record: true,
        kept: build
            .here
            .then(|| "it ran in the project's checkout, which stays".into()),
        lines,
    })
}
