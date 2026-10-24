//! Blob row and scratch directory transitions (plan sections 7-8).

use rusqlite::params;
use serde_json::json;

use super::codec::*;
use super::load;
use super::tx::{Applied, Tx, require};
use crate::backend::*;
use crate::blob_ref::BlobHash;

impl Tx<'_> {
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
                let expires = ts(add_duration(now, ttl));
                let outcome = match load::blob_by_hash(self.db, &hash).or_io()? {
                    Some(row) if row.state == BlobState::Deleting => ReserveOutcome::Busy,
                    Some(_) => {
                        self.db.execute(
                            "DELETE FROM reservations WHERE hash = ?1 AND token = ?2",
                            params![hash.as_str(), token],
                        )?;
                        ReserveOutcome::Exists
                    }
                    None => {
                        self.db.execute(
                            "INSERT INTO blobs(hash, raw_bytes, stored_bytes, codec, state, refs, \
                             unreferenced_since, created) \
                             VALUES (?1, ?2, ?3, ?4, 'writing', 0, NULL, ?5)",
                            params![
                                hash.as_str(),
                                int(raw_bytes).or_io()?,
                                int(stored_bytes).or_io()?,
                                codec,
                                ts(now),
                            ],
                        )?;
                        ReserveOutcome::MustWrite
                    }
                };
                if outcome != ReserveOutcome::Busy {
                    self.db.execute(
                        "INSERT INTO reservations(hash, token, owner, expires) \
                         VALUES (?1, ?2, ?3, ?4)",
                        params![hash.as_str(), token, owner, expires],
                    )?;
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
                self.db.execute(
                    "UPDATE blobs SET state = 'deleting' WHERE hash = ?1",
                    params![hash.as_str()],
                )?;
                self.scope(ChangeScope::Blob { hash });
                self.hit("blob state")?;
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
                self.db.execute(
                    "DELETE FROM reservations WHERE hash = ?1",
                    params![hash.as_str()],
                )?;
                self.db
                    .execute("DELETE FROM blobs WHERE hash = ?1", params![hash.as_str()])?;
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
                require(
                    load::blob_by_hash(self.db, &hash).or_io()?.is_none(),
                    || format!("blob {hash} has a row; it is not a stray"),
                )?;
                self.db.execute(
                    "INSERT INTO blobs(hash, raw_bytes, stored_bytes, codec, state, refs, \
                     unreferenced_since, created) VALUES (?1, 0, ?2, 'unknown', 'deleting', 0, ?3, ?3)",
                    params![hash.as_str(), int(stored_bytes).or_io()?, ts(now)],
                )?;
                self.scope(ChangeScope::Blob { hash });
                self.hit("blob adopt")?;
                Ok(Outcome::Applied)
            }
        }
    }

    fn blob_row(&self, hash: &BlobHash) -> Applied<BlobRow> {
        load::blob_by_hash(self.db, hash)
            .or_io()?
            .ok_or_else(|| ApplyError::precondition(format!("blob {hash} has no row")))
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

    fn scratch_row(&self, id: &str) -> Applied<ScratchRow> {
        load::scratch_by_id(self.db, id)
            .or_io()?
            .ok_or_else(|| ApplyError::precondition(format!("scratch {id} is not registered")))
    }

    fn scratch_transition(
        &mut self,
        id: &str,
        from: ScratchState,
        edit: impl FnOnce(&mut ScratchRow),
    ) -> Applied<ScratchRow> {
        let mut row = self.scratch_row(id)?;
        if row.state != from {
            return Err(Conflict::new(format!("scratch {id}"), from, row.state).into());
        }
        edit(&mut row);
        self.db.execute(
            "UPDATE scratch_dirs SET state = ?2, reason = ?3, next_check_at = ?4, owner = ?5, \
             warned = ?6, evidence_bytes = ?7, closed = ?8, reclaimed = ?9 WHERE id = ?1",
            params![
                id,
                name(&row.state).or_io()?,
                row.reason,
                opt_ts(row.next_check_at),
                row.owner,
                opt_ts(row.warned),
                int(row.evidence_bytes).or_io()?,
                opt_ts(row.closed),
                opt_ts(row.reclaimed),
            ],
        )?;
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
                require(load::scratch_by_id(self.db, &id).or_io()?.is_none(), || {
                    format!("scratch {id} is already registered")
                })?;
                let relative = !path.is_empty()
                    && !path.starts_with('/')
                    && !path.split('/').any(|part| part == ".." || part.is_empty());
                require(relative, || {
                    format!("scratch path {path:?} must be relative, without `..` or empty parts")
                })?;
                let taken = load::first(
                    self.db,
                    "SELECT 1 FROM scratch_dirs WHERE path = ?1",
                    params![path],
                    |_| Ok(()),
                )
                .or_io()?;
                require(taken.is_none(), || {
                    format!("scratch path {path} is already registered")
                })?;
                self.scope(ChangeScope::Scratch { id: id.clone() });
                self.db.execute(
                    "INSERT INTO scratch_dirs(id, kind, task, subject, attempt, path, state, \
                     reason, next_check_at, owner, warned, evidence_bytes, created, closed, \
                     reclaimed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', NULL, NULL, NULL, NULL, \
                     0, ?7, NULL, NULL)",
                    params![
                        id,
                        name(&kind).or_io()?,
                        task,
                        subject,
                        attempt,
                        path,
                        ts(at)
                    ],
                )?;
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
                let row = self.scratch_row(&id)?;
                require(row.warned.is_none(), || {
                    format!("scratch {id} was already warned")
                })?;
                self.scratch_transition(&id, row.state, |row| {
                    row.warned = Some(at);
                    row.evidence_bytes = evidence_bytes;
                })?;
                self.storage_event(
                    row.task,
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
                let row = self.scratch_row(&id)?;
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
                let held = load::scratch_by_id(self.db, &id)
                    .or_io()?
                    .and_then(|row| row.owner);
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
}
