//! Store-owned database readers and portable task rendering.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;

use super::Store;
use crate::backend::{EventQuery, PageReq};
use crate::event_reader::{EventCursor, EventReader};
use crate::legacy::{parse_task, parse_task_record, render_task, render_task_record};
use crate::model::{Event, Task};

impl Store {
    /// Every event of one task, oldest first.
    pub fn events_for(&self, task: &str) -> Result<Vec<Event>> {
        self.database_events(Some(task), &[])
    }

    /// Events of the given kinds, oldest first; `task` narrows them to one task.
    pub fn events_of(&self, task: Option<&str>, kinds: &[&str]) -> Result<Vec<Event>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        self.database_events(task, kinds)
    }

    /// Events without loading configuration, including when the config does not parse.
    pub fn events_of_at(root: &Path, task: Option<&str>, kinds: &[&str]) -> Result<Vec<Event>> {
        let db = super::db::open(&root.join(".corral"))?;
        let view = db.backend.view()?;
        super::events::collect_events(view.as_ref(), task, kinds)
    }

    /// A bounded follower after the persisted sequence; `None` starts at the current tail.
    pub fn events_since(&self, after: Option<EventCursor>) -> Result<EventReader> {
        EventReader::database(self, after)
    }

    /// Database documents have no legacy line limit. The argument remains for callers
    /// importing their previous file-reader policy.
    pub fn events_since_bounded(
        &self,
        after: Option<EventCursor>,
        _max_line_bytes: usize,
    ) -> Result<EventReader> {
        self.events_since(after)
    }

    /// Newest complete events fitting in `max_bytes` of serialized history, oldest first.
    pub fn recent_events(&self, max_bytes: u64) -> Result<Vec<Event>> {
        let view = self.view()?;
        let query = EventQuery {
            newest_first: true,
            ..Default::default()
        };
        let mut page = PageReq::first(64);
        let mut bytes = 0_u64;
        let mut events = Vec::new();
        loop {
            let found = view.events(&query, page.clone())?;
            for stored in found.items {
                bytes =
                    bytes.saturating_add(u64::try_from(serde_json::to_vec(&stored.event)?.len())?);
                if bytes > max_bytes {
                    events.reverse();
                    return Ok(events);
                }
                if let Some(event) = super::events::projected(stored)? {
                    events.push(event);
                }
            }
            match found.next {
                Some(next) => page.after = Some(next),
                None => break,
            }
        }
        events.reverse();
        Ok(events)
    }

    /// Opaque event-stream identity.
    pub fn events_stamp(&self) -> Result<String> {
        Ok(format!("event:{}", self.event_tail()?))
    }

    /// Opaque identity of every stored task.
    pub fn tasks_stamp(&self) -> Result<String> {
        Ok(format!("seq:{}", self.reader()?.seq().0))
    }

    /// Task numbers already taken.
    pub fn reserved_task_numbers(&self) -> Result<BTreeSet<u64>> {
        let reader = self.reader()?;
        Ok(Self::headers_on(&*reader)?
            .iter()
            .map(|header| header.num.0)
            .collect())
    }

    /// The portable Markdown form of a task.
    pub fn render_task_markdown(task: &Task) -> String {
        render_task(task)
    }

    /// Parses portable Markdown without a stored location.
    pub fn parse_task_markdown(text: &str) -> Result<Task> {
        parse_task(text, Path::new(""))
    }

    /// Portable Markdown including recovery evidence.
    pub fn render_task_record_markdown(record: &crate::TaskRecord) -> Result<String> {
        render_task_record(record)
    }

    /// Parses portable Markdown without losing recovery evidence.
    pub fn parse_task_record_markdown(text: &str) -> Result<crate::TaskRecord> {
        parse_task_record(text, Path::new(""))
    }
}
