//! Indexed event selections and scalar run-overlap predicates.
use std::collections::BTreeSet;

use rusqlite::types::Value as Sql;

use super::{codec::*, load};
use crate::backend::{EventQuery, PageReq, StoredEvent, ViewError};

pub(super) const EVENT_INDEXES: &str = "
CREATE INDEX IF NOT EXISTS events_time ON events(ts, seq);
CREATE INDEX IF NOT EXISTS events_kind_time ON events(kind, ts, seq);
CREATE INDEX IF NOT EXISTS events_task_time ON events(task, ts, seq);
CREATE INDEX IF NOT EXISTS events_task_kind_time ON events(task, kind, ts, seq);
CREATE INDEX IF NOT EXISTS events_task_run_kind_time ON events(task, run, kind, ts, seq);";

// RunHead timestamps serialize as UTC RFC3339. Pad the optional fractional part to
// the same nine digits as codec::ts, including integral-second timestamps. No JSON
// document is decoded into Rust to select a run, and nanoseconds are not rounded.
pub(super) fn run_time(field: &str) -> String {
    let value = format!("json_extract(doc, '$.{field}')");
    format!(
        "(substr({value}, 1, 19) || '.' || \
         substr(rtrim(substr({value}, 21), 'Z') || '000000000', 1, 9) || 'Z')"
    )
}

pub(super) fn run_end() -> String {
    format!(
        "coalesce({}, '9999-12-31T23:59:59.999999999Z')",
        run_time("ended")
    )
}

pub(super) fn run_index() -> String {
    format!(
        "CREATE INDEX IF NOT EXISTS task_rows_run_time ON task_rows(task, part, {}, {}, key) \
         WHERE part IN ('run', 'review_run');",
        run_end(),
        run_time("started")
    )
}

/// Every index these queries name, as schema setup creates them.
pub fn indexes() -> String {
    format!("{EVENT_INDEXES}\n{}", run_index())
}

/// Merge at most one page per kind, not an unbounded lifetime sort. Sequence-only
/// callers never enter this path; their existing ordering/index choices are intact.
pub(super) fn events(q: &EventQuery, page: &PageReq) -> Result<(String, Vec<Sql>), ViewError> {
    let cursor = if q.chronological { after(page)? } else { None };
    let source_page = if q.chronological {
        PageReq::first(page.limit)
    } else {
        page.clone()
    };
    let (offset, limit) = super::view::bounds(&source_page)?;
    let take = offset
        .checked_add(limit)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| ViewError::Io(anyhow::anyhow!("event page overflow")))?;
    let kinds: BTreeSet<_> = q.kinds.iter().map(String::as_str).collect();
    let kinds: Vec<_> = if kinds.is_empty() {
        vec![None]
    } else {
        kinds.into_iter().map(Some).collect()
    };
    let direction = if q.newest_first { "DESC" } else { "ASC" };
    let order = if q.chronological {
        format!("ts {direction}, seq {direction}")
    } else {
        format!("seq {direction}")
    };
    let mut selects = Vec::new();
    let mut args = Vec::new();
    for kind in kinds {
        for runless in [false, true]
            .into_iter()
            .take(if q.include_runless && q.run.is_some() {
                2
            } else {
                1
            })
        {
            let mut filters = Vec::new();
            if q.valid_only {
                filters.push("malformed = 0".to_owned());
            }
            if let Some(task) = &q.task {
                filters.push("task = ?".to_owned());
                args.push(Sql::Text(task.clone()));
            }
            if let Some(kind) = kind {
                filters.push("kind = ?".to_owned());
                args.push(Sql::Text(kind.to_owned()));
            }
            if runless {
                filters.push("run IS NULL".to_owned());
            } else if let Some(run) = &q.run {
                filters.push("run = ?".to_owned());
                args.push(Sql::Text(run.clone()));
            }
            if let Some(at) = q.since {
                filters.push("ts >= ?".to_owned());
                args.push(Sql::Text(ts(at)));
            }
            if let Some(at) = q.until {
                filters.push("ts < ?".to_owned());
                args.push(Sql::Text(ts(at)));
            }
            if let Some(seq) = q.after {
                filters.push("seq > ?".to_owned());
                args.push(Sql::Integer(int(seq.0).or_view()?));
            }
            if let Some(seq) = q.before {
                filters.push("seq < ?".to_owned());
                args.push(Sql::Integer(int(seq.0).or_view()?));
            }
            if let Some((at, seq)) = cursor {
                filters.push(format!(
                    "(ts, seq) {} (?, ?)",
                    if q.newest_first { "<" } else { ">" }
                ));
                args.push(Sql::Text(ts(at)));
                args.push(Sql::Integer(int(seq).or_view()?));
            }
            let index = match (q.task.is_some(), kind.is_some(), q.run.is_some()) {
                (true, true, true) => "events_task_run_kind_time",
                (true, true, false) => "events_task_kind_time",
                (true, false, _) => "events_task_time",
                (false, true, _) => "events_kind_time",
                (false, false, _) => "events_time",
            };
            selects.push(format!(
                "SELECT * FROM (SELECT {} FROM events INDEXED BY {index} WHERE {} \
             ORDER BY {order} LIMIT {take})",
                load::EVENT_COLUMNS,
                if filters.is_empty() {
                    "1".to_owned()
                } else {
                    filters.join(" AND ")
                }
            ));
        }
    }
    Ok((
        format!(
            "SELECT * FROM ({}) ORDER BY {order}",
            selects.join(" UNION ALL ")
        ),
        args,
    ))
}

pub(super) fn cursor(event: &StoredEvent) -> String {
    format!("time:{}|{}", ts(event.event.ts), event.seq.0)
}

fn after(page: &PageReq) -> Result<Option<(jiff::Timestamp, u64)>, ViewError> {
    let Some(cursor) = page.after.as_deref() else {
        return Ok(None);
    };
    let (at, seq) = cursor
        .strip_prefix("time:")
        .and_then(|cursor| cursor.split_once('|'))
        .ok_or_else(|| ViewError::Io(anyhow::anyhow!("invalid chronological event cursor")))?;
    Ok(Some((
        at.parse::<jiff::Timestamp>()
            .map_err(|error| ViewError::Io(error.into()))?,
        seq.parse::<u64>()
            .map_err(|error| ViewError::Io(error.into()))?,
    )))
}

/// Preserve sequence-only queries, bounding each kind before the ordered merge.
pub(super) fn sequence_events(
    query: &EventQuery,
    page: &PageReq,
) -> Result<(String, Vec<Sql>), ViewError> {
    let kinds: BTreeSet<_> = query.kinds.iter().map(String::as_str).collect();
    let order = if query.newest_first { "DESC" } else { "ASC" };
    if kinds.len() > 1 {
        let (offset, limit) = super::view::bounds(page)?;
        let take = offset
            .checked_add(limit)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| ViewError::Io(anyhow::anyhow!("event page overflow")))?;
        let mut selects = Vec::new();
        let mut args = Vec::new();
        for kind in kinds {
            let (filters, values) = sequence_filters(query, Some(kind))?;
            args.extend(values);
            selects.push(format!(
                "SELECT * FROM (SELECT {} FROM events{filters} ORDER BY seq {order} LIMIT {take})",
                load::EVENT_COLUMNS
            ));
        }
        return Ok((
            format!(
                "SELECT * FROM ({}) ORDER BY seq {order}",
                selects.join(" UNION ALL ")
            ),
            args,
        ));
    }
    let (filters, args) = sequence_filters(query, kinds.into_iter().next())?;
    Ok((
        format!(
            "SELECT {} FROM events{filters} ORDER BY seq {order}",
            load::EVENT_COLUMNS
        ),
        args,
    ))
}

fn sequence_filters(
    query: &EventQuery,
    kind: Option<&str>,
) -> Result<(String, Vec<Sql>), ViewError> {
    let mut filters = Vec::new();
    let mut args = Vec::new();
    if query.valid_only {
        filters.push("malformed = 0");
    }
    if let Some(after) = query.after {
        filters.push("seq > ?");
        args.push(Sql::Integer(int(after.0).or_view()?));
    }
    if let Some(before) = query.before {
        filters.push("seq < ?");
        args.push(Sql::Integer(int(before.0).or_view()?));
    }
    if let Some(task) = &query.task {
        filters.push("task = ?");
        args.push(Sql::Text(task.clone()));
    }
    if let Some(kind) = kind {
        filters.push("kind = ?");
        args.push(Sql::Text(kind.to_owned()));
    }
    if let Some(run) = &query.run {
        filters.push(if query.include_runless {
            "(run = ? OR run IS NULL)"
        } else {
            "run = ?"
        });
        args.push(Sql::Text(run.clone()));
    }
    let clause = if filters.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", filters.join(" AND "))
    };
    Ok((clause, args))
}
