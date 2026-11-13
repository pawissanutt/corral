//! Record, claim, session, condition, lifecycle, lease and import operations, and the change
//! entry every applied operation appends.

use rusqlite::params;
use serde_json::json;

use super::codec::*;
use super::load::{self, first};
use super::tx::{Applied, Owner, Tx, require, scope_kind};
use crate::backend::*;
use crate::blob_ref;

impl crate::backend::question_events::QuestionEventTx for Tx<'_> {
    fn question_record(&self, area: &str, key: &str) -> Applied<Option<StoredRecord>> {
        first(
            self.db,
            "SELECT area, key, version, doc, updated FROM records WHERE area = ?1 AND key = ?2",
            params![area, key],
            load::record,
        )
        .or_io()
    }
    fn question_put(&mut self, write: RecordWrite) -> Applied<()> {
        self.put_record(&write)
    }
    fn question_now(&self) -> jiff::Timestamp {
        self.now
    }
}

impl Tx<'_> {
    pub(super) fn put_record(&mut self, write: &RecordWrite) -> Applied<()> {
        let stored: Option<u64> = first(
            self.db,
            "SELECT version FROM records WHERE area = ?1 AND key = ?2",
            params![write.area, write.key],
            |row| load::num(row, 0),
        )
        .or_io()?;
        if stored != write.expected {
            return Err(Conflict::new(
                format!("record {}/{}", write.area, write.key),
                write.expected,
                stored,
            )
            .into());
        }
        let refs = match &write.doc {
            Some(doc) => {
                self.db.execute(
                    "INSERT OR REPLACE INTO records(area, key, version, doc, updated) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        write.area,
                        write.key,
                        int(stored.map_or(1, |v| v + 1)).or_io()?,
                        json(doc).or_io()?,
                        ts(write.updated),
                    ],
                )?;
                blob_ref::scan(doc)
            }
            None => {
                let removed = self.db.execute(
                    "DELETE FROM records WHERE area = ?1 AND key = ?2",
                    params![write.area, write.key],
                )?;
                require(removed > 0, || {
                    format!("record {}/{} does not exist", write.area, write.key)
                })?;
                Default::default()
            }
        };
        if write.area == "reviewed-artifacts" && write.key == "set" {
            super::artifact_reviews::replace(self.db, write.doc.as_ref()).or_io()?;
        }
        self.scope(ChangeScope::Record {
            area: write.area.clone(),
            key: write.key.clone(),
        });
        self.hit("record")?;
        self.set_refs(Owner::Record(&write.area, &write.key), refs)
    }

    fn claim_owner(&self, name: &str) -> Applied<Option<String>> {
        first(
            self.db,
            "SELECT owner FROM claims WHERE name = ?1",
            params![name],
            |row| load::text(row, 0),
        )
        .or_io()
    }

    pub(super) fn claim(&mut self, op: Claim) -> Applied<Outcome> {
        if let Some(held) = self.claim_owner(&op.name)? {
            return Err(Conflict::new(format!("claim {}", op.name), "absent", &held).into());
        }
        self.scope(ChangeScope::Record {
            area: "claims".to_owned(),
            key: op.name.clone(),
        });
        self.db.execute(
            "INSERT INTO claims(name, owner, acquired, doc) VALUES (?1, ?2, ?3, ?4)",
            params![op.name, op.owner, ts(op.acquired), json(&op.doc).or_io()?],
        )?;
        self.hit("claim")?;
        self.set_refs(Owner::Claim(&op.name), blob_ref::scan(&op.doc))?;
        Ok(Outcome::Applied)
    }

    pub(super) fn release_claim(&mut self, op: ReleaseClaim) -> Applied<Outcome> {
        let held = self.claim_owner(&op.name)?;
        require(held.as_deref() == Some(op.owner.as_str()), || {
            format!("claim {} is held by {held:?}, not {}", op.name, op.owner)
        })?;
        self.db
            .execute("DELETE FROM claims WHERE name = ?1", params![op.name])?;
        self.scope(ChangeScope::Record {
            area: "claims".to_owned(),
            key: op.name.clone(),
        });
        self.hit("claim release")?;
        self.set_refs(Owner::Claim(&op.name), Default::default())?;
        Ok(Outcome::Applied)
    }

    pub(super) fn require_gc_claim(&self, gc_owner: &str) -> Applied<()> {
        let held = self.claim_owner(STORAGE_GC_CLAIM)?;
        require(held.as_deref() == Some(gc_owner), || {
            format!("{STORAGE_GC_CLAIM} is held by {held:?}, not {gc_owner}")
        })
    }

    pub(super) fn bind_session(&mut self, op: BindSession) -> Applied<Outcome> {
        let stored = first(
            self.db,
            "SELECT area, key, version, doc, updated FROM records WHERE area = ?1 AND key = ?2",
            params![SESSION_AREA, SESSION_KEY],
            load::record,
        )
        .or_io()?;
        if let Some(record) = &stored {
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

    fn open_condition(&self, task: &str, kind: &str) -> Applied<Option<EventSeq>> {
        first(
            self.db,
            "SELECT open_run FROM conditions WHERE task = ?1 AND kind = ?2",
            params![task, kind],
            |row| Ok(EventSeq(load::num(row, 0)?)),
        )
        .or_io()
    }

    fn condition_run(&self, opened: EventSeq) -> Applied<ConditionRun> {
        let sql = format!(
            "SELECT {} FROM condition_runs WHERE opened_seq = ?1",
            load::CONDITION_COLUMNS
        );
        first(
            self.db,
            &sql,
            params![int(opened.0).or_io()?],
            load::condition_run,
        )
        .or_io()?
        .ok_or_else(|| ApplyError::Io(anyhow::anyhow!("condition run {} is missing", opened.0)))
    }

    pub(super) fn observe_condition(&mut self, op: ObserveCondition) -> Applied<Outcome> {
        require(CONDITION_KINDS.contains(&op.kind.as_str()), || {
            format!("{} is not a condition kind", op.kind)
        })?;
        let digest = detail_digest(&op.detail);
        self.scope(ChangeScope::Condition {
            task: op.task.clone(),
            kind: op.kind.clone(),
        });
        let open = self.open_condition(&op.task, &op.kind)?;
        if let Some(opened) = open {
            let run = self.condition_run(opened)?;
            if run.detail_digest == digest {
                self.db.execute(
                    "UPDATE condition_runs SET count = count + 1, last_at = ?2, last_actor = ?3, \
                     last_via = ?4 WHERE opened_seq = ?1",
                    params![int(opened.0).or_io()?, ts(op.at), op.actor, op.via],
                )?;
                self.hit("condition repeat")?;
                return Ok(Outcome::Condition(ConditionOutcome::Repeated {
                    opened_seq: opened,
                    count: run.count + 1,
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
        self.db.execute(
            "INSERT INTO condition_runs(opened_seq, task, kind, detail_digest, count, first_at, \
             last_at, first_actor, first_via, last_actor, last_via, closed_seq, close_reason) \
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5, ?6, ?7, ?6, ?7, NULL, NULL)",
            params![
                int(event.0).or_io()?,
                op.task,
                op.kind,
                digest,
                ts(op.at),
                op.actor,
                op.via,
            ],
        )?;
        self.hit("condition run")?;
        self.db.execute(
            "INSERT OR REPLACE INTO conditions(task, kind, open_run) VALUES (?1, ?2, ?3)",
            params![op.task, op.kind, int(event.0).or_io()?],
        )?;
        self.hit("condition pointer")?;
        Ok(Outcome::Condition(ConditionOutcome::Opened {
            opened_seq: event,
            closed: open,
        }))
    }

    fn close_run(&mut self, opened: EventSeq, by: EventSeq, reason: CloseReason) -> Applied<()> {
        self.db.execute(
            "UPDATE condition_runs SET closed_seq = ?2, close_reason = ?3 WHERE opened_seq = ?1",
            params![
                int(opened.0).or_io()?,
                int(by.0).or_io()?,
                name(&reason).or_io()?
            ],
        )?;
        self.hit("condition close")
    }

    pub(super) fn clear_condition(&mut self, op: ClearCondition) -> Applied<Outcome> {
        let Some(opened) = self.open_condition(&op.task, &op.kind)? else {
            return Ok(Outcome::Condition(ConditionOutcome::NotOpen));
        };
        let run = self.condition_run(opened)?;
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
        self.db.execute(
            "DELETE FROM conditions WHERE task = ?1 AND kind = ?2",
            params![op.task, op.kind],
        )?;
        self.hit("condition pointer")?;
        Ok(Outcome::Condition(ConditionOutcome::Cleared { event }))
    }

    /// Appends the operation's change entry and returns its seq. An outcome that wrote
    /// nothing appends none and returns the current seq, so feed followers see no change.
    pub(super) fn change_entry(&self, kind: OpKind, outcome: &Outcome) -> Applied<ChangeSeq> {
        let current = ChangeSeq(load::meta_u64(self.db, "change_seq").or_io()?);
        if no_op(outcome) {
            return Ok(current);
        }
        let seq = ChangeSeq(current.0 + 1);
        self.db.execute(
            "INSERT INTO changes(seq, at, op) VALUES (?1, ?2, ?3)",
            params![int(seq.0).or_io()?, ts(self.now), name(&kind).or_io()?],
        )?;
        for scope in &self.scopes {
            self.db.execute(
                "INSERT INTO change_scopes(seq, kind, key) VALUES (?1, ?2, ?3)",
                params![int(seq.0).or_io()?, scope_kind(scope), json(scope).or_io()?],
            )?;
        }
        self.db.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'change_seq'",
            params![seq.0.to_string()],
        )?;
        self.hit("change entry")?;
        Ok(seq)
    }

    pub(super) fn lifecycle(&mut self, op: LifecycleWrite) -> Applied<Outcome> {
        let stored: Option<LifecycleOutcome> = first(
            self.db,
            "SELECT outcome FROM lifecycle WHERE task = ?1 AND generation = ?2 AND target = ?3",
            params![op.task, op.generation, op.target],
            |row| from_name(&load::text(row, 0)?),
        )
        .or_io()?;
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
        self.db.execute(
            "INSERT OR REPLACE INTO lifecycle(task, generation, target, policy_digest, outcome, \
             reason, bytes, next_check_at, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                op.task,
                op.generation,
                op.target,
                op.policy_digest,
                name(&op.outcome).or_io()?,
                op.reason,
                opt_int(op.bytes).or_io()?,
                opt_ts(op.next_check_at),
                ts(op.at),
            ],
        )?;
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
        let stored = first(
            self.db,
            "SELECT id, holder, started, expires FROM leases WHERE id = ?1",
            params![id],
            load::lease,
        )
        .or_io()?;
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
                self.db.execute(
                    "INSERT OR REPLACE INTO leases(id, holder, started, expires) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![id, holder, ts(now), ts(expires)],
                )?;
            }
            LeaseOp::Renew { expires, .. } => {
                require(stored.is_some_and(|lease| lease.holder == holder), || {
                    format!("lease {id} is not held by {holder}")
                })?;
                self.db.execute(
                    "UPDATE leases SET expires = ?2 WHERE id = ?1",
                    params![id, ts(expires)],
                )?;
            }
            LeaseOp::Release { .. } => {
                require(stored.is_some_and(|lease| lease.holder == holder), || {
                    format!("lease {id} is not held by {holder}")
                })?;
                self.db
                    .execute("DELETE FROM leases WHERE id = ?1", params![id])?;
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
        let now = ts(self.now);
        for blob in op.blobs {
            let exists = first(
                self.db,
                "SELECT 1 FROM blobs WHERE hash = ?1",
                params![blob.hash.as_str()],
                |_| Ok(()),
            )
            .or_io()?;
            require(exists.is_none(), || {
                format!("blob {} already exists", blob.hash)
            })?;
            self.db.execute(
                "INSERT INTO blobs(hash, raw_bytes, stored_bytes, codec, state, refs, \
                 unreferenced_since, created) VALUES (?1, ?2, ?3, ?4, 'live', 0, ?5, ?5)",
                params![
                    blob.hash.as_str(),
                    int(blob.raw_bytes).or_io()?,
                    int(blob.stored_bytes).or_io()?,
                    blob.codec,
                    now,
                ],
            )?;
            self.scope(ChangeScope::Blob { hash: blob.hash });
            self.hit("import blob")?;
        }
        for task in op.tasks {
            let header = &task.header;
            let num_taken = first(
                self.db,
                "SELECT 1 FROM tasks WHERE num = ?1",
                params![int(header.num.0).or_io()?],
                |_| Ok(()),
            )
            .or_io()?
            .is_some();
            require(!num_taken && !self.id_taken(&header.id)?, || {
                format!("task {} already exists", header.id)
            })?;
            let head = TaskHeadDoc {
                header: task.header,
                doc: task.doc,
                summary: task.summary,
            };
            self.insert_task(head, task.rows)?;
        }
        for imported in op.events {
            self.insert_event(imported.event, imported.malformed)?;
        }
        for record in op.records {
            use crate::backend::question_events::QuestionEventTx;
            if self.question_import_skip(&record)? {
                continue;
            }
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

/// Outcomes that write nothing and therefore append no change entry: clearing a condition
/// that is not open, and a blob reservation refused while the blob is being deleted. Keep in
/// step with the fake's `no_op`.
fn no_op(outcome: &Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Condition(ConditionOutcome::NotOpen) | Outcome::Reserved(ReserveOutcome::Busy)
    )
}
