//! The one submission protocol (plan T-253 section 4, "Head-form submission chain") over the
//! full task and over a head: replay refusal, strict policy revalidation with refusals
//! recorded, one `Authorization` operation carrying the task change and every buffered event
//! and record, the send callback once, then Submitted or Uncertain.

use anyhow::{Result, bail};

use super::Store;
use super::head_mut::TaskHeadMut;
use super::write::Held;
use crate::{
    ActionAuthorization, OperatingActionIdentity, SubmissionDisposition, SubmissionOutcome, Task,
    TaskRef,
};

pub(super) trait SubmissionTask: TaskRef + Sized {
    type Held;
    fn hold(store: &Store, id: &str) -> Result<(Self::Held, Self)>;
    fn commit(store: &Store, held: &Self::Held, task: Self) -> Result<()>;
}

impl SubmissionTask for Task {
    type Held = Held;

    fn hold(store: &Store, id: &str) -> Result<(Held, Task)> {
        let held = store.hold(id)?;
        let task = held.task();
        Ok((held, task))
    }

    fn commit(store: &Store, held: &Held, task: Task) -> Result<()> {
        store.save_held(held, &task).map(drop)
    }
}

impl SubmissionTask for TaskHeadMut {
    type Held = ();

    fn hold(store: &Store, id: &str) -> Result<((), TaskHeadMut)> {
        Ok(((), TaskHeadMut::new(store.head(id)?)))
    }

    fn commit(store: &Store, (): &(), head: TaskHeadMut) -> Result<()> {
        store.write_head(head).map(drop)
    }
}

impl Store {
    pub(super) fn submit_in<M: SubmissionTask, R>(
        &self,
        authorization: &ActionAuthorization,
        expected: &OperatingActionIdentity,
        prepare: impl FnOnce(&Store, &mut M) -> Result<()>,
        submit: impl FnOnce() -> Result<R>,
    ) -> Result<SubmissionOutcome<R>> {
        anyhow::ensure!(
            Self::normalize_id(&expected.task)? == expected.task,
            "submission needs canonical task identity"
        );
        self.submission_barrier("submission_entered");
        let _board = self.lock("dispatch")?;
        let _task = self.lock(&expected.task)?;
        let mut record = self.authorized_submission(&authorization.id)?;
        anyhow::ensure!(
            record.authorization == *authorization && authorization.action == *expected,
            "submission authorization does not match exact task/run/attempt"
        );
        match record.disposition {
            SubmissionDisposition::Prepared
            | SubmissionDisposition::AuthorizationRefused { .. } => {}
            SubmissionDisposition::Submitting
            | SubmissionDisposition::Submitted
            | SubmissionDisposition::Uncertain { .. } => {
                bail!("submission already attempted; refusing replay")
            }
        }
        let (held, mut task) = <M as SubmissionTask>::hold(self, &expected.task)?;
        let validation = self.operating_config().and_then(|config| {
            self.validate_operating_action(&config, &task, authorization)?;
            Ok(config)
        });
        let config = match validation {
            Ok(config) => config,
            Err(error) => return self.refuse(&mut record, format!("{error:#}")),
        };
        let mut fresh = self.clone();
        fresh.config = config;
        let unit = fresh.event_unit();
        if let Err(error) = prepare(&fresh, &mut task) {
            drop(unit);
            return self.refuse(
                &mut record,
                format!("submission precondition refused: {error:#}"),
            );
        }
        fresh.submission_barrier("prepare_returned");
        record.disposition = SubmissionDisposition::Submitting;
        fresh.buffer_authorization(fresh.authorization_operation(&record)?)?;
        M::commit(&fresh, &held, task)?;
        drop(unit);
        fresh.submission_barrier("authorization_committed");
        let sent = submit();
        fresh.submission_barrier("send_returned");
        match sent {
            Ok(result) => {
                record.disposition = SubmissionDisposition::Submitted;
                self.save_authorized_submission(&record)?;
                fresh.submission_barrier("submitted_committed");
                Ok(SubmissionOutcome::Submitted(result))
            }
            Err(error) => {
                record.disposition = SubmissionDisposition::Uncertain {
                    reason: format!("{error:#}"),
                };
                self.save_authorized_submission(&record)?;
                fresh.submission_barrier("uncertain_committed");
                Err(error)
            }
        }
    }

    fn refuse<R>(
        &self,
        record: &mut crate::AuthorizedSubmission,
        reason: String,
    ) -> Result<SubmissionOutcome<R>> {
        record.disposition = SubmissionDisposition::AuthorizationRefused {
            reason: reason.clone(),
        };
        self.save_authorized_submission(record)?;
        self.submission_barrier("refused_committed");
        Ok(SubmissionOutcome::AuthorizationRefused(reason))
    }

    /// The head form of [`Store::submit_operating_action`]: the same protocol, with `prepare`
    /// mutating the acted task's head instead of the full task.
    ///
    /// # Errors
    /// Returns lookup, lock, validation, commit or send errors; refusals are outcomes.
    pub fn submit_operating_action_head<R>(
        &self,
        authorization: &ActionAuthorization,
        expected: &OperatingActionIdentity,
        prepare: impl FnOnce(&Store, &mut TaskHeadMut) -> Result<()>,
        submit: impl FnOnce() -> Result<R>,
    ) -> Result<SubmissionOutcome<R>> {
        self.submit_in(authorization, expected, prepare, submit)
    }
}
