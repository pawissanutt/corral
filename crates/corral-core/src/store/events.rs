//! Database event projection and callback-local buffering. The unit is keyed by backend
//! identity and thread, so reopened handles join it while other threads remain independent.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};

use super::Store;
use crate::backend::{
    AppendEvents, EventQuery, EventSeq, NewEvent, Operation, PageReq, StoredEvent,
};

use crate::Event;

struct Unit {
    db: Arc<super::db::Db>,
    events: RefCell<Vec<NewEvent>>,
}

thread_local! {
    static UNITS: RefCell<Vec<Rc<Unit>>> = const { RefCell::new(Vec::new()) };
}

/// Dropping a unit discards uncommitted events, including on callback error or unwind.
pub(super) struct EventUnit(Option<Rc<Unit>>);

impl Drop for EventUnit {
    fn drop(&mut self) {
        if let Some(owned) = &self.0 {
            UNITS.with(|units| units.borrow_mut().retain(|unit| !Rc::ptr_eq(unit, owned)));
        }
    }
}

pub(super) fn new_event(event: Event) -> Result<NewEvent> {
    Ok(NewEvent {
        ts: event.ts,
        task: event.task,
        kind: event.kind,
        actor: event
            .actor
            .map(serde_json::to_value)
            .transpose()?
            .and_then(|actor| actor.as_str().map(str::to_owned)),
        via: event.via,
        run: event
            .detail
            .get("run")
            .and_then(|run| run.as_str())
            .map(str::to_owned),
        detail: event.detail,
    })
}

pub(crate) fn projected(stored: StoredEvent) -> Result<Option<Event>> {
    if stored.malformed {
        return Ok(None);
    }
    let event = stored.event;
    Ok(Some(Event {
        seq: stored.seq.0,
        ts: event.ts,
        task: event.task,
        kind: event.kind,
        actor: event
            .actor
            .map(|actor| serde_json::from_value(actor.into()))
            .transpose()?,
        via: event.via,
        detail: event.detail,
    }))
}

impl Store {
    pub(super) fn event_unit(&self) -> EventUnit {
        if self.current_unit().is_some() {
            return EventUnit(None);
        }
        let unit = Rc::new(Unit {
            db: Arc::clone(&self.db),
            events: RefCell::default(),
        });
        UNITS.with(|units| units.borrow_mut().push(Rc::clone(&unit)));
        EventUnit(Some(unit))
    }

    fn current_unit(&self) -> Option<Rc<Unit>> {
        UNITS.with(|units| {
            units
                .borrow()
                .iter()
                .rev()
                .find(|unit| Arc::ptr_eq(&unit.db, &self.db))
                .cloned()
        })
    }

    pub(super) fn buffered_events(&self) -> Vec<NewEvent> {
        self.current_unit().map_or_else(Vec::new, |unit| {
            std::mem::take(&mut *unit.events.borrow_mut())
        })
    }

    /// Business-rule validation sees its own staged writes without publishing them to
    /// readers or draining the unit before its eventual commit.
    pub(crate) fn staged_events(&self, task: &str, kinds: &[&str]) -> Result<Vec<Event>> {
        let Some(unit) = self.current_unit() else {
            return Ok(Vec::new());
        };
        unit.events
            .borrow()
            .iter()
            .filter(|event| {
                event.task.as_deref() == Some(task) && kinds.contains(&event.kind.as_str())
            })
            .filter_map(|event| {
                projected(StoredEvent {
                    seq: EventSeq(0),
                    event: event.clone(),
                    malformed: false,
                })
                .transpose()
            })
            .collect()
    }

    pub(super) fn append_database_event(&self, event: Event) -> Result<()> {
        anyhow::ensure!(
            !self.is_snapshot(),
            "a read-only snapshot cannot append events"
        );
        let event = new_event(event)?;
        if let Some(unit) = self.current_unit() {
            unit.events.borrow_mut().push(event);
            return Ok(());
        }
        self.db
            .backend
            .apply(Operation::AppendEvents(AppendEvents {
                events: vec![event],
                reservations: Vec::new(),
            }))
            .map_err(super::write::apply_error)
            .context("appending event")?;
        Ok(())
    }

    /// An index-served page in event sequence order. Only returned rows are decoded.
    pub fn events_page(
        &self,
        task: Option<&str>,
        kinds: &[&str],
        run: Option<&str>,
        after: u64,
        limit: u32,
    ) -> Result<Vec<Event>> {
        let view = self.view()?;
        let query = EventQuery {
            after: Some(EventSeq(after)),
            task: task.map(str::to_owned),
            kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
            run: run.map(str::to_owned),
            newest_first: false,
            valid_only: true,
        };
        view.events(&query, PageReq::first(limit))?
            .items
            .into_iter()
            .filter_map(|row| projected(row).transpose())
            .collect()
    }

    /// Run-filtered event page, strictly after `after`.
    pub fn events_of_run(
        &self,
        task: Option<&str>,
        kinds: &[&str],
        run: Option<&str>,
        after: u64,
        limit: u32,
    ) -> Result<Vec<Event>> {
        self.events_page(task, kinds, run, after, limit)
    }

    pub(super) fn database_events(&self, task: Option<&str>, kinds: &[&str]) -> Result<Vec<Event>> {
        let view = self.view()?;
        collect_events(view.as_ref(), task, kinds)
    }

    /// Current event tail, read through the event sequence index, not the change feed.
    pub fn event_tail(&self) -> Result<u64> {
        let page = self.view()?.events(
            &EventQuery {
                newest_first: true,
                ..EventQuery::default()
            },
            PageReq::first(1),
        )?;
        Ok(page.items.first().map_or(0, |event| event.seq.0))
    }
}

pub(super) fn collect_events(
    view: &dyn crate::backend::ReadView,
    task: Option<&str>,
    kinds: &[&str],
) -> Result<Vec<Event>> {
    let mut query = EventQuery {
        task: task.map(str::to_owned),
        kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
        valid_only: true,
        ..EventQuery::default()
    };
    let mut events = Vec::new();
    loop {
        let page = view.events(&query, PageReq::first(500))?;
        let last = page.items.last().map(|row| row.seq);
        for row in page.items {
            if let Some(event) = projected(row)? {
                events.push(event);
            }
        }
        match (last, page.next) {
            (Some(last), Some(_)) => query.after = Some(last),
            _ => break,
        }
    }
    Ok(events)
}
