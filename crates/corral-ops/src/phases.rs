//! Pure phase timeline over one task's lifecycle events.
//!
//! Pairs queue, active implementation, needs-me, gate, review, owner-accept and landing
//! spans per run and round, plus task lead and cycle time. An unpaired or interrupted
//! span is incomplete: it keeps the interrupting event's time as `end`, but never a
//! duration, and nothing is closed at the reader's clock. This module performs no I/O.
//!
//! Pairing rules:
//! - queue: first `queued` since the last dispatch -> `dispatched` (`dequeued` and
//!   `discarded` interrupt it).
//! - active: `dispatched` or `changes_requested` -> `done`, minus complete needs-me time
//!   inside the round; an overlapping incomplete needs-me span leaves it incomplete.
//! - needs_me: `needs_me` -> `resumed`, `replied`, or `run_resumed` answering the
//!   last decision the wait raised. Failed deliveries (`submitted: false`) of
//!   `replied` and `changes_requested` change no phase.
//! - gate: `gate_started` -> `gate` (pass or fail), per attempt; a restart before a
//!   result (`no_result`) or `review_released` of the gate (`process_lost`) interrupts it.
//! - review: `review_started` -> `verdict`; the owner's `changes_requested` or `approved`
//!   before a verdict (normally after `review_escalated`) finishes it as `owner_finished`.
//! - owner_accept: autopilot `reviewed` (accept = owner) -> the approval decision, which is
//!   autopilot `landing`, the owner's first landing attempt or `approved`.
//! - landing: the approval decision -> `approved`. `approved` is written after landing,
//!   so an owner approval starts at the event's recorded `started`; older events
//!   without it leave landing and owner wait incomplete (`unknown_start`).
//! - lead: `created` -> the last `done` after the last change request.
//! - cycle: first `dispatched` -> the last landing `approved` (not `keep_branch`).
//!
//! Events without a run id belong to the run whose dispatch interval contains them.
use std::collections::{BTreeMap, BTreeSet};

use corral_core::{CommandReceipt, Event, Task};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queue,
    Active,
    NeedsMe,
    Gate,
    Review,
    OwnerAccept,
    Landing,
    Lead,
    Cycle,
}

/// One gate command's time; `None` for receipts written before command timestamps.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandSpan {
    pub command: String,
    pub exit_code: i32,
    pub seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Span {
    pub phase: Phase,
    /// Run id; `None` for task-level spans and queue waits that never dispatched.
    pub run: Option<String>,
    pub round: Option<u32>,
    pub start: Timestamp,
    /// Closing or interrupting event; `None` while still open at the last event.
    pub end: Option<Timestamp>,
    /// Present only for complete spans.
    pub seconds: Option<f64>,
    pub complete: bool,
    /// Closing outcome (`pass`, `approve`, `landed`, ...) or interruption reason
    /// (`open`, `process_lost`, `no_result`, `superseded`, ...).
    pub outcome: String,
    /// Who finished a review: `reviewer` or `owner`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CommandSpan>,
}

fn seconds(start: Timestamp, end: Timestamp) -> Option<f64> {
    (end >= start).then(|| end.duration_since(start).as_secs_f64())
}

fn span(
    phase: Phase,
    run: Option<&str>,
    round: Option<u32>,
    start: Timestamp,
    end: Option<Timestamp>,
    complete: bool,
    outcome: &str,
) -> Span {
    let duration = end.filter(|_| complete).and_then(|end| seconds(start, end));
    let negative = complete && duration.is_none();
    Span {
        phase,
        run: run.map(str::to_owned),
        round,
        start,
        end,
        seconds: duration,
        complete: duration.is_some(),
        outcome: if negative {
            "negative_duration"
        } else {
            outcome
        }
        .to_owned(),
        by: None,
        commands: Vec::new(),
    }
}

struct NeedsMe {
    start: Timestamp,
    end: Option<Timestamp>,
    complete: bool,
}

struct RunState {
    round: u32,
    dispatched: bool,
    active: Option<Timestamp>,
    needs_open: Option<Timestamp>,
    /// Portable decisions the open wait still needs answered.
    decisions: BTreeSet<String>,
    needs: Vec<NeedsMe>,
    gate: Option<(Timestamp, u32)>,
    review: Option<(Timestamp, u32)>,
    accept: Option<(Timestamp, u32)>,
    landing: Option<(Timestamp, u32)>,
}

impl RunState {
    fn new() -> Self {
        Self {
            round: 1,
            dispatched: false,
            active: None,
            needs_open: None,
            decisions: BTreeSet::new(),
            needs: Vec::new(),
            gate: None,
            review: None,
            accept: None,
            landing: None,
        }
    }

    fn has_open(&self) -> bool {
        self.active.is_some()
            || self.needs_open.is_some()
            || self.gate.is_some()
            || self.review.is_some()
            || self.accept.is_some()
            || self.landing.is_some()
    }
}

#[derive(Default)]
struct Builder {
    spans: Vec<Span>,
    runs: BTreeMap<String, RunState>,
    queue: Option<Timestamp>,
    created: Option<Timestamp>,
    first_dispatch: Option<Timestamp>,
    last_done: Option<Timestamp>,
    last_change: Option<Timestamp>,
    landed: Option<Timestamp>,
}

fn close<'a>(
    spans: &'a mut Vec<Span>,
    phase: Phase,
    run: &str,
    open: Option<(Timestamp, u32)>,
    end: Option<Timestamp>,
    complete: bool,
    outcome: &str,
) -> Option<&'a mut Span> {
    let (start, round) = open?;
    spans.push(span(
        phase,
        Some(run),
        Some(round),
        start,
        end,
        complete,
        outcome,
    ));
    spans.last_mut()
}

fn close_needs(
    spans: &mut Vec<Span>,
    run: &str,
    state: &mut RunState,
    end: Option<Timestamp>,
    complete: bool,
    outcome: &str,
) {
    state.decisions.clear();
    if let Some(start) = state.needs_open.take() {
        let closed = span(
            Phase::NeedsMe,
            Some(run),
            Some(state.round),
            start,
            end,
            complete,
            outcome,
        );
        state.needs.push(NeedsMe {
            start,
            end,
            complete: closed.complete,
        });
        spans.push(closed);
    }
}

/// Closes the round's active span; needs-me time is subtracted only when every
/// needs-me span overlapping the round is complete.
fn close_active(
    spans: &mut Vec<Span>,
    run: &str,
    state: &mut RunState,
    end: Option<Timestamp>,
    complete: bool,
    outcome: &str,
) {
    let Some(start) = state.active.take() else {
        return;
    };
    let mut closed = span(
        Phase::Active,
        Some(run),
        Some(state.round),
        start,
        end,
        complete,
        outcome,
    );
    if let (Some(wall), Some(end)) = (closed.seconds, end) {
        let mut waited = 0.0;
        let mut unknown = false;
        for wait in &state.needs {
            let overlaps = wait.start < end && wait.end.is_none_or(|e| e > start);
            match wait.end.filter(|_| wait.complete) {
                Some(wait_end) if overlaps => {
                    waited += seconds(wait.start.max(start), wait_end.min(end)).unwrap_or(0.0);
                }
                Some(_) => {}
                None => unknown |= overlaps,
            }
        }
        if unknown {
            closed.seconds = None;
            closed.complete = false;
            closed.outcome = "needs_me_incomplete".into();
        } else {
            closed.seconds = Some((wall - waited).max(0.0));
        }
    }
    spans.push(closed);
}

/// Interrupts every open span of one run; `at = None` marks spans still open at the end.
fn interrupt(
    spans: &mut Vec<Span>,
    run: &str,
    state: &mut RunState,
    at: Option<Timestamp>,
    why: &str,
) {
    close_needs(spans, run, state, at, false, why);
    close_active(spans, run, state, at, false, why);
    for (phase, open) in [
        (Phase::Gate, state.gate.take()),
        (Phase::Review, state.review.take()),
        (Phase::OwnerAccept, state.accept.take()),
        (Phase::Landing, state.landing.take()),
    ] {
        close(spans, phase, run, open, at, false, why);
    }
}

fn commands(event: &Event) -> Vec<CommandSpan> {
    serde_json::from_value::<Vec<CommandReceipt>>(event.detail["commands"].clone())
        .unwrap_or_default()
        .into_iter()
        .map(|receipt| CommandSpan {
            seconds: receipt
                .started
                .zip(receipt.ended)
                .and_then(|(start, end)| seconds(start, end)),
            command: receipt.command,
            exit_code: receipt.exit_code,
        })
        .collect()
}

impl Builder {
    fn state(&mut self, run: &str) -> &mut RunState {
        self.runs
            .entry(run.to_owned())
            .or_insert_with(RunState::new)
    }

    fn task_event(&mut self, event: &Event) {
        let at = event.ts;
        match event.kind.as_str() {
            "created" => {
                self.created.get_or_insert(at);
            }
            "queued" => {
                self.queue.get_or_insert(at);
            }
            "dequeued" => {
                if let Some(start) = self.queue.take() {
                    self.spans.push(span(
                        Phase::Queue,
                        None,
                        None,
                        start,
                        Some(at),
                        false,
                        "dequeued",
                    ));
                }
            }
            _ => {}
        }
    }

    fn run_event(&mut self, event: &Event, run: &str) {
        let at = event.ts;
        let detail = &event.detail;
        // Failed deliveries are audit records; the worker never received them.
        if matches!(event.kind.as_str(), "replied" | "changes_requested")
            && detail["submitted"] == false
        {
            return;
        }
        match event.kind.as_str() {
            "dispatched" => self.dispatched(at, run),
            "needs_me" => {
                let state = self.state(run);
                state.needs_open.get_or_insert(at);
                if let Some(decision) = detail["decision"].as_str() {
                    state.decisions.insert(decision.to_owned());
                }
            }
            // Only an answered decision receipt resumes; wait receipts never answer.
            "run_resumed" if detail["outcome"] == "answered" => {
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                let decision = detail["decision"]
                    .as_str()
                    .or_else(|| detail["receipt"].as_str());
                if decision.is_some_and(|id| state.decisions.remove(id))
                    && state.decisions.is_empty()
                {
                    close_needs(&mut self.spans, run, state, Some(at), true, "run_resumed");
                }
            }
            "resumed" | "replied" => {
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                close_needs(&mut self.spans, run, state, Some(at), true, &event.kind);
            }
            "done" => {
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                close_needs(&mut self.spans, run, state, Some(at), false, "done");
                close_active(&mut self.spans, run, state, Some(at), true, "done");
                self.last_done = self.last_done.max(Some(at));
            }
            "changes_requested" => {
                let by = detail["by"].as_str().unwrap_or("owner").to_owned();
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                close_active(
                    &mut self.spans,
                    run,
                    state,
                    Some(at),
                    false,
                    "changes_requested",
                );
                let review = state.review.take();
                if let Some(owner) = close(
                    &mut self.spans,
                    Phase::Review,
                    run,
                    review,
                    Some(at),
                    true,
                    "owner_finished",
                ) {
                    owner.by = Some(by);
                }
                for (phase, open) in [
                    (Phase::OwnerAccept, state.accept.take()),
                    (Phase::Landing, state.landing.take()),
                ] {
                    close(
                        &mut self.spans,
                        phase,
                        run,
                        open,
                        Some(at),
                        false,
                        "changes_requested",
                    );
                }
                state.round = state.round.saturating_add(1);
                state.active = Some(at);
                self.last_change = Some(at);
            }
            "gate_started" => {
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                let previous = state.gate.replace((at, state.round));
                close(
                    &mut self.spans,
                    Phase::Gate,
                    run,
                    previous,
                    Some(at),
                    false,
                    "no_result",
                );
            }
            "gate" => {
                if let Some(result @ ("pass" | "fail")) = detail["result"].as_str() {
                    let state = self.state(run);
                    let open = state.gate.take();
                    if let Some(gate) = close(
                        &mut self.spans,
                        Phase::Gate,
                        run,
                        open,
                        Some(at),
                        true,
                        result,
                    ) {
                        gate.commands = commands(event);
                    }
                }
            }
            "review_released" => {
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                let (phase, open) = match detail["kind"].as_str() {
                    Some("gate") => (Phase::Gate, state.gate.take()),
                    Some("reviewer") => (Phase::Review, state.review.take()),
                    _ => return,
                };
                close(
                    &mut self.spans,
                    phase,
                    run,
                    open,
                    Some(at),
                    false,
                    "process_lost",
                );
            }
            "review_started" => {
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                let previous = state.review.replace((at, state.round));
                close(
                    &mut self.spans,
                    Phase::Review,
                    run,
                    previous,
                    Some(at),
                    false,
                    "restarted",
                );
            }
            "verdict" => {
                let verdict = detail["verdict"].as_str().unwrap_or("unknown").to_owned();
                let by = if detail["reviewer"] == "owner" {
                    "owner"
                } else {
                    "reviewer"
                };
                let open = self.state(run).review.take();
                if let Some(review) = close(
                    &mut self.spans,
                    Phase::Review,
                    run,
                    open,
                    Some(at),
                    true,
                    &verdict,
                ) {
                    review.by = Some(by.into());
                }
            }
            "autopilot" => self.autopilot(at, run, detail["to"].as_str().unwrap_or("")),
            "landing_stale" => self.decided(at, run),
            "approved" => self.approved(event, run),
            "discarded" | "superseded" => {
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                interrupt(&mut self.spans, run, state, Some(at), &event.kind);
                if event.kind == "discarded"
                    && let Some(start) = self.queue.take()
                {
                    self.spans.push(span(
                        Phase::Queue,
                        None,
                        None,
                        start,
                        Some(at),
                        false,
                        "discarded",
                    ));
                }
            }
            _ => {}
        }
    }

    fn dispatched(&mut self, at: Timestamp, run: &str) {
        if let Some(start) = self.queue.take() {
            self.spans.push(span(
                Phase::Queue,
                Some(run),
                None,
                start,
                Some(at),
                true,
                "dispatched",
            ));
        }
        for (other, state) in self.runs.iter_mut().filter(|(id, _)| id.as_str() != run) {
            if state.has_open() {
                interrupt(&mut self.spans, other, state, Some(at), "next_run");
            }
        }
        self.first_dispatch = Some(self.first_dispatch.map_or(at, |first| first.min(at)));
        let state = self.state(run);
        if !state.dispatched {
            state.dispatched = true;
            state.active.get_or_insert(at);
        }
    }

    fn decided(&mut self, at: Timestamp, run: &str) {
        let state = self
            .runs
            .entry(run.to_owned())
            .or_insert_with(RunState::new);
        let accept = state.accept.take();
        close(
            &mut self.spans,
            Phase::OwnerAccept,
            run,
            accept,
            Some(at),
            true,
            "approved",
        );
        state.landing.get_or_insert((at, state.round));
    }

    fn autopilot(&mut self, at: Timestamp, run: &str, to: &str) {
        match to {
            "reviewed" => {
                let state = self.state(run);
                state.accept.get_or_insert((at, state.round));
            }
            "landing" => self.decided(at, run),
            "changes" | "escalated" => {
                let state = self
                    .runs
                    .entry(run.to_owned())
                    .or_insert_with(RunState::new);
                let why = format!("autopilot_{to}");
                for (phase, open) in [
                    (Phase::OwnerAccept, state.accept.take()),
                    (Phase::Landing, state.landing.take()),
                ] {
                    close(&mut self.spans, phase, run, open, Some(at), false, &why);
                }
            }
            _ => {}
        }
    }

    fn approved(&mut self, event: &Event, run: &str) {
        let at = event.ts;
        let by = match event.actor {
            Some(corral_core::Actor::Autopilot) => "autopilot",
            Some(corral_core::Actor::Pm) => "pm",
            _ => "owner",
        };
        // `approved` is written after landing; its start is the recorded approval
        // time or an earlier decision event, never the approval event itself.
        let started = event.detail["started"]
            .as_str()
            .and_then(|s| s.parse::<Timestamp>().ok());
        let state = self
            .runs
            .entry(run.to_owned())
            .or_insert_with(RunState::new);
        let opened = state.landing.map(|(start, _)| start);
        let decision = opened.or(started).filter(|start| *start <= at);
        let round = state.round;
        let review = state.review.take();
        if let Some(owner) = close(
            &mut self.spans,
            Phase::Review,
            run,
            review,
            Some(decision.unwrap_or(at)),
            true,
            "owner_finished",
        ) {
            owner.by = Some(by.into());
        }
        let kept = event.detail["strategy"] == "keep_branch";
        let outcome = if kept { "kept_branch" } else { "landed" };
        match decision {
            Some(decision) => {
                if opened.is_none() {
                    self.decided(decision, run);
                }
                let landing = self.state(run).landing.take();
                close(
                    &mut self.spans,
                    Phase::Landing,
                    run,
                    landing,
                    Some(at),
                    true,
                    outcome,
                );
            }
            None => {
                let accept = self.state(run).accept.take();
                let why = "unknown_decision_time";
                close(
                    &mut self.spans,
                    Phase::OwnerAccept,
                    run,
                    accept,
                    Some(at),
                    false,
                    why,
                );
                self.spans.push(span(
                    Phase::Landing,
                    Some(run),
                    Some(round),
                    at,
                    Some(at),
                    false,
                    "unknown_start",
                ));
            }
        }
        if !kept {
            self.landed = Some(at);
        }
    }

    fn finish(mut self) -> Vec<Span> {
        for (run, state) in &mut self.runs {
            interrupt(&mut self.spans, run, state, None, "open");
        }
        if let Some(start) = self.queue.take() {
            self.spans
                .push(span(Phase::Queue, None, None, start, None, false, "open"));
        }
        if let Some(created) = self.created {
            let done = self
                .last_done
                .filter(|done| self.last_change.is_none_or(|change| *done >= change));
            self.spans.push(span(
                Phase::Lead,
                None,
                None,
                created,
                done,
                done.is_some(),
                if done.is_some() { "done" } else { "open" },
            ));
        }
        if let Some(first) = self.first_dispatch {
            let landed = self.landed.filter(|landed| *landed >= first);
            self.spans.push(span(
                Phase::Cycle,
                None,
                None,
                first,
                landed,
                landed.is_some(),
                if landed.is_some() {
                    "landed"
                } else {
                    "not_landed"
                },
            ));
        }
        self.spans.sort_by_key(|span| span.start);
        self.spans
    }
}

/// Pairs one task's events (any order; equal timestamps keep their order) into spans.
pub fn task_phases(task: &Task, events: &[&Event]) -> Vec<Span> {
    let mut events: Vec<&Event> = events
        .iter()
        .copied()
        .filter(|event| event.task.as_deref() == Some(task.id.as_str()))
        .collect();
    events.sort_by_key(|event| event.ts);
    let mut windows: Vec<(Timestamp, &str)> = task
        .runs
        .iter()
        .map(|run| (run.started, run.id.as_str()))
        .chain(
            events
                .iter()
                .filter(|e| e.kind == "dispatched")
                .filter_map(|e| e.detail["run"].as_str().map(|run| (e.ts, run))),
        )
        .collect();
    windows.sort();
    let mut builder = Builder::default();
    for event in events {
        builder.task_event(event);
        let run = event.detail["run"].as_str().or_else(|| {
            windows
                .iter()
                .rev()
                .find(|(start, _)| *start <= event.ts)
                .map(|(_, run)| *run)
        });
        if let Some(run) = run {
            builder.run_event(event, run);
        }
    }
    builder.finish()
}

#[cfg(test)]
#[path = "phases_tests.rs"]
mod tests;
