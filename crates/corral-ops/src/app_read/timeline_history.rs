//! Indexed range events and sufficient pre-range phase state, not task lifetime replay.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use corral_core::backend::{EventQuery, PageReq, RowQuery, TaskHeader, TaskPart};
use corral_core::{Event, RunHead};
use jiff::Timestamp;

use super::{PAGE, PHASE_EVENTS, budget::Budget};
use crate::Ctx;

fn project(row: corral_core::backend::StoredEvent) -> Result<Event> {
    let event = row.event;
    Ok(Event {
        seq: row.seq.0,
        ts: event.ts,
        task: event.task,
        kind: event.kind,
        actor: event
            .actor
            .map(|actor| serde_json::from_value(actor.into()))
            .transpose()?,
        via: event.via,
        detail: event.detail,
    })
}

pub(super) fn query(task: Option<&str>, since: Option<Timestamp>, until: Timestamp) -> EventQuery {
    EventQuery {
        task: task.map(str::to_owned),
        since,
        until: Some(until),
        chronological: true,
        valid_only: true,
        ..Default::default()
    }
}

pub(super) fn read(ctx: &Ctx, query: &EventQuery, budget: &mut Budget) -> Result<Vec<Event>> {
    if budget.exhausted() {
        return Ok(Vec::new());
    }
    let view = ctx.store.view()?;
    let mut page = PageReq::first(PAGE);
    let mut events = Vec::new();
    loop {
        let found = view
            .events(query, page.clone())
            .context("reading timeline time index")?;
        if !budget.admit(found.items.len()) {
            return Ok(events);
        }
        events.reserve(found.items.len());
        for row in found.items {
            let event = project(row)?;
            budget.observe(event.ts);
            events.push(event);
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => return Ok(events),
        }
    }
}

/// A predecessor lookup decodes only its examined candidates, including rejected
/// audit receipts. In particular, historical gate attempts cannot fill a page of
/// needs-me context or vice versa.
fn last(
    ctx: &Ctx,
    query: &EventQuery,
    kinds: &[&str],
    accepts: impl Fn(&Event) -> bool,
    budget: &mut Budget,
) -> Result<Option<Event>> {
    if budget.exhausted() {
        return Ok(None);
    }
    let view = ctx.store.view()?;
    let query = EventQuery {
        kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
        newest_first: true,
        ..query.clone()
    };
    let mut page = PageReq::first(1);
    loop {
        let found = view.events(&query, page.clone())?;
        if !budget.admit(found.items.len()) {
            return Ok(None);
        }
        for row in found.items {
            let event = project(row)?;
            budget.observe(event.ts);
            if accepts(&event) {
                return Ok(Some(event));
            }
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => return Ok(None),
        }
    }
}

pub(super) fn dispatch(
    ctx: &Ctx,
    task: &str,
    from: Timestamp,
    budget: &mut Budget,
) -> Result<Option<Event>> {
    last(
        ctx,
        &query(Some(task), None, from),
        &["dispatched"],
        |_| true,
        budget,
    )
}

fn effective(event: &Event) -> bool {
    !matches!(event.kind.as_str(), "replied" | "changes_requested")
        || event.detail["submitted"] != false
}

/// Shared interrupts reset every run phase. The last dispatch before `from`
/// interrupts earlier runs, so no state before it can cross the boundary.
/// Within that dispatch window:
/// - active/gate/review need only their last effective transition;
/// - accept/landing keep the first opener after their last closing transition;
/// - needs-me retains its decision receipts since its last unconditional close,
///   because one answered decision does not necessarily resume the worker.
///
/// These are real event records, sorted back into original `(ts, seq)` order.
pub(super) fn context(
    ctx: &Ctx,
    task: &str,
    run: &str,
    include_runless: bool,
    floor: Option<Timestamp>,
    from: Timestamp,
    budget: &mut Budget,
) -> Result<Vec<Event>> {
    let query = EventQuery {
        run: Some(run.to_owned()),
        include_runless,
        ..query(Some(task), floor, from)
    };
    let mut found = BTreeMap::new();
    for (kinds, phase) in [
        (
            &[
                "dispatched",
                "changes_requested",
                "done",
                "discarded",
                "superseded",
            ][..],
            "active",
        ),
        (
            &[
                "gate_started",
                "gate",
                "gate_cancelled",
                "review_released",
                "discarded",
                "superseded",
            ][..],
            "gate",
        ),
        (
            &[
                "review_started",
                "verdict",
                "review_cancelled",
                "review_released",
                "changes_requested",
                "approved",
                "discarded",
                "superseded",
            ][..],
            "review",
        ),
    ] {
        if let Some(event) = last(
            ctx,
            &query,
            kinds,
            |event| {
                effective(event)
                    && match event.kind.as_str() {
                        "review_released" => {
                            event.detail["kind"]
                                == if phase == "gate" { "gate" } else { "reviewer" }
                        }
                        "gate" => matches!(
                            event.detail["result"].as_str(),
                            Some("pass" | "fail" | "stale" | "cancelled")
                        ),
                        _ => true,
                    }
            },
            budget,
        )? {
            found.insert(event.seq, event);
        }
    }
    for landing in [false, true] {
        let close = last(
            ctx,
            &query,
            &[
                "changes_requested",
                "approved",
                "autopilot",
                "landing_stale",
                "discarded",
                "superseded",
            ],
            |event| {
                effective(event)
                    && match event.kind.as_str() {
                        "autopilot" => {
                            matches!(event.detail["to"].as_str(), Some("changes" | "escalated"))
                                || (!landing && event.detail["to"] == "landing")
                        }
                        "landing_stale" => !landing,
                        _ => true,
                    }
            },
            budget,
        )?;
        let open_query = EventQuery {
            since: close.as_ref().map(|event| event.ts).or(floor),
            kinds: if landing {
                vec!["autopilot".into(), "landing_stale".into()]
            } else {
                vec!["autopilot".into()]
            },
            ..query.clone()
        };
        // get_or_insert phases retain the first opener, not the most recent one.
        let view = ctx.store.view()?;
        let mut page = PageReq::first(1);
        loop {
            let events = view.events(&open_query, page.clone())?;
            if !budget.admit(events.items.len()) {
                break;
            }
            let mut opened = false;
            for row in events.items {
                let event = project(row)?;
                budget.observe(event.ts);
                if close
                    .as_ref()
                    .is_some_and(|close| (event.ts, event.seq) <= (close.ts, close.seq))
                {
                    continue;
                }
                let opens = if landing {
                    event.kind == "landing_stale" || event.detail["to"] == "landing"
                } else {
                    event.detail["to"] == "reviewed"
                };
                if opens {
                    found.insert(event.seq, event);
                    opened = true;
                    break;
                }
            }
            if opened {
                break;
            }
            match events.next {
                Some(next) => page.after = Some(next),
                None => break,
            }
        }
        if let Some(event) = close {
            found.insert(event.seq, event);
        }
    }
    let close = last(
        ctx,
        &query,
        &["resumed", "done", "discarded", "superseded"],
        |_| true,
        budget,
    )?;
    let needs_query = EventQuery {
        since: close.as_ref().map(|event| event.ts).or(floor),
        kinds: [
            "needs_me",
            "resumed",
            "replied",
            "run_resumed",
            "reply_marked_delivered",
            "done",
            "discarded",
            "superseded",
        ]
        .map(str::to_owned)
        .into(),
        ..query
    };
    for event in read(ctx, &needs_query, budget)? {
        if close
            .as_ref()
            .is_none_or(|close| (event.ts, event.seq) >= (close.ts, close.seq))
        {
            found.insert(event.seq, event);
        }
    }
    Ok(found.into_values().collect())
}

pub(super) fn phase_window(
    ctx: &Ctx,
    task: &str,
    from: Timestamp,
    to: Timestamp,
    budget: &mut Budget,
) -> Result<Vec<Event>> {
    let query = EventQuery {
        kinds: PHASE_EVENTS.iter().map(|kind| (*kind).to_owned()).collect(),
        ..query(Some(task), Some(from), to)
    };
    read(ctx, &query, budget)
}

/// Queue is task-level and first-in/first-out even when `queued` is repeated.
pub(super) fn queue(
    ctx: &Ctx,
    task: &str,
    from: Timestamp,
    budget: &mut Budget,
) -> Result<Vec<Event>> {
    let query = query(Some(task), None, from);
    let close = last(
        ctx,
        &query,
        &["dispatched", "dequeued", "discarded"],
        |_| true,
        budget,
    )?;
    let query = EventQuery {
        kinds: vec!["queued".into()],
        since: close.as_ref().map(|event| event.ts),
        ..query
    };
    let view = ctx.store.view()?;
    let mut page = PageReq::first(1);
    loop {
        let found = view.events(&query, page.clone())?;
        if !budget.admit(found.items.len()) {
            return Ok(Vec::new());
        }
        for row in found.items {
            let event = project(row)?;
            budget.observe(event.ts);
            if close
                .as_ref()
                .is_none_or(|close| (event.ts, event.seq) > (close.ts, close.seq))
            {
                return Ok(vec![event]);
            }
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => return Ok(Vec::new()),
        }
    }
}

pub(super) fn run_rows(
    ctx: &Ctx,
    task: &TaskHeader,
    range: Option<(Timestamp, Timestamp)>,
    events: &[Event],
    runs: &mut BTreeMap<String, RunHead>,
    budget: &mut Budget,
) -> Result<()> {
    if budget.exhausted() {
        return Ok(());
    }
    let view = ctx.store.view()?;
    let mut queries = Vec::new();
    if let Some(range) = range {
        queries.push(RowQuery::part(TaskPart::Run).current());
        for part in [TaskPart::Run, TaskPart::ReviewRun] {
            let mut query = RowQuery::part(part);
            query.overlaps = Some(range);
            queries.push(query);
        }
    }
    for run in events
        .iter()
        .filter_map(|event| event.detail["run"].as_str())
        .collect::<BTreeSet<_>>()
    {
        if runs.values().any(|known| known.id == run) {
            continue;
        }
        queries.push(RowQuery::part(TaskPart::Run).with_index(format!("id:{run}")));
    }
    // A review that crosses the boundary can belong to a session which already
    // ended. Select its actual child row at the recorded phase start as well.
    for event in events.iter().filter(|event| event.kind == "review_started") {
        let mut query = RowQuery::part(TaskPart::ReviewRun);
        if let Some(parent) = event.detail["run"]
            .as_str()
            .and_then(|id| {
                runs.iter()
                    .find(|(key, run)| !key.contains('/') && run.id == id)
            })
            .map(|(key, _)| format!("{key}/"))
        {
            if runs.iter().any(|(key, run)| {
                key.starts_with(&parent)
                    && run.started <= event.ts
                    && run.ended.is_none_or(|end| end >= event.ts)
            }) {
                continue;
            }
            query.prefix = Some(parent);
        }
        query.overlaps = Some((
            event.ts,
            event.ts.checked_add(jiff::SignedDuration::from_nanos(1))?,
        ));
        queries.push(query);
    }
    for query in queries {
        if let Some(token) = &query.index
            && let Some(id) = token.strip_prefix("id:")
            && runs.values().any(|run| run.id == id)
        {
            continue;
        }
        let mut page = PageReq::first(PAGE);
        loop {
            let found = view
                .rows(task.num, &query, page.clone())
                .context("reading recorded timeline run window")?;
            #[cfg(test)]
            super::tests::record_run_rows(found.items.len());
            if !budget.admit(found.items.len()) {
                return Ok(());
            }
            for row in found.items {
                let run: RunHead =
                    serde_json::from_value(row.doc).context("decoding timeline run")?;
                budget.observe(run.started);
                runs.insert(row.key, run);
            }
            match found.next {
                Some(next) => page.after = Some(next),
                None => break,
            }
        }
    }
    // A child session may outlive the worker. Its parent is still the source of
    // gate/landing receipts and the run identity used by phase events.
    let parents: BTreeSet<_> = runs
        .keys()
        .filter_map(|key| key.split_once('/').map(|(key, _)| key.to_owned()))
        .collect();
    for key in parents {
        if runs.contains_key(&key) {
            continue;
        }
        let found = view.rows(
            task.num,
            &RowQuery::key(TaskPart::Run, key),
            PageReq::first(1),
        )?;
        #[cfg(test)]
        super::tests::record_run_rows(found.items.len());
        if !budget.admit(found.items.len()) {
            return Ok(());
        }
        for row in found.items {
            runs.insert(row.key, serde_json::from_value(row.doc)?);
        }
    }
    Ok(())
}
