//! Record, claim, condition, blob, scratch, lifecycle, lease and import operations of the fake.

use std::sync::Arc;

use serde_json::json;

use super::RefOwner;
use super::apply::{Applied, Tx, require};
use crate::backend::support::{add, detail_digest};
use crate::backend::*;
use crate::blob_ref::{self, BlobHash};

impl Tx<'_> {
    pub(super) fn put_record(&mut self, write: &RecordWrite) -> Applied<()> {
        let key = (write.area.clone(), write.key.clone());
        let stored = self.state.records.get(&key).map(|record| record.version);
        if stored != write.expected {
            return Err(Conflict::new(
                format!("record {}/{}", write.area, write.key),
                write.expected,
                stored,
            )
            .into());
        }
        let owner = RefOwner::Record(write.area.clone(), write.key.clone());
        if write.area == "reviewed-artifacts" && write.key == "set" {
            let paths = write
                .doc
                .as_ref()
                .map(crate::backend::artifact_reviews::paths)
                .transpose()
                .map_err(ApplyError::Io)?
                .unwrap_or_default();
            Arc::make_mut(&mut self.state.artifact_reviews).replace(paths);
        }
        let records = Arc::make_mut(&mut self.state.records);
        let refs = match &write.doc {
            Some(doc) => {
                records.insert(
                    key,
                    StoredRecord {
                        area: write.area.clone(),
                        key: write.key.clone(),
                        version: stored.map_or(1, |v| v + 1),
                        doc: doc.clone(),
                        updated: write.updated,
                    },
                );
                blob_ref::scan(doc)
            }
            None => {
                require(records.remove(&key).is_some(), || {
                    format!("record {}/{} does not exist", write.area, write.key)
                })?;
                Default::default()
            }
        };
        self.scope(ChangeScope::Record {
            area: write.area.clone(),
            key: write.key.clone(),
        });
        self.hit("record")?;
        self.set_refs(owner, refs)
    }

    pub(super) fn claim(&mut self, op: Claim) -> Applied<Outcome> {
        if let Some(held) = self.state.claims.get(&op.name) {
            return Err(Conflict::new(format!("claim {}", op.name), "absent", &held.owner).into());
        }
        self.scope(ChangeScope::Record {
            area: "claims".to_owned(),
            key: op.name.clone(),
        });
        let refs = blob_ref::scan(&op.doc);
        Arc::make_mut(&mut self.state.claims).insert(
            op.name.clone(),
            ClaimRow {
                name: op.name.clone(),
                owner: op.owner,
                acquired: op.acquired,
                doc: op.doc,
            },
        );
        self.hit("claim")?;
        self.set_refs(RefOwner::Claim(op.name), refs)?;
        Ok(Outcome::Applied)
    }

    pub(super) fn release_claim(&mut self, op: ReleaseClaim) -> Applied<Outcome> {
        let held = self
            .state
            .claims
            .get(&op.name)
            .map(|claim| claim.owner.clone());
        require(held.as_deref() == Some(op.owner.as_str()), || {
            format!("claim {} is held by {held:?}, not {}", op.name, op.owner)
        })?;
        Arc::make_mut(&mut self.state.claims).remove(&op.name);
        self.scope(ChangeScope::Record {
            area: "claims".to_owned(),
            key: op.name.clone(),
        });
        self.hit("claim release")?;
        self.set_refs(RefOwner::Claim(op.name), Default::default())?;
        Ok(Outcome::Applied)
    }

    fn require_gc_claim(&self, gc_owner: &str) -> Applied<()> {
        let held = self
            .state
            .claims
            .get(STORAGE_GC_CLAIM)
            .map(|c| c.owner.as_str());
        require(held == Some(gc_owner), || {
            format!("{STORAGE_GC_CLAIM} is held by {held:?}, not {gc_owner}")
        })
    }

    pub(super) fn bind_session(&mut self, op: BindSession) -> Applied<Outcome> {
        let key = (SESSION_AREA.to_owned(), SESSION_KEY.to_owned());
        let stored = self.state.records.get(&key);
        if let Some(record) = stored {
            let bound = record.doc.get("socket").and_then(|s| s.as_str());
            require(bound == Some(op.socket.as_str()), || {
                format!("the session is bound to {bound:?}, not {}", op.socket)
            })?;
        }
        let write = RecordWrite {
            area: SESSION_AREA.to_owned(),
            key: SESSION_KEY.to_owned(),
            expected: stored.map(|record| record.version),
            doc: Some(json!({"socket": op.socket, "binding": op.binding})),
            updated: op.at,
        };
        self.put_record(&write)?;
        Ok(Outcome::Applied)
    }

    pub(super) fn observe_condition(&mut self, op: ObserveCondition) -> Applied<Outcome> {
        require(CONDITION_KINDS.contains(&op.kind.as_str()), || {
            format!("{} is not a condition kind", op.kind)
        })?;
        let digest = detail_digest(&op.detail);
        let key = (op.task.clone(), op.kind.clone());
        self.scope(ChangeScope::Condition {
            task: op.task.clone(),
            kind: op.kind.clone(),
        });
        let open = self.state.conditions.get(&key).copied();
        if let Some(opened) = open {
            let runs = Arc::make_mut(&mut self.state.condition_runs);
            let run = runs.get_mut(&opened).ok_or_else(|| {
                ApplyError::Io(anyhow::anyhow!("condition run {} is missing", opened.0))
            })?;
            if run.detail_digest == digest {
                run.count += 1;
                run.last_at = op.at;
                run.last_actor = op.actor;
                run.last_via = op.via;
                let count = run.count;
                self.hit("condition repeat")?;
                return Ok(Outcome::Condition(ConditionOutcome::Repeated {
                    opened_seq: opened,
                    count,
                }));
            }
        }
        let event = self.append_event(NewEvent {
            ts: op.at,
            task: (!op.task.is_empty()).then(|| op.task.clone()),
            kind: op.kind.clone(),
            actor: op.actor.clone(),
            via: op.via.clone(),
            run: None,
            detail: op.detail,
        })?;
        if let Some(opened) = open {
            self.close_run(opened, event, CloseReason::Changed)?;
        }
        Arc::make_mut(&mut self.state.condition_runs).insert(
            event,
            ConditionRun {
                opened_seq: event,
                task: op.task,
                kind: op.kind,
                detail_digest: digest,
                count: 1,
                first_at: op.at,
                last_at: op.at,
                first_actor: op.actor.clone(),
                first_via: op.via.clone(),
                last_actor: op.actor,
                last_via: op.via,
                closed_seq: None,
                close_reason: None,
            },
        );
        self.hit("condition run")?;
        Arc::make_mut(&mut self.state.conditions).insert(key, event);
        self.hit("condition pointer")?;
        Ok(Outcome::Condition(ConditionOutcome::Opened {
            opened_seq: event,
            closed: open,
        }))
    }

    fn close_run(&mut self, opened: EventSeq, by: EventSeq, reason: CloseReason) -> Applied<()> {
        let runs = Arc::make_mut(&mut self.state.condition_runs);
        if let Some(run) = runs.get_mut(&opened) {
            run.closed_seq = Some(by);
            run.close_reason = Some(reason);
        }
        self.hit("condition close")
    }

    pub(super) fn clear_condition(&mut self, op: ClearCondition) -> Applied<Outcome> {
        let key = (op.task.clone(), op.kind.clone());
        let Some(opened) = self.state.conditions.get(&key).copied() else {
            return Ok(Outcome::Condition(ConditionOutcome::NotOpen));
        };
        let run = self
            .state
            .condition_runs
            .get(&opened)
            .cloned()
            .ok_or_else(|| {
                ApplyError::Io(anyhow::anyhow!("condition run {} is missing", opened.0))
            })?;
        self.scope(ChangeScope::Condition {
            task: op.task.clone(),
            kind: op.kind.clone(),
        });
        let event = self.append_event(NewEvent {
            ts: op.at,
            task: (!op.task.is_empty()).then(|| op.task.clone()),
            kind: format!("{}_cleared", op.kind),
            actor: op.actor,
            via: op.via,
            run: None,
            detail: json!({
                "run": opened.0,
                "count": run.count,
                "first_at": run.first_at,
                "last_at": run.last_at,
            }),
        })?;
        self.close_run(opened, event, CloseReason::Cleared)?;
        Arc::make_mut(&mut self.state.conditions).remove(&key);
        self.hit("condition pointer")?;
        Ok(Outcome::Condition(ConditionOutcome::Cleared { event }))
    }

    pub(super) fn blob(&mut self, op: BlobOp) -> Applied<Outcome> {
        let now = self.now;
        match op {
            BlobOp::Reserve {
                hash,
                raw_bytes,
                stored_bytes,
                codec,
                token,
                owner,
                ttl,
            } => {
                let expires = add(now, ttl);
                let reservation = Reservation {
                    token: token.clone(),
                    owner,
                    expires,
                };
                let blobs = Arc::make_mut(&mut self.state.blobs);
                let outcome = match blobs.get_mut(&hash) {
                    Some(row) if row.state == BlobState::Deleting => ReserveOutcome::Busy,
                    Some(row) => {
                        row.reservations.retain(|r| r.token != token);
                        row.reservations.push(reservation);
                        ReserveOutcome::Exists
                    }
                    None => {
                        blobs.insert(
                            hash.clone(),
                            BlobRow {
                                hash: hash.clone(),
                                raw_bytes,
                                stored_bytes,
                                codec,
                                state: BlobState::Writing,
                                refs: 0,
                                unreferenced_since: None,
                                created: now,
                                reservations: vec![reservation],
                            },
                        );
                        ReserveOutcome::MustWrite
                    }
                };
                if outcome != ReserveOutcome::Busy {
                    self.scope(ChangeScope::Blob { hash });
                    self.hit("blob reserve")?;
                }
                Ok(Outcome::Reserved(outcome))
            }
            BlobOp::BeginDelete {
                hash,
                gc_owner,
                horizon,
            } => {
                self.require_gc_claim(&gc_owner)?;
                let row = self.blob_row(&hash)?;
                let reserved = row.reservations.iter().any(|r| r.expires > now);
                if row.state == BlobState::Deleting {
                    return Err(Conflict::new(format!("blob {hash}"), "live", row.state).into());
                }
                require(row.refs == 0, || {
                    format!("blob {hash} has {} references", row.refs)
                })?;
                require(!reserved, || format!("blob {hash} has a live reservation"))?;
                let since = row.unreferenced_since.unwrap_or(row.created);
                require(since < horizon, || {
                    format!("blob {hash} is unreferenced since {since}, not before {horizon}")
                })?;
                self.set_blob_state(&hash, BlobState::Deleting)?;
                Ok(Outcome::Applied)
            }
            BlobOp::FinishDelete { hash, gc_owner } => {
                self.require_gc_claim(&gc_owner)?;
                let row = self.blob_row(&hash)?;
                if row.state != BlobState::Deleting {
                    return Err(Conflict::new(
                        format!("blob {hash}"),
                        BlobState::Deleting,
                        row.state,
                    )
                    .into());
                }
                Arc::make_mut(&mut self.state.blobs).remove(&hash);
                self.scope(ChangeScope::Blob { hash });
                self.hit("blob delete")?;
                Ok(Outcome::Applied)
            }
            BlobOp::AdoptStray {
                hash,
                gc_owner,
                stored_bytes,
            } => {
                self.require_gc_claim(&gc_owner)?;
                require(!self.state.blobs.contains_key(&hash), || {
                    format!("blob {hash} has a row; it is not a stray")
                })?;
                Arc::make_mut(&mut self.state.blobs).insert(
                    hash.clone(),
                    BlobRow {
                        hash: hash.clone(),
                        raw_bytes: 0,
                        stored_bytes,
                        codec: "unknown".to_owned(),
                        state: BlobState::Deleting,
                        refs: 0,
                        unreferenced_since: Some(now),
                        created: now,
                        reservations: Vec::new(),
                    },
                );
                self.scope(ChangeScope::Blob { hash });
                self.hit("blob adopt")?;
                Ok(Outcome::Applied)
            }
        }
    }

    fn blob_row(&self, hash: &BlobHash) -> Applied<&BlobRow> {
        self.state
            .blobs
            .get(hash)
            .ok_or_else(|| ApplyError::precondition(format!("blob {hash} has no row")))
    }

    fn set_blob_state(&mut self, hash: &BlobHash, state: BlobState) -> Applied<()> {
        if let Some(row) = Arc::make_mut(&mut self.state.blobs).get_mut(hash) {
            row.state = state;
        }
        self.scope(ChangeScope::Blob { hash: hash.clone() });
        self.hit("blob state")
    }

    pub(super) fn close_scratch(&mut self, ids: &[String]) -> Applied<()> {
        for id in ids {
            let at = self.now;
            self.scratch_transition(id, ScratchState::Open, |row| {
                row.state = ScratchState::Closed;
                row.closed = Some(at);
            })?;
        }
        Ok(())
    }

    fn scratch_transition(
        &mut self,
        id: &str,
        from: ScratchState,
        edit: impl FnOnce(&mut ScratchRow),
    ) -> Applied<ScratchRow> {
        let row =
            self.state.scratch.get(id).ok_or_else(|| {
                ApplyError::precondition(format!("scratch {id} is not registered"))
            })?;
        if row.state != from {
            return Err(Conflict::new(format!("scratch {id}"), from, row.state).into());
        }
        let table = Arc::make_mut(&mut self.state.scratch);
        let row = table
            .get_mut(id)
            .ok_or_else(|| ApplyError::precondition(format!("scratch {id} is not registered")))?;
        edit(row);
        let row = row.clone();
        self.scope(ChangeScope::Scratch { id: id.to_owned() });
        self.hit("scratch")?;
        Ok(row)
    }

    pub(super) fn scratch(&mut self, op: ScratchOp) -> Applied<Outcome> {
        let now = self.now;
        match op {
            ScratchOp::Register {
                id,
                kind,
                task,
                subject,
                attempt,
                path,
                at,
            } => {
                require(!self.state.scratch.contains_key(&id), || {
                    format!("scratch {id} is already registered")
                })?;
                let relative = !path.is_empty()
                    && !path.starts_with('/')
                    && !path.split('/').any(|part| part == ".." || part.is_empty());
                require(relative, || {
                    format!("scratch path {path:?} must be relative, without `..` or empty parts")
                })?;
                require(
                    !self.state.scratch.values().any(|row| row.path == path),
                    || format!("scratch path {path} is already registered"),
                )?;
                self.scope(ChangeScope::Scratch { id: id.clone() });
                Arc::make_mut(&mut self.state.scratch).insert(
                    id.clone(),
                    ScratchRow {
                        id,
                        kind,
                        task,
                        subject,
                        attempt,
                        path,
                        state: ScratchState::Open,
                        reason: None,
                        next_check_at: None,
                        owner: None,
                        warned: None,
                        evidence_bytes: 0,
                        created: at,
                        closed: None,
                        reclaimed: None,
                    },
                );
                self.hit("scratch register")?;
            }
            ScratchOp::Close { id, at } => {
                self.scratch_transition(&id, ScratchState::Open, |row| {
                    row.state = ScratchState::Closed;
                    row.closed = Some(at);
                })?;
            }
            ScratchOp::Defer {
                id,
                from,
                reason,
                next_check_at,
            } => {
                require(
                    matches!(from, ScratchState::Closed | ScratchState::Deferred),
                    || format!("scratch {id} cannot be deferred from {from:?}"),
                )?;
                self.scratch_transition(&id, from, |row| {
                    row.state = ScratchState::Deferred;
                    row.reason = Some(reason);
                    row.next_check_at = Some(next_check_at);
                })?;
            }
            ScratchOp::Warn {
                id,
                evidence_bytes,
                at,
            } => {
                let row = self.state.scratch.get(&id).ok_or_else(|| {
                    ApplyError::precondition(format!("scratch {id} is not registered"))
                })?;
                require(row.warned.is_none(), || {
                    format!("scratch {id} was already warned")
                })?;
                let state = row.state;
                let task = row.task.clone();
                self.scratch_transition(&id, state, |row| {
                    row.warned = Some(at);
                    row.evidence_bytes = evidence_bytes;
                })?;
                self.storage_event(
                    task,
                    "storage_scratch_warning",
                    json!({"scratch": id, "bytes": evidence_bytes}),
                )?;
            }
            ScratchOp::BeginReclaim {
                id,
                from,
                owner,
                at: _,
            } => {
                let row = self.state.scratch.get(&id).ok_or_else(|| {
                    ApplyError::precondition(format!("scratch {id} is not registered"))
                })?;
                let due = match from {
                    ScratchState::Closed => true,
                    ScratchState::Deferred => row.next_check_at.is_none_or(|at| at <= now),
                    _ => false,
                };
                if row.state == from {
                    require(due, || {
                        format!("scratch {id} cannot start reclaiming from {from:?} now")
                    })?;
                }
                self.scratch_transition(&id, from, |row| {
                    row.state = ScratchState::Reclaiming;
                    row.owner = Some(owner);
                })?;
            }
            ScratchOp::FinishReclaim {
                id,
                owner,
                result,
                at,
            } => {
                let held = self
                    .state
                    .scratch
                    .get(&id)
                    .and_then(|row| row.owner.clone());
                require(held.as_deref() == Some(owner.as_str()), || {
                    format!("scratch {id} is reclaimed by {held:?}, not {owner}")
                })?;
                let (state, kind, detail) = match &result {
                    ReclaimResult::Reclaimed { bytes } => (
                        ScratchState::Reclaimed,
                        "storage_reclaimed",
                        json!({"scratch": id, "bytes": bytes}),
                    ),
                    ReclaimResult::Failed { reason } => (
                        ScratchState::Failed,
                        "storage_reclaim_failed",
                        json!({"scratch": id, "reason": reason}),
                    ),
                };
                let row = self.scratch_transition(&id, ScratchState::Reclaiming, |row| {
                    row.state = state;
                    if state == ScratchState::Reclaimed {
                        row.reclaimed = Some(at);
                    }
                })?;
                self.storage_event(row.task, kind, detail)?;
            }
        }
        Ok(Outcome::Applied)
    }

    fn storage_event(
        &mut self,
        task: Option<String>,
        kind: &str,
        detail: serde_json::Value,
    ) -> Applied<()> {
        self.append_event(NewEvent {
            ts: self.now,
            task,
            kind: kind.to_owned(),
            actor: None,
            via: None,
            run: None,
            detail,
        })
        .map(drop)
    }

    pub(super) fn lifecycle(&mut self, op: LifecycleWrite) -> Applied<Outcome> {
        let key = (op.task.clone(), op.generation.clone(), op.target.clone());
        let stored = self.state.lifecycle.get(&key).map(|row| row.outcome);
        if stored != op.expected {
            return Err(Conflict::new(
                format!("lifecycle {}/{}/{}", op.task, op.generation, op.target),
                op.expected,
                stored,
            )
            .into());
        }
        self.scope(ChangeScope::Lifecycle {
            task: op.task.clone(),
        });
        Arc::make_mut(&mut self.state.lifecycle).insert(
            key,
            LifecycleRow {
                task: op.task,
                generation: op.generation,
                target: op.target,
                policy_digest: op.policy_digest,
                outcome: op.outcome,
                reason: op.reason,
                bytes: op.bytes,
                next_check_at: op.next_check_at,
                at: op.at,
            },
        );
        self.hit("lifecycle")?;
        Ok(Outcome::Applied)
    }

    pub(super) fn lease(&mut self, op: LeaseOp) -> Applied<Outcome> {
        let now = self.now;
        let (id, holder) = match &op {
            LeaseOp::Acquire { id, holder, .. }
            | LeaseOp::Renew { id, holder, .. }
            | LeaseOp::Release { id, holder } => (id.clone(), holder.clone()),
        };
        let stored = self.state.leases.get(&id).cloned();
        let leases = Arc::make_mut(&mut self.state.leases);
        match op {
            LeaseOp::Acquire { expires, .. } => {
                if let Some(held) = stored.filter(|lease| lease.expires > now) {
                    return Err(Conflict::new(
                        format!("lease {id}"),
                        "absent or expired",
                        held.holder,
                    )
                    .into());
                }
                leases.insert(
                    id.clone(),
                    LeaseRow {
                        id: id.clone(),
                        holder,
                        started: now,
                        expires,
                    },
                );
            }
            LeaseOp::Renew { expires, .. } => {
                let mut lease = stored
                    .filter(|lease| lease.holder == holder)
                    .ok_or_else(|| {
                        ApplyError::precondition(format!("lease {id} is not held by {holder}"))
                    })?;
                lease.expires = expires;
                leases.insert(id.clone(), lease);
            }
            LeaseOp::Release { .. } => {
                require(stored.is_some_and(|lease| lease.holder == holder), || {
                    format!("lease {id} is not held by {holder}")
                })?;
                leases.remove(&id);
            }
        }
        self.scope(ChangeScope::Record {
            area: "leases".to_owned(),
            key: id,
        });
        self.hit("lease")?;
        Ok(Outcome::Applied)
    }

    pub(super) fn import(&mut self, op: ImportBatch) -> Applied<Outcome> {
        let now = self.now;
        for blob in op.blobs {
            require(!self.state.blobs.contains_key(&blob.hash), || {
                format!("blob {} already exists", blob.hash)
            })?;
            Arc::make_mut(&mut self.state.blobs).insert(
                blob.hash.clone(),
                BlobRow {
                    hash: blob.hash.clone(),
                    raw_bytes: blob.raw_bytes,
                    stored_bytes: blob.stored_bytes,
                    codec: blob.codec,
                    state: BlobState::Live,
                    refs: 0,
                    unreferenced_since: Some(now),
                    created: now,
                    reservations: Vec::new(),
                },
            );
            self.scope(ChangeScope::Blob { hash: blob.hash });
            self.hit("import blob")?;
        }
        for task in op.tasks {
            let header = &task.header;
            require(
                !self.state.tasks.contains_key(&header.num)
                    && !self
                        .state
                        .tasks
                        .values()
                        .any(|t| t.head.header.id == header.id),
                || format!("task {} already exists", header.id),
            )?;
            let head = TaskHeadDoc {
                header: task.header,
                doc: task.doc,
                summary: task.summary,
            };
            self.insert_task(head, task.rows)?;
        }
        for imported in op.events {
            self.append_event(imported.event)?;
            if imported.malformed {
                let events = Arc::make_mut(&mut self.state.events);
                if let Some(last) = events.last_mut() {
                    Arc::make_mut(last).malformed = true;
                }
                self.hit("import malformed flag")?;
            }
        }
        for record in op.records {
            self.put_record(&RecordWrite {
                area: record.area,
                key: record.key,
                expected: None,
                doc: Some(record.doc),
                updated: record.updated,
            })?;
        }
        Ok(Outcome::Applied)
    }
}
