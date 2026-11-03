//! Database event projection and callback-local buffering. The unit is keyed by backend
//! identity and thread, so reopened handles join it while other threads remain independent.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};

use super::Store;
use crate::backend::{
    AppendEvents, Authorization, ConditionWrite, EventQuery, EventSeq, GateAttempt, GateStart,
    NewEvent, Operation, PageReq, PutRecords, RecordWrite, ReservationUse, StoredEvent,
    StoredRecord, TaskChange, UpdateForm, UpdateTask,
};

use crate::Event;

struct Unit {
    db: Arc<super::db::Db>,
    events: RefCell<Vec<NewEvent>>,
    conditions: RefCell<Vec<ConditionWrite>>,
    records: RefCell<Vec<RecordWrite>>,
    reads: RefCell<BTreeMap<(String, String), Option<StoredRecord>>>,
    view: RefCell<Option<super::head::PinnedView>>,
    authorization: RefCell<Option<Authorization>>,
    gate: RefCell<StagedGate>,
}

/// Gate transitions staged into a unit. A start alone makes its task change one `GateStart`;
/// a result makes it one `GateResult`, which then carries a staged start's row (a start that
/// supersedes a lost attempt); with an authorization the start's row joins the authorized
/// change. Blob reservations always join the change.
#[derive(Default)]
pub(super) struct StagedGate {
    pub(super) start: Option<GateAttempt>,
    pub(super) reservations: Vec<ReservationUse>,
    pub(super) result: Option<super::gate_rows::GateOutcome>,
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
    pub(crate) fn bind_session_record(&self, binding: &crate::HerdrSessionBinding) -> Result<()> {
        anyhow::ensure!(!self.is_snapshot(), "a snapshot cannot bind a session");
        self.check_runtime()?;
        self.db
            .backend
            .apply(Operation::BindSession(crate::backend::BindSession {
                socket: binding.socket.to_string_lossy().into_owned(),
                binding: serde_json::to_value(binding).context("encoding session binding")?,
                at: binding.bound_at,
            }))
            .map_err(super::write::apply_error)
            .context("binding herdr session")?;
        Ok(())
    }

    pub(super) fn event_unit(&self) -> EventUnit {
        if self.current_unit().is_some() {
            return EventUnit(None);
        }
        let unit = Rc::new(Unit {
            db: Arc::clone(&self.db),
            events: RefCell::default(),
            conditions: RefCell::default(),
            records: RefCell::default(),
            reads: RefCell::default(),
            view: RefCell::default(),
            authorization: RefCell::default(),
            gate: RefCell::default(),
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

    pub(crate) fn has_record_unit(&self) -> bool {
        self.current_unit().is_some()
    }

    pub(crate) fn record_view(&self) -> Result<Box<dyn crate::backend::ReadView + Send>> {
        let Some(unit) = self.current_unit() else {
            return self.view();
        };
        let mut held = unit.view.borrow_mut();
        let view = match held.as_ref() {
            Some(view) => Arc::clone(view),
            None => {
                let view = Arc::new(std::sync::Mutex::new(self.view()?));
                *held = Some(Arc::clone(&view));
                view
            }
        };
        Ok(Box::new(super::snapshot::SharedView(view)))
    }

    pub(crate) fn held_record(&self, area: &str, key: &str) -> Option<Option<StoredRecord>> {
        self.current_unit()?
            .reads
            .borrow()
            .get(&(area.to_owned(), key.to_owned()))
            .cloned()
    }

    pub(crate) fn hold_record(&self, area: &str, key: &str, row: &Option<StoredRecord>) {
        if let Some(unit) = self.current_unit() {
            unit.reads
                .borrow_mut()
                .entry((area.to_owned(), key.to_owned()))
                .or_insert_with(|| row.clone());
        }
    }

    pub(super) fn buffered_events(&self) -> Vec<NewEvent> {
        self.current_unit().map_or_else(Vec::new, |unit| {
            std::mem::take(&mut *unit.events.borrow_mut())
        })
    }

    /// Conditions wait in the unit and commit inside its task operation.
    pub(super) fn buffer_condition(&self, write: ConditionWrite) -> Result<()> {
        let unit = self
            .current_unit()
            .context("buffering a condition needs an open unit")?;
        unit.conditions.borrow_mut().push(write);
        Ok(())
    }

    pub(super) fn buffered_conditions(&self) -> Vec<ConditionWrite> {
        self.current_unit().map_or_else(Vec::new, |unit| {
            std::mem::take(&mut *unit.conditions.borrow_mut())
        })
    }

    pub(super) fn buffer_events(&self, events: Vec<NewEvent>) -> Result<()> {
        let unit = self
            .current_unit()
            .context("buffering events needs an open unit")?;
        unit.events.borrow_mut().extend(events);
        Ok(())
    }

    /// Callback audits refer to the update being committed, not the previous timestamp the
    /// callback read. Reports join movement and owner-decision audits using this field.
    pub(super) fn buffered_task_events(
        &self,
        task: &str,
        updated: jiff::Timestamp,
    ) -> Result<Vec<NewEvent>> {
        let mut events = self.buffered_events();
        let updated = serde_json::to_value(updated)?;
        for event in &mut events {
            if event.task.as_deref() == Some(task)
                && let Some(stamp) = event.detail.get_mut("task_updated")
            {
                *stamp = updated.clone();
            }
        }
        Ok(events)
    }

    pub(super) fn buffered_records(&self) -> Vec<RecordWrite> {
        self.current_unit().map_or_else(Vec::new, |unit| {
            std::mem::take(&mut *unit.records.borrow_mut())
        })
    }

    pub(super) fn buffer_authorization(&self, authorization: Authorization) -> Result<()> {
        let unit = self
            .current_unit()
            .context("authorization needs an open unit")?;
        let mut slot = unit.authorization.borrow_mut();
        anyhow::ensure!(slot.is_none(), "a unit already has an authorization");
        *slot = Some(authorization);
        Ok(())
    }

    pub(super) fn has_buffered_authorization(&self) -> bool {
        self.current_unit()
            .is_some_and(|unit| unit.authorization.borrow().is_some())
    }

    /// A task unit may publish immutable evidence alongside its task/record writes.
    pub(crate) fn stage_evidence_reservation(&self, reservation: ReservationUse) -> Result<()> {
        self.stage_gate(|stage| {
            stage.reservations.push(reservation);
            Ok(())
        })
    }

    /// Stages gate attempt rows, blob reservations or a result into the open unit; a unit
    /// holds at most one result and never a result together with an authorization.
    pub(super) fn stage_gate(
        &self,
        stage: impl FnOnce(&mut StagedGate) -> Result<()>,
    ) -> Result<()> {
        let unit = self
            .current_unit()
            .context("staging a gate transition needs an open task update")?;
        stage(&mut unit.gate.borrow_mut())?;
        anyhow::ensure!(
            unit.gate.borrow().result.is_none() || unit.authorization.borrow().is_none(),
            "a gate result cannot commit with an authorization"
        );
        Ok(())
    }

    pub(super) fn has_staged_gate(&self) -> bool {
        self.current_unit().is_some_and(|unit| {
            let staged = unit.gate.borrow();
            staged.start.is_some() || staged.result.is_some()
        })
    }

    pub(super) fn task_operation(
        &self,
        mut change: TaskChange,
        form: UpdateForm,
    ) -> Result<Operation> {
        let Some(unit) = self.current_unit() else {
            return Ok(Operation::UpdateTask(UpdateTask { change, form }));
        };
        let staged = std::mem::take(&mut *unit.gate.borrow_mut());
        change.reservations.extend(staged.reservations);
        let authorization = unit.authorization.borrow_mut().take();
        match (authorization, staged.start, staged.result) {
            (Some(_), _, Some(_)) => {
                anyhow::bail!("a gate result cannot commit with an authorization")
            }
            (Some(mut authorization), start, None) => {
                change.rows.extend(
                    start
                        .map(|start| super::gate_rows::start_row(&start))
                        .transpose()?,
                );
                authorization.change = Some((change, form));
                Ok(Operation::Authorization(authorization))
            }
            (None, start, Some(result)) => {
                change.rows.extend(
                    start
                        .map(|start| super::gate_rows::start_row(&start))
                        .transpose()?,
                );
                Ok(Operation::GateResult(result.operation(change, form)))
            }
            (None, Some(attempt), None) => Ok(Operation::GateStart(GateStart {
                change,
                attempt,
                form,
            })),
            (None, None, None) => Ok(Operation::UpdateTask(UpdateTask { change, form })),
        }
    }

    /// Writes version-checked records, joining the calling thread's task mutation when one
    /// is open. A failed callback publishes neither these records nor its task and events.
    pub fn put_records(&self, writes: Vec<RecordWrite>) -> Result<()> {
        anyhow::ensure!(
            !self.is_snapshot(),
            "a read-only snapshot cannot write records"
        );
        self.check_runtime()?;
        if writes.is_empty() {
            return Ok(());
        }
        if let Some(unit) = self.current_unit() {
            let mut buffered = unit.records.borrow_mut();
            for mut write in writes {
                if let Some(index) = buffered
                    .iter()
                    .position(|old| old.area == write.area && old.key == write.key)
                {
                    let previous = &buffered[index];
                    anyhow::ensure!(
                        write.expected == Some(previous.expected.unwrap_or(0) + 1)
                            || (previous.doc.is_none() && write.expected.is_none()),
                        "staged record version changed"
                    );
                    write.expected = previous.expected;
                    // A record created and removed inside this unit never existed in its base
                    // view: emitting a tombstone would fail the backend's existence guard.
                    if write.expected.is_none() && write.doc.is_none() {
                        buffered.remove(index);
                    } else {
                        buffered[index] = write;
                    }
                } else {
                    buffered.push(write);
                }
            }
            return Ok(());
        }
        self.db
            .backend
            .apply(Operation::PutRecords(PutRecords {
                writes,
                events: Vec::new(),
                reservations: Vec::new(),
            }))
            .map_err(super::write::apply_error)
            .context("writing records")?;
        Ok(())
    }

    pub(crate) fn staged_record(&self, area: &str, key: &str) -> Option<Option<StoredRecord>> {
        let unit = self.current_unit()?;
        unit.records
            .borrow()
            .iter()
            .find(|write| write.area == area && write.key == key)
            .map(crate::records::staged_row)
    }

    pub(crate) fn staged_record_writes(&self, area: &str) -> Vec<RecordWrite> {
        self.current_unit().map_or_else(Vec::new, |unit| {
            unit.records
                .borrow()
                .iter()
                .filter(|write| write.area == area)
                .cloned()
                .collect()
        })
    }

    pub(crate) fn insert_record_write(&self, write: RecordWrite) -> Result<bool> {
        anyhow::ensure!(!self.is_snapshot(), "a snapshot cannot insert records");
        self.check_runtime()?;
        if self.current_unit().is_some() {
            self.put_records(vec![write])?;
            return Ok(true);
        }
        match self.db.backend.apply(Operation::PutRecords(PutRecords {
            writes: vec![write],
            events: Vec::new(),
            reservations: Vec::new(),
        })) {
            Ok(_) => Ok(true),
            Err(crate::backend::ApplyError::Conflict(_)) => Ok(false),
            Err(error) => Err(super::write::apply_error(error)).context("inserting record"),
        }
    }

    /// Groups a records-only mutation and its events, or joins the caller's task mutation.
    /// Callers retain the area's existing process lease; the callback runs before the
    /// version-checked operation, outside the backend transaction.
    pub fn records_transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let nested = self.current_unit().is_some();
        let _unit = self.event_unit();
        let result = f()?;
        if !nested {
            self.refuse_buffered_conditions("a records-only unit")?;
            let writes = self.buffered_records();
            let events = self.buffered_events();
            if !writes.is_empty() || !events.is_empty() {
                self.db
                    .backend
                    .apply(Operation::PutRecords(PutRecords {
                        writes,
                        events,
                        reservations: Vec::new(),
                    }))
                    .map_err(super::write::apply_error)
                    .context("committing record unit")?;
            }
        }
        Ok(result)
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
        self.append_event_immediate(event)
    }

    // Runtime fencing must survive an aborted unit or detection on a read-only snapshot.
    pub(super) fn append_event_immediate(&self, event: NewEvent) -> Result<()> {
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

    /// Events of `task` (any task when `None`) and `kinds`, newest first, read one at a time
    /// for as long as `more` accepts each one; the first event it refuses ends the scan and is
    /// not returned. Exactly the events examined are decoded, never an older page remainder.
    ///
    /// # Errors
    /// Returns read errors.
    pub fn events_newest(
        &self,
        task: Option<&str>,
        kinds: &[&str],
        run: Option<&str>,
        mut more: impl FnMut(&Event) -> bool,
    ) -> Result<Vec<Event>> {
        let view = self.view()?;
        let query = EventQuery {
            task: task.map(str::to_owned),
            kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
            run: run.map(str::to_owned),
            newest_first: true,
            valid_only: true,
            ..EventQuery::default()
        };
        let mut events = Vec::new();
        let mut page = PageReq::first(1);
        loop {
            let found = view.events(&query, page.clone())?;
            for row in found.items {
                let Some(event) = projected(row)? else {
                    continue;
                };
                if !more(&event) {
                    return Ok(events);
                }
                events.push(event);
            }
            match found.next {
                Some(next) => page.after = Some(next),
                None => return Ok(events),
            }
        }
    }

    /// The newest event of `task`, `kinds` and `run` that `matches` accepts; older ones are not
    /// read.
    ///
    /// # Errors
    /// Returns read errors.
    pub fn last_event_where(
        &self,
        task: Option<&str>,
        kinds: &[&str],
        run: Option<&str>,
        matches: impl Fn(&Event) -> bool,
    ) -> Result<Option<Event>> {
        let mut found = false;
        let events = self.events_newest(task, kinds, run, |event| {
            !std::mem::replace(&mut found, matches(event))
        })?;
        Ok(events.into_iter().last().filter(|event| matches(event)))
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
