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

    pub(crate) fn has_ready_since_done(&self, task: &str) -> Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE task=?1 AND kind='ready' AND seq > COALESCE((SELECT MAX(seq) FROM events WHERE task=?1 AND kind='done'),0))",
            [task], |row| row.get(0),
        ).context("check whether readiness was announced")
    }

    pub(crate) fn has_verdict_since_done(&self, task: &str, round: u32) -> Result<bool> {
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
        ensure!(
            !self.connection.is_autocommit(),
            "append_event requires an engine transaction"
        );
        let ts = Timestamp::now();
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

    pub fn events_after(&self, filter: &EventFilter, after_seq: u64) -> Result<Vec<Event>> {
        self.query_events(filter, after_seq, -1)
    }

    fn query_events(&self, filter: &EventFilter, after_seq: u64, limit: i64) -> Result<Vec<Event>> {
        let Ok(after_seq) = i64::try_from(after_seq) else {
            return Ok(Vec::new());
        };
        let kinds = encode(&filter.kinds)?;
        let mut statement = self.connection.prepare("SELECT events.seq,events.ts,events.task,events.run,events.kind,events.actor,events.detail FROM events LEFT JOIN tasks ON tasks.id=events.task WHERE events.seq>?1 AND (?2 IS NULL OR events.task=?2) AND (?3 IS NULL OR tasks.project=?3) AND (?4='[]' OR events.kind IN (SELECT value FROM json_each(?4))) ORDER BY events.seq LIMIT ?5").context("prepare event listing")?;
        let mut rows = statement
            .query(params![
                after_seq,
                filter.task,
                filter.project,
                kinds,
                limit
            ])
            .context("query engine events")?;
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
                run: row.get(3).context("read event run")?,
                kind,
                actor: serde_json::from_str(&actor).context("decode event actor")?,
                detail: serde_json::from_str(&detail).context("decode event detail")?,
            });
        }
        Ok(events)
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

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
