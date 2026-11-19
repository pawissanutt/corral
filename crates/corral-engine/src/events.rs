use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
pub use corral_types::{Event, EventKind};
use jiff::Timestamp;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    model::Actor,
    store::{Store, encode},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub task: Option<String>,
    pub run: Option<String>,
    pub kind: EventKind,
    pub actor: Actor,
    pub detail: Value,
}

#[derive(Debug, Default, Clone)]
pub struct EventFilter {
    pub kinds: Vec<EventKind>,
    pub task: Option<String>,
    pub project: Option<String>,
}

impl Store {
    pub fn latest_seq(&self) -> Result<u64> {
        self.connection
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
                row.get::<_, i64>(0)
            })
            .context("read newest event sequence")?
            .try_into()
            .context("event sequence must not be negative")
    }

    /// Whether the pipeline announced readiness for the current submission.
    pub fn has_ready_since_done(&self, task: &str) -> Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE task=?1 AND kind='ready' AND seq > COALESCE((SELECT MAX(seq) FROM events WHERE task=?1 AND kind='done'),0))",
            [task], |row| row.get(0),
        ).context("check whether readiness was announced")
    }

    /// Review rounds can be reused; ignore a verdict from a previous submission.
    pub fn has_verdict_since_done(&self, task: &str, round: u32) -> Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE task=?1 AND kind='verdict' AND json_extract(detail,'$.round')=?2 AND seq > COALESCE((SELECT MAX(seq) FROM events WHERE task=?1 AND kind='done'),0))",
            params![task, round], |row| row.get(0),
        ).context("check whether the current submission received a verdict")
    }

    pub(crate) fn has_review_request_since_done(
        &self,
        task: &str,
        run: &str,
        round: u32,
    ) -> Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE task=?1 AND run=?2 AND kind='review_started' AND json_extract(detail,'$.round')=?3 AND seq > COALESCE((SELECT MAX(seq) FROM events WHERE task=?1 AND kind='done'),0))",
            params![task, run, round], |row| row.get(0),
        ).context("check whether this reviewer was asked to review the current submission")
    }

    /// Requires the caller's transaction; prevents a row/event split commit.
    pub fn append_event(&self, event: &NewEvent) -> Result<Event> {
        self.append_event_at(event, Timestamp::now())
    }

    /// Records an event that happened at `ts`, for history brought in from another store.
    /// Sequence order stays insertion order, so append such events oldest first.
    pub fn append_event_at(&self, event: &NewEvent, ts: Timestamp) -> Result<Event> {
        ensure!(
            !self.connection.is_autocommit(),
            "append_event requires an engine transaction"
        );
        self.connection
            .execute(
                "INSERT INTO events(ts,task,run,kind,actor,detail) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    ts.to_string(),
                    event.task,
                    event.run,
                    event.kind.as_str(),
                    encode(&event.actor)?,
                    encode(&event.detail)?
                ],
            )
            .context("append engine event")?;
        Ok(Event {
            seq: self.connection.last_insert_rowid() as u64,
            ts,
            task: event.task.clone(),
            project: event
                .task
                .as_deref()
                .map(|id| self.get_task(id))
                .transpose()?
                .flatten()
                .and_then(|task| task.project),
            run: event.run.clone(),
            kind: event.kind,
            actor: event.actor,
            detail: event.detail.clone(),
        })
    }

    pub fn list_events(&self, task: &str) -> Result<Vec<Event>> {
        self.events_after(
            &EventFilter {
                task: Some(task.to_owned()),
                ..Default::default()
            },
            0,
        )
    }

    /// Exclusive sequence bounds. `after` without `before` can walk forward;
    /// history pages walk backwards from a `before` cursor or the head.
    pub fn event_page(
        &self,
        task: Option<&str>,
        after: Option<u64>,
        before: Option<u64>,
        newest_first: bool,
        limit: u16,
    ) -> Result<Vec<Event>> {
        let lower = after.unwrap_or(0).saturating_add(1);
        let upper = before.map_or(i64::MAX as u64, |seq| seq.saturating_sub(1));
        let Ok(lower) = i64::try_from(lower) else {
            return Ok(Vec::new());
        };
        let upper = i64::try_from(upper).unwrap_or(i64::MAX);
        let order = if newest_first { "DESC" } else { "ASC" };
        let sql = if task.is_some() {
            format!(
                "SELECT events.seq,events.ts,events.task,events.run,events.kind,events.actor,events.detail,tasks.project FROM events LEFT JOIN tasks ON tasks.id=events.task WHERE events.seq BETWEEN ?1 AND ?2 AND events.task=?4 ORDER BY events.seq {order} LIMIT ?3"
            )
        } else {
            format!(
                "SELECT events.seq,events.ts,events.task,events.run,events.kind,events.actor,events.detail,tasks.project FROM events LEFT JOIN tasks ON tasks.id=events.task WHERE events.seq BETWEEN ?1 AND ?2 ORDER BY events.seq {order} LIMIT ?3"
            )
        };
        let mut statement = self
            .connection
            .prepare_cached(&sql)
            .context("prepare event page")?;
        let rows = if let Some(task) = task {
            statement.query(params![lower, upper, limit, task])
        } else {
            statement.query(params![lower, upper, limit])
        }
        .context("query event page")?;
        read_events(rows)
    }

    pub fn event_count_after(&self, after: u64) -> Result<u64> {
        let Ok(after) = i64::try_from(after) else {
            return Ok(0);
        };
        self.connection
            .query_row("SELECT count(*) FROM events WHERE seq>?1", [after], |row| {
                row.get::<_, i64>(0)
            })
            .context("count engine events")?
            .try_into()
            .context("event count must not be negative")
    }

    /// The diff registered by `done` for the specified worker's last submission.
    pub fn submitted_diff_ref(&self, task: &str, run: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.connection
            .query_row(
                "SELECT json_extract(detail,'$.reference') FROM events
             WHERE task=?1 AND run=?2 AND kind='artifact' AND json_extract(detail,'$.kind')='diff'
             ORDER BY seq DESC LIMIT 1",
                params![task, run],
                |row| row.get(0),
            )
            .optional()
            .context("read submitted diff reference")
    }

    pub fn events_in_window(&self, from: Timestamp, to: Timestamp) -> Result<Vec<Event>> {
        let from = from.to_string();
        let to = to.to_string();
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT events.seq,events.ts,events.task,events.run,events.kind,events.actor,events.detail,tasks.project FROM events LEFT JOIN tasks ON tasks.id=events.task
             WHERE rtrim(events.ts,'Z')>=?1 AND rtrim(events.ts,'Z')<?2 ORDER BY events.seq",
            )
            .context("prepare timeline events")?;
        read_events(
            statement
                .query(params![
                    from.trim_end_matches('Z'),
                    to.trim_end_matches('Z')
                ])
                .context("query timeline events")?,
        )
    }

    pub fn events_after(&self, filter: &EventFilter, after_seq: u64) -> Result<Vec<Event>> {
        self.query_events(filter, after_seq, -1)
    }

    fn query_events(&self, filter: &EventFilter, after_seq: u64, limit: i64) -> Result<Vec<Event>> {
        let Ok(after_seq) = i64::try_from(after_seq) else {
            return Ok(Vec::new());
        };
        let kinds = encode(&filter.kinds)?;
        let mut statement = self.connection.prepare("SELECT events.seq,events.ts,events.task,events.run,events.kind,events.actor,events.detail,tasks.project FROM events LEFT JOIN tasks ON tasks.id=events.task WHERE events.seq>?1 AND (?2 IS NULL OR events.task=?2) AND (?3 IS NULL OR tasks.project=?3) AND (?4='[]' OR events.kind IN (SELECT value FROM json_each(?4))) ORDER BY events.seq LIMIT ?5").context("prepare event listing")?;
        let rows = statement
            .query(params![
                after_seq,
                filter.task,
                filter.project,
                kinds,
                limit
            ])
            .context("query engine events")?;
        read_events(rows)
    }

    /// Returns the first matching event, or None when the timeout expires.
    pub fn wait(
        &self,
        filter: &EventFilter,
        after_seq: u64,
        timeout: Duration,
    ) -> Result<Option<Event>> {
        let started = Instant::now();
        loop {
            if let Some(event) = self.query_events(filter, after_seq, 1)?.into_iter().next() {
                return Ok(Some(event));
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Ok(None);
            }
            thread::sleep(remaining.min(Duration::from_millis(25)));
        }
    }
}

fn read_events(mut rows: rusqlite::Rows<'_>) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    while let Some(row) = rows.next().context("read engine event row")? {
        let kind: String = row.get(4).context("read event kind")?;
        let kind: EventKind =
            serde_json::from_value(Value::String(kind)).context("decode event kind")?;
        let ts: String = row.get(1).context("read event timestamp")?;
        let actor: String = row.get(5).context("read event actor")?;
        let detail: String = row.get(6).context("read event detail")?;
        events.push(Event {
            seq: row
                .get::<_, i64>(0)
                .context("read event sequence")?
                .try_into()
                .context("event sequence must not be negative")?,
            ts: ts.parse().context("parse event timestamp")?,
            task: row.get(2).context("read event task")?,
            project: row.get(7).context("read event project")?,
            run: row.get(3).context("read event run")?,
            kind,
            actor: serde_json::from_str(&actor).context("decode event actor")?,
            detail: serde_json::from_str(&detail).context("decode event detail")?,
        });
    }
    Ok(events)
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
