//! Live lifecycle details over canonical scalar records and the existing phase projector.
//!
//! Instants are RFC3339, ranges are half-open and at most 366 days. Query budgets fail
//! explicitly instead of returning a deceptively complete truncated timeline/burnup.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use corral_core::backend::{HeadQuery, PageReq, TaskHeader, TaskNum, TaskOrder, TaskQuery};
use corral_core::{Actor, Event, RunHead};
use corral_protocol::detail::{
    Burnup, BurnupPoint, DetailBody, TimelineAction, TimelineBy, TimelineKind, TimelineLane,
    TimelinePage, TimelineSegment,
};
use jiff::Timestamp;

use crate::Ctx;
use crate::phases::{Phase, Span};

#[path = "timeline_history.rs"]
mod history;

#[cfg(not(test))]
const PAGE: u32 = 256;
// Exercise continuation with a small fixture, rather than hundreds of test events.
#[cfg(test)]
const PAGE: u32 = 4;
const MAX_TASKS: usize = 20_000;
const MAX_ROWS: usize = 100_000;
const PHASE_EVENTS: &[&str] = &[
    "created",
    "queued",
    "dequeued",
    "dispatched",
    "needs_me",
    "run_resumed",
    "reply_marked_delivered",
    "resumed",
    "replied",
    "done",
    "changes_requested",
    "gate_started",
    "gate",
    "review_released",
    "review_started",
    "verdict",
    "autopilot",
    "landing_stale",
    "approved",
    "discarded",
    "superseded",
    "gate_cancelled",
    "review_cancelled",
];

fn headers(ctx: &Ctx, project: Option<&str>, since: Option<Timestamp>) -> Result<Vec<TaskHeader>> {
    let view = ctx.store.view()?;
    let query = TaskQuery {
        statuses: Vec::new(),
        project: project.map(str::to_owned),
        order: TaskOrder::UpdatedDesc,
    };
    let mut page = PageReq::first(PAGE);
    let mut tasks = Vec::new();
    loop {
        let found = view
            .tasks(&query, page.clone())
            .context("reading timeline task index")?;
        for task in found.items {
            if since.is_some_and(|since| task.fields.updated < since) {
                return Ok(tasks);
            }
            ensure!(
                tasks.len() < MAX_TASKS,
                "unavailable: timeline task query exceeds {MAX_TASKS} tasks; select a narrower scope"
            );
            tasks.push(task);
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => return Ok(tasks),
        }
    }
}

fn indexed_headers(ctx: &Ctx, query: HeadQuery) -> Result<Vec<TaskHeader>> {
    let view = ctx.store.view()?;
    let mut page = PageReq::first(PAGE);
    let mut tasks = Vec::new();
    loop {
        let found = view
            .heads(&query, page.clone())
            .context("reading timeline scope index")?;
        ensure!(
            tasks.len() + found.items.len() <= MAX_TASKS,
            "unavailable: timeline scope exceeds {MAX_TASKS} tasks"
        );
        tasks.extend(found.items.into_iter().map(|doc| doc.header));
        match found.next {
            Some(next) => page.after = Some(next),
            None => return Ok(tasks),
        }
    }
}

fn header(ctx: &Ctx, id: &str) -> Result<TaskHeader> {
    let id = corral_core::Store::normalize_id(id)?;
    let num = id
        .strip_prefix("T-")
        .context("invalid task id")?
        .parse()
        .context("invalid task number")?;
    ctx.store
        .view()?
        .head(TaskNum(num))?
        .map(|doc| doc.header)
        .with_context(|| format!("task {id} not found"))
}

fn events(ctx: &Ctx, task: Option<&str>, kinds: &[&str], budget: &mut usize) -> Result<Vec<Event>> {
    let mut after = 0;
    let mut result = Vec::new();
    loop {
        let found = ctx
            .store
            .events_page(task, kinds, None, after, PAGE)
            .context("reading recorded lifecycle events")?;
        *budget += found.len();
        ensure!(
            *budget <= MAX_ROWS,
            "unavailable: lifecycle exceeds {MAX_ROWS} events; select a narrower scope"
        );
        let Some(last) = found.last() else {
            return Ok(result);
        };
        after = last.seq;
        let done = found.len() < PAGE as usize;
        result.extend(found);
        if done {
            return Ok(result);
        }
    }
}

fn overlaps(start: Timestamp, end: Option<Timestamp>, from: Timestamp, to: Timestamp) -> bool {
    start < to && end.is_none_or(|end| end >= from)
}

fn lane(
    run: Option<&RunHead>,
    task: &TaskHeader,
    by: TimelineBy,
    review: bool,
) -> (String, String) {
    if by == TimelineBy::Task {
        return (
            task.id.clone(),
            super::short(&format!("{} · {}", task.id, task.fields.title), 160),
        );
    }
    match run {
        Some(run) => {
            let id = run
                .session
                .as_ref()
                .and_then(|session| session.id.clone())
                .unwrap_or_else(|| format!("{}/{}", task.id, run.id));
            let role = if review { "reviewer" } else { "worker" };
            (
                id.clone(),
                super::short(
                    &format!(
                        "{id} · {role} · {}",
                        run.profile.as_deref().unwrap_or(&run.agent)
                    ),
                    160,
                ),
            )
        }
        None => (
            format!("{}/unattributed", task.id),
            format!("{} · session unavailable", task.id),
        ),
    }
}

fn push(
    lanes: &mut BTreeMap<String, TimelineLane>,
    id: String,
    title: String,
    segment: TimelineSegment,
) {
    lanes
        .entry(id.clone())
        .or_insert_with(|| TimelineLane {
            id,
            title,
            segments: Vec::new(),
        })
        .segments
        .push(segment);
}

fn segment(span: &Span, kind: TimelineKind) -> TimelineSegment {
    let unavailable = span.outcome == "unknown_start";
    TimelineSegment {
        kind: if unavailable {
            TimelineKind::Unavailable
        } else {
            kind
        },
        from: (!unavailable).then(|| span.start.to_string()),
        to: span.end.map(|at| at.to_string()),
        note: Some(super::short(
            &format!(
                "{}{}",
                if unavailable { "land: " } else { "" },
                span.outcome
            ),
            240,
        )),
    }
}

fn kind(phase: Phase) -> Option<TimelineKind> {
    match phase {
        Phase::Active => Some(TimelineKind::Run),
        Phase::Gate => Some(TimelineKind::Gate),
        Phase::Review => Some(TimelineKind::Review),
        Phase::Landing => Some(TimelineKind::Land),
        Phase::NeedsMe => Some(TimelineKind::Ask),
        Phase::OwnerAccept => Some(TimelineKind::Held),
        Phase::Queue => Some(TimelineKind::Idle),
        Phase::Lead | Phase::Cycle => None,
    }
}

fn sorted_lanes(lanes: BTreeMap<String, TimelineLane>) -> Vec<TimelineLane> {
    lanes
        .into_values()
        .map(|mut lane| {
            // All endpoints are Timestamp's UTC rendering. Removing the trailing
            // Z makes an integral second sort before fractions of that second.
            fn key(at: &Option<String>) -> Option<&str> {
                at.as_deref().map(|at| at.trim_end_matches('Z'))
            }
            lane.segments.sort_by(|a, b| {
                key(&a.from)
                    .cmp(&key(&b.from))
                    .then_with(|| key(&a.to).cmp(&key(&b.to)))
            });
            lane
        })
        .collect()
}

/// The shared projector predates cancellation and stale-result events. Complete only
/// those missing exits from their own recorded event, never from the reader's clock.
fn recorded_exits(spans: &mut [Span], events: &[&Event]) {
    let mut indexes: BTreeMap<Phase, BTreeMap<String, Vec<usize>>> = BTreeMap::new();
    for (index, span) in spans.iter().enumerate() {
        if !matches!(span.phase, Phase::Gate | Phase::Review) {
            continue;
        }
        let Some(run) = span.run.as_deref() else {
            continue;
        };
        let runs = indexes.entry(span.phase).or_default();
        if let Some(indices) = runs.get_mut(run) {
            indices.push(index);
        } else {
            runs.insert(run.to_owned(), vec![index]);
        }
    }
    for event in events {
        let (phase, outcome) = match event.kind.as_str() {
            "gate_cancelled" => (Phase::Gate, "cancelled"),
            "review_cancelled" => (Phase::Review, "cancelled"),
            "gate" if matches!(event.detail["result"].as_str(), Some("stale" | "cancelled")) => (
                Phase::Gate,
                event.detail["result"].as_str().unwrap_or_default(),
            ),
            _ => continue,
        };
        let Some(run) = event.detail["run"].as_str() else {
            continue;
        };
        let Some(indices) = indexes.get(&phase).and_then(|runs| runs.get(run)) else {
            continue;
        };
        let before = indices.partition_point(|index| spans[*index].start <= event.ts);
        let Some(index) = before.checked_sub(1).map(|index| indices[index]) else {
            continue;
        };
        let span = &mut spans[index];
        if span.complete || span.end.is_some_and(|end| end < event.ts) {
            continue;
        }
        span.end = Some(event.ts);
        span.seconds = Some(event.ts.duration_since(span.start).as_secs_f64());
        span.complete = true;
        span.outcome = outcome.to_owned();
    }
}

/// Range bounds are supplied by the app; no fixture date or reader-clock end is inserted.
pub(super) fn timeline(ctx: &Ctx, from: &str, to: &str, by: TimelineBy) -> Result<DetailBody> {
    let from: Timestamp = from
        .parse()
        .context("invalid timeline from: expected RFC3339 instant")?;
    let to: Timestamp = to
        .parse()
        .context("invalid timeline to: expected RFC3339 instant")?;
    ensure!(
        from < to && to.as_second() - from.as_second() <= 366 * 86_400,
        "invalid timeline range: from must precede to by at most 366 days"
    );
    ensure!(
        matches!(by, TimelineBy::Task | TimelineBy::Agent),
        "invalid timeline grouping"
    );
    let mut lanes = BTreeMap::new();
    let mut owner_actions = Vec::new();
    let mut pm = Vec::new();
    let mut budget = 0;
    let point_events = history::read(ctx, &history::query(None, Some(from), to), &mut budget)?;
    let mut tasks: BTreeMap<_, _> = headers(ctx, None, Some(from))?
        .into_iter()
        .map(|task| (task.id.clone(), task))
        .collect();
    for task in indexed_headers(ctx, HeadQuery::Open { project: None })? {
        tasks.insert(task.id.clone(), task);
    }
    let view = ctx.store.view()?;
    for id in point_events
        .iter()
        .filter_map(|event| event.task.as_deref())
        .collect::<BTreeSet<_>>()
    {
        if tasks.contains_key(id) {
            continue;
        }
        if let Ok(id) = corral_core::Store::normalize_id(id)
            && let Some(num) = id.strip_prefix("T-").and_then(|num| num.parse().ok())
            && let Some(doc) = view.head(TaskNum(num))?
        {
            tasks.insert(doc.header.id.clone(), doc.header);
        }
    }
    ensure!(
        tasks.len() <= MAX_TASKS,
        "unavailable: timeline range exceeds {MAX_TASKS} tasks"
    );
    for task in tasks.into_values() {
        if task.created >= to {
            continue;
        }
        let mut task_events = history::phase_window(ctx, &task.id, from, to, &mut budget)?;
        let dispatch = history::dispatch(ctx, &task.id, from, &mut budget)?;
        let floor = dispatch.as_ref().map(|event| event.ts);
        let dispatch_run = dispatch
            .as_ref()
            .and_then(|event| event.detail["run"].as_str())
            .map(str::to_owned);
        if let Some(event) = dispatch {
            task_events.push(event);
        }
        let mut runs = BTreeMap::new();
        history::run_rows(
            ctx,
            &task,
            Some((from, to)),
            &task_events,
            &mut runs,
            &mut budget,
        )?;
        let legacy_run = dispatch_run.or_else(|| {
            runs.iter()
                .filter(|(key, run)| !key.contains('/') && run.started < from)
                .max_by_key(|(_, run)| run.started)
                .map(|(_, run)| run.id.clone())
        });
        let ids: BTreeSet<_> = runs
            .iter()
            .filter(|(key, run)| !key.contains('/') && run.started < from)
            .map(|(_, run)| run.id.clone())
            .chain(
                task_events
                    .iter()
                    .filter_map(|event| event.detail["run"].as_str().map(str::to_owned)),
            )
            .collect();
        for run in &ids {
            task_events.extend(history::context(
                ctx,
                &task.id,
                run,
                legacy_run.as_deref() == Some(run.as_str()),
                floor,
                from,
                &mut budget,
            )?);
        }
        task_events.extend(history::queue(ctx, &task.id, from, &mut budget)?);
        task_events.sort_by_key(|event| (event.ts, event.seq));
        task_events.dedup_by_key(|event| event.seq);
        // Context can name a reviewer session that ended before the range.
        history::run_rows(ctx, &task, None, &task_events, &mut runs, &mut budget)?;
        let head = ctx.store.head(&task.id)?;
        let mut shell = head.to_shell_task()?;
        shell.runs = runs
            .iter()
            .filter(|(key, _)| !key.contains('/'))
            .map(|(_, run)| Ok(serde_json::from_value(serde_json::to_value(run)?)?))
            .collect::<Result<Vec<_>>>()?;
        let refs: Vec<_> = task_events.iter().collect();
        let mut spans = crate::phases::task_phases(&shell, &refs);
        recorded_exits(&mut spans, &refs);
        let mut seen = BTreeSet::new();
        for span in &spans {
            let Some(kind) = kind(span.phase) else {
                continue;
            };
            if !overlaps(span.start, span.end, from, to) {
                continue;
            }
            let parent = runs.iter().find(|(key, run)| {
                !key.contains('/') && span.run.as_deref() == Some(run.id.as_str())
            });
            let selected = if span.phase == Phase::Review {
                runs.iter().find(|(key, run)| {
                    parent.is_some_and(|(parent_key, _)| {
                        key.strip_prefix(parent_key.as_str())
                            .is_some_and(|suffix| suffix.starts_with('/'))
                    }) && run.started <= span.start
                        && run.ended.is_none_or(|end| end >= span.start)
                })
            } else {
                parent
            };
            if let Some(run) = span.run.as_ref() {
                seen.insert((run.clone(), span.phase));
            }
            let (id, title) = lane(
                selected.map(|(_, run)| run),
                &task,
                by,
                span.phase == Phase::Review,
            );
            push(&mut lanes, id, title, segment(span, kind));
        }
        let mut closed: BTreeSet<_> = spans
            .iter()
            .filter_map(|span| span.end.map(|end| (span.phase, end)))
            .collect();
        for (key, run) in &runs {
            if let Some(end) = run.ended {
                closed.insert((
                    if key.contains('/') {
                        Phase::Review
                    } else {
                        Phase::Active
                    },
                    end,
                ));
            }
        }
        // An end receipt is still a real phase fact when its legacy start is absent.
        // Preserve that fact explicitly instead of fabricating a zero-duration span.
        for event in &task_events {
            if event.ts < from || event.ts >= to {
                continue;
            }
            let (phase, label) = match event.kind.as_str() {
                "gate" | "gate_cancelled" => (Phase::Gate, "gate"),
                "verdict" | "review_cancelled" => (Phase::Review, "review"),
                "approved" => (Phase::Landing, "land"),
                _ => continue,
            };
            if closed.contains(&(phase, event.ts)) {
                continue;
            }
            let run_id = event.detail["run"].as_str();
            let selected = runs
                .iter()
                .find(|(key, run)| !key.contains('/') && Some(run.id.as_str()) == run_id);
            if let Some(run) = run_id {
                seen.insert((run.to_owned(), phase));
            }
            let (id, title) = lane(selected.map(|(_, run)| run), &task, by, false);
            push(
                &mut lanes,
                id,
                title,
                TimelineSegment {
                    kind: TimelineKind::Unavailable,
                    from: None,
                    to: Some(event.ts.to_string()),
                    note: Some(format!("{label}: start not recorded")),
                },
            );
        }
        for (key, run) in &runs {
            let review = key.contains('/');
            if !overlaps(run.started, run.ended, from, to) {
                continue;
            }
            let (id, title) = lane(Some(run), &task, by, review);
            let already = if review {
                spans.iter().any(|span| {
                    span.phase == Phase::Review
                        && span.start >= run.started
                        && run.ended.is_none_or(|end| span.start <= end)
                })
            } else {
                spans.iter().any(|span| {
                    span.phase == Phase::Active && span.run.as_deref() == Some(run.id.as_str())
                })
            };
            if !already {
                push(
                    &mut lanes,
                    id.clone(),
                    title.clone(),
                    TimelineSegment {
                        kind: if review {
                            TimelineKind::Review
                        } else {
                            TimelineKind::Run
                        },
                        from: Some(run.started.to_string()),
                        to: run.ended.map(|at| at.to_string()),
                        note: Some(format!("{} · recorded session range", run.id)),
                    },
                );
            }
            if review {
                continue;
            }
            if let Some(gate) = &run.gate
                && gate.at < to
                && (gate.at >= from || gate.result == corral_core::GateResult::Running)
                && !seen.contains(&(run.id.clone(), Phase::Gate))
            {
                let started = gate.result == corral_core::GateResult::Running;
                let running = started && !gate.process_lost;
                push(
                    &mut lanes,
                    id.clone(),
                    title.clone(),
                    TimelineSegment {
                        kind: if running {
                            TimelineKind::Gate
                        } else {
                            TimelineKind::Unavailable
                        },
                        from: started.then(|| gate.at.to_string()),
                        to: (!started).then(|| gate.at.to_string()),
                        note: Some(format!(
                            "gate: {}{}",
                            gate.result.as_str(),
                            if running {
                                ""
                            } else if started {
                                "; end not recorded (process lost)"
                            } else {
                                "; start not recorded"
                            }
                        )),
                    },
                );
            }
            if let Some(review) = &run.review
                && review.at < to
                && (review.at >= from || review.verdict.is_none())
                && !seen.contains(&(run.id.clone(), Phase::Review))
                && !runs.iter().any(|(key, child)| {
                    key.contains('/')
                        && child.started <= review.at
                        && child.ended.is_none_or(|end| end >= review.at)
                })
            {
                let started = review.verdict.is_none();
                let running = started && !review.cancelled && !review.process_lost;
                push(
                    &mut lanes,
                    id.clone(),
                    title.clone(),
                    TimelineSegment {
                        kind: if running {
                            TimelineKind::Review
                        } else {
                            TimelineKind::Unavailable
                        },
                        from: started.then(|| review.at.to_string()),
                        to: (!started).then(|| review.at.to_string()),
                        note: Some(if running {
                            "review: in progress".into()
                        } else if started {
                            "review: end not recorded".into()
                        } else {
                            "review: start not recorded".into()
                        }),
                    },
                );
            }
            if head
                .landed
                .as_ref()
                .is_some_and(|receipt| receipt.run == run.id)
                && !seen.contains(&(run.id.clone(), Phase::Landing))
                && !spans.iter().any(|span| {
                    span.phase == Phase::Landing && span.run.as_deref() == Some(run.id.as_str())
                })
            {
                push(
                    &mut lanes,
                    id,
                    title,
                    TimelineSegment {
                        kind: TimelineKind::Unavailable,
                        from: None,
                        to: None,
                        note: Some(format!(
                            "land: {} receipt exists; timing not recorded",
                            run.id
                        )),
                    },
                );
            }
        }
    }
    // Point actions carry actual authorship, including workspace events and owner verbs
    // outside the phase-pairing vocabulary; no PM interval is inferred from these points.
    for event in point_events {
        let target = match event.actor {
            Some(Actor::Owner) => &mut owner_actions,
            Some(Actor::Pm) => &mut pm,
            _ => continue,
        };
        target.push(TimelineAction {
            at: event.ts.to_string(),
            task: event.task,
            text: super::short(&event.kind, 240),
            source_ref: Some(format!("event:{}", event.seq)),
        });
    }
    Ok(DetailBody::Timeline(TimelinePage {
        lanes: sorted_lanes(lanes),
        owner_actions,
        pm,
        last_visit: None,
    }))
}

/// Burnup uses actual creation and integration events, not status-update timestamps.
/// `plan:` selects current canonical members; it does not reconstruct past membership.
pub(super) fn burnup(ctx: &Ctx, scope: &str) -> Result<DetailBody> {
    let mut tasks = if let Some(project) = scope.strip_prefix("project:") {
        ensure!(!project.is_empty(), "invalid burnup project");
        headers(ctx, Some(project), None)?
    } else if let Some(plan) = scope.strip_prefix("plan:") {
        let owner = header(ctx, plan)?;
        indexed_headers(
            ctx,
            HeadQuery::EdgeTo {
                kind: "plan".into(),
                other: owner.id,
            },
        )?
    } else if let Some(id) = scope
        .strip_prefix("task:")
        .or_else(|| scope.starts_with("T-").then_some(scope))
    {
        vec![header(ctx, id)?]
    } else if matches!(scope, "all" | "workspace") {
        headers(ctx, None, None)?
    } else {
        bail!("invalid burnup scope: use all, workspace, project:<name>, plan:<id> or task:<id>");
    };
    tasks.retain(|task| !task.fields.tracker);
    let mut changes: BTreeMap<Timestamp, (u32, u32)> = BTreeMap::new();
    let mut budget = 0;
    for task in tasks {
        changes.entry(task.created).or_default().0 += 1;
        let approvals = events(ctx, Some(&task.id), &["approved"], &mut budget)?;
        if let Some(first) = approvals
            .iter()
            .filter(|event| event.detail["strategy"] != "keep_branch")
            .map(|event| event.ts)
            .min()
        {
            changes.entry(first).or_default().1 += 1;
        }
    }
    let mut total = 0u32;
    let mut landed = 0u32;
    let points = changes
        .into_iter()
        .map(|(at, (added, done))| {
            total += added;
            landed += done;
            BurnupPoint {
                at: at.to_string(),
                total,
                landed,
            }
        })
        .collect();
    Ok(DetailBody::Burnup(Burnup {
        scope: scope.to_owned(),
        points,
    }))
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;
