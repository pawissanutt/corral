//! Task-writing operations with their own preconditions (plan section 3 table).

use rusqlite::params;
use serde_json::Value;

use super::codec::*;
use super::tx::{Applied, Owner, Tx, has_event, require};
use crate::backend::*;
use crate::blob_ref;

fn to_doc<T: serde::Serialize>(value: &T) -> Applied<Value> {
    serde_json::to_value(value).map_err(|error| ApplyError::Io(error.into()))
}

fn from_doc<T: serde::de::DeserializeOwned>(value: &Value) -> Applied<T> {
    serde_json::from_value(value.clone()).map_err(|error| ApplyError::Io(error.into()))
}

impl Tx<'_> {
    pub(super) fn comment(&mut self, op: Comment) -> Applied<Outcome> {
        require(has_event(&op.change.events, "commented"), || {
            "a comment requires a `commented` event".to_owned()
        })?;
        require(
            !op.change
                .rows
                .iter()
                .any(|w| matches!(w, RowWrite::Put(r) if r.part == TaskPart::Comment)),
            || "comment rows are keyed by the backend".to_owned(),
        )?;
        let next = self
            .rows_of(op.change.num, TaskPart::Comment)?
            .iter()
            .filter_map(|row| row.key.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let row = HistoryRow {
            part: TaskPart::Comment,
            key: artifact_key(next),
            current: false,
            open: false,
            doc: op.comment,
        };
        self.write_task(&op.change, UpdateForm::Head, vec![RowWrite::Put(row)])
    }

    pub(super) fn done(&mut self, op: Done) -> Applied<Outcome> {
        let run = self.row(op.change.num, TaskPart::Run, &op.run_key)?;
        require(run.is_some_and(|run| run.current && run.open), || {
            format!("run {} is not the open current run", op.run_key)
        })?;
        require(op.change.fields.status == "review", || {
            format!(
                "done moves the task to review, not {}",
                op.change.fields.status
            )
        })?;
        require(has_event(&op.change.events, "done"), || {
            "done requires a `done` event".to_owned()
        })?;
        let row = HistoryRow {
            part: TaskPart::Run,
            key: op.run_key.clone(),
            current: true,
            open: false,
            doc: op.run_doc,
        };
        self.write_task(&op.change, UpdateForm::Head, vec![RowWrite::Put(row)])
    }

    pub(super) fn verdict(&mut self, op: Verdict) -> Applied<Outcome> {
        let status: String = self
            .db
            .query_row(
                "SELECT status FROM tasks WHERE num = ?1",
                params![int(op.change.num.0).or_io()?],
                |row| row.get(0),
            )
            .map_err(ApplyError::from)?;
        require(status == "review", || {
            format!("a verdict needs a task in review, not {status}")
        })?;
        require(has_event(&op.change.events, "verdict"), || {
            "a verdict requires a `verdict` event".to_owned()
        })?;
        let row = self.completed_review(
            op.change.num,
            TaskPart::ReviewRun,
            &op.review_key,
            op.review_doc,
        )?;
        let outcome = self.write_task(&op.change, UpdateForm::Head, vec![RowWrite::Put(row)])?;
        self.close_scratch(&op.close_scratch)?;
        Ok(outcome)
    }

    /// The launched child at `key` (still open and current) closed with `doc` laid over its
    /// stored fields, or a new closed row when no child was launched.
    fn completed_review(
        &self,
        num: TaskNum,
        part: TaskPart,
        key: &str,
        doc: Value,
    ) -> Applied<HistoryRow> {
        let Some(launched) = self.row(num, part, key)? else {
            return Ok(HistoryRow {
                part,
                key: key.to_owned(),
                current: true,
                open: false,
                doc,
            });
        };
        require(launched.current, || {
            format!("review {key} was replaced by a newer attempt")
        })?;
        require(launched.open, || {
            format!("review {key} is already complete")
        })?;
        let doc = match (launched.doc, doc) {
            (Value::Object(mut stored), Value::Object(update)) => {
                stored.extend(update);
                Value::Object(stored)
            }
            (_, update) => update,
        };
        Ok(HistoryRow {
            part,
            key: key.to_owned(),
            current: true,
            open: false,
            doc,
        })
    }

    pub(super) fn plan_submit(&mut self, op: PlanSubmit) -> Applied<Outcome> {
        let num = op.change.num;
        require(
            self.row(num, TaskPart::Plan, &op.plan_key)?.is_none(),
            || format!("plan {} already exists", op.plan_key),
        )?;
        require(!op.change.events.is_empty(), || {
            "a plan submission requires an event".to_owned()
        })?;
        let mut extra: Vec<RowWrite> = self
            .rows_of(num, TaskPart::Plan)?
            .into_iter()
            .filter(|row| row.current)
            .map(|row| {
                RowWrite::Put(HistoryRow {
                    current: false,
                    ..row
                })
            })
            .collect();
        extra.push(RowWrite::Put(HistoryRow {
            part: TaskPart::Plan,
            key: op.plan_key,
            current: true,
            open: true,
            doc: op.plan_doc,
        }));
        self.write_task(&op.change, UpdateForm::Head, extra)
    }

    pub(super) fn plan_verdict(&mut self, op: PlanVerdict) -> Applied<Outcome> {
        let num = op.change.num;
        let plan = self.row(num, TaskPart::Plan, &op.plan_key)?;
        require(plan.is_some_and(|plan| plan.current), || {
            format!("plan {} is not the current plan", op.plan_key)
        })?;
        let key = format!("{}/{}", op.plan_key, op.review_key);
        require(!op.change.events.is_empty(), || {
            "a plan verdict requires an event".to_owned()
        })?;
        let row = self.completed_review(num, TaskPart::PlanReview, &key, op.review_doc)?;
        let outcome = self.write_task(&op.change, UpdateForm::Head, vec![RowWrite::Put(row)])?;
        self.close_scratch(&op.close_scratch)?;
        Ok(outcome)
    }

    fn gate_row(&self, num: TaskNum, run: &str, attempt: &str) -> Applied<GateAttempt> {
        let row = self
            .row(num, TaskPart::GateAttempt, &gate_key(run, attempt))?
            .ok_or_else(|| {
                ApplyError::precondition(format!("gate attempt {run}/{attempt} does not exist"))
            })?;
        let gate: GateAttempt = from_doc(&row.doc)?;
        require(gate.ended.is_none(), || {
            format!("gate attempt {run}/{attempt} already ended")
        })?;
        Ok(gate)
    }

    fn gate_put(gate: &GateAttempt) -> Applied<RowWrite> {
        Ok(RowWrite::Put(HistoryRow {
            part: TaskPart::GateAttempt,
            key: gate.key(),
            current: true,
            open: gate.ended.is_none(),
            doc: to_doc(gate)?,
        }))
    }

    pub(super) fn gate_start(&mut self, op: GateStart) -> Applied<Outcome> {
        let key = op.attempt.key();
        require(
            self.row(op.change.num, TaskPart::GateAttempt, &key)?
                .is_none(),
            || format!("gate attempt {key} already exists"),
        )?;
        require(op.attempt.ended.is_none(), || {
            "a started gate attempt is unended".to_owned()
        })?;
        require(!op.change.events.is_empty(), || {
            "a gate start requires an event".to_owned()
        })?;
        let row = Self::gate_put(&op.attempt)?;
        self.write_task(&op.change, UpdateForm::Head, vec![row])
    }

    pub(super) fn gate_launched(&mut self, op: GateLaunched) -> Applied<Outcome> {
        let mut gate = self.gate_row(op.change.num, &op.run, &op.attempt)?;
        require(gate.launched.is_none(), || {
            format!(
                "gate attempt {}/{} was already launched",
                op.run, op.attempt
            )
        })?;
        gate.launched = Some(op.pane);
        let row = Self::gate_put(&gate)?;
        self.write_task(&op.change, UpdateForm::Head, vec![row])
    }

    pub(super) fn gate_result(&mut self, op: GateResult) -> Applied<Outcome> {
        let mut gate = self.gate_row(op.change.num, &op.run, &op.attempt)?;
        gate.result = Some(op.result);
        gate.failed_cmd = op.failed_cmd;
        gate.log = op.log;
        gate.result_doc = op.result_doc;
        gate.context = op.context.or(gate.context);
        gate.ended = Some(op.ended);
        let row = Self::gate_put(&gate)?;
        let outcome = self.write_task(&op.change, UpdateForm::Head, vec![row])?;
        self.close_scratch(&op.close_scratch)?;
        Ok(outcome)
    }

    pub(super) fn promote_artifacts(&mut self, op: PromoteArtifacts) -> Applied<Outcome> {
        require(has_event(&op.change.events, "artifact"), || {
            "promoting artifacts requires an `artifact` event".to_owned()
        })?;
        let mut rows = Vec::new();
        for artifact in &op.artifacts {
            let key = artifact_key(artifact.seq);
            require(
                self.row(op.change.num, TaskPart::Artifact, &key)?.is_none(),
                || format!("artifact {} already exists", artifact.seq),
            )?;
            rows.push(RowWrite::Put(HistoryRow {
                part: TaskPart::Artifact,
                key,
                current: false,
                open: false,
                doc: to_doc(artifact)?,
            }));
        }
        self.write_task(&op.change, UpdateForm::Head, rows)
    }

    pub(super) fn purge_artifact(&mut self, op: PurgeArtifact) -> Applied<Outcome> {
        let key = artifact_key(op.seq);
        let row = self
            .row(op.change.num, TaskPart::Artifact, &key)?
            .ok_or_else(|| {
                ApplyError::precondition(format!("artifact {} does not exist", op.seq))
            })?;
        let mut artifact: ArtifactRow = from_doc(&row.doc)?;
        require(artifact.purged_at.is_none(), || {
            format!("artifact {} is already purged", op.seq)
        })?;
        artifact.blob = None;
        artifact.purged_at = Some(op.purged_at);
        let stub = HistoryRow {
            doc: to_doc(&artifact)?,
            ..row
        };
        self.write_task(&op.change, UpdateForm::Full, vec![RowWrite::Put(stub)])
    }

    pub(super) fn authorization(&mut self, op: Authorization) -> Applied<Outcome> {
        use Disposition::*;
        let stored: Option<Disposition> = super::load::first(
            self.db,
            "SELECT disposition FROM authorizations WHERE id = ?1",
            params![op.id],
            |row| from_name(&super::load::text(row, 0)?),
        )
        .or_io()?;
        if stored != op.from {
            return Err(Conflict::new(format!("authorization {}", op.id), op.from, stored).into());
        }
        let legal = matches!(
            (op.from, op.to),
            (None, Prepared)
                | (Some(Prepared), Submitting | Refused)
                | (Some(Submitting), Submitted | Uncertain)
        );
        require(legal, || {
            format!(
                "authorization {:?} -> {:?} is not a transition",
                op.from, op.to
            )
        })?;
        require(op.change.is_none() || op.to == Submitting, || {
            format!(
                "only Prepared -> Submitting carries a task change, not -> {:?}",
                op.to
            )
        })?;
        let outcome = match &op.change {
            Some((change, form)) => self.write_task(change, *form, Vec::new())?,
            None => Outcome::Applied,
        };
        let refs = blob_ref::scan(&op.doc);
        self.db.execute(
            "INSERT OR REPLACE INTO authorizations(id, task, disposition, doc) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                op.id,
                op.task,
                name(&op.to).or_io()?,
                json(&op.doc).or_io()?
            ],
        )?;
        self.scope(ChangeScope::Record {
            area: "authorizations".to_owned(),
            key: op.id.clone(),
        });
        self.hit("authorization")?;
        self.set_refs(Owner::Authorization(&op.id), refs)?;
        Ok(outcome)
    }
}
