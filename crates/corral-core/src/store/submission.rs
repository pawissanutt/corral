//! Authorization documents and the deterministic submission test barriers.

use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};

use super::Store;
use crate::backend::{Authorization, Disposition, Operation, PageReq, StorageQuery, StorageRow};
use crate::{AuthorizedSubmission, SubmissionDisposition};

#[derive(Clone)]
pub(super) struct SubmissionBarrier(Arc<dyn Fn(&str) + Send + Sync>);

impl std::fmt::Debug for SubmissionBarrier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SubmissionBarrier")
    }
}

fn disposition(value: &SubmissionDisposition) -> Disposition {
    match value {
        SubmissionDisposition::Prepared => Disposition::Prepared,
        SubmissionDisposition::Submitting => Disposition::Submitting,
        SubmissionDisposition::Submitted => Disposition::Submitted,
        SubmissionDisposition::Uncertain { .. } => Disposition::Uncertain,
        SubmissionDisposition::AuthorizationRefused { .. } => Disposition::Refused,
    }
}

impl Store {
    /// Installs a callback at submission commit boundaries for deterministic crash fixtures.
    #[doc(hidden)]
    pub fn with_submission_barrier(&self, barrier: Arc<dyn Fn(&str) + Send + Sync>) -> Store {
        let mut store = self.clone();
        store.submission_barrier = Some(SubmissionBarrier(barrier));
        store
    }

    pub(super) fn submission_barrier(&self, point: &str) {
        if let Some(barrier) = &self.submission_barrier {
            (barrier.0)(point);
        }
    }

    /// Marks a nontransport effect boundary in deterministic submission fixtures.
    #[doc(hidden)]
    pub fn submission_effect_barrier(&self, point: &str) {
        self.submission_barrier(point);
    }

    pub(crate) fn submission_record(&self, id: &str) -> Result<Option<AuthorizedSubmission>> {
        ensure!(
            id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid action authorization identity"
        );
        let view = self.view()?;
        let found = view.storage_rows(
            &StorageQuery::Authorization { id: id.to_owned() },
            PageReq::first(1),
        )?;
        let Some(row) = found.items.into_iter().next() else {
            return Ok(None);
        };
        let StorageRow::Authorization(row) = row else {
            bail!("authorization lookup returned another storage row");
        };
        if crate::work::in_hot_scope() {
            crate::work::decoded(
                crate::work::Counter::RecordDecodes,
                super::read::doc_bytes(&row.doc),
                crate::work::Subject::Plain,
            );
        }
        let record: AuthorizedSubmission =
            serde_json::from_value(row.doc).context("decoding authorization")?;
        ensure!(
            row.id == id
                && record.version == 1
                && record.authorization.id == id
                && record.authorization.action.task == row.task
                && disposition(&record.disposition) == row.disposition,
            "mismatched submission record"
        );
        Ok(Some(record))
    }

    pub(super) fn authorization_operation(
        &self,
        record: &AuthorizedSubmission,
    ) -> Result<Authorization> {
        let before = self.submission_record(&record.authorization.id)?;
        Ok(Authorization {
            id: record.authorization.id.clone(),
            task: record.authorization.action.task.clone(),
            from: before.as_ref().map(|old| disposition(&old.disposition)),
            to: disposition(&record.disposition),
            doc: serde_json::to_value(record).context("encoding authorization")?,
            change: None,
        })
    }

    pub(crate) fn save_submission_record(&self, record: &AuthorizedSubmission) -> Result<()> {
        ensure!(
            !self.is_snapshot(),
            "a read-only snapshot cannot save authorizations"
        );
        self.check_runtime()?;
        if self.submission_record(&record.authorization.id)?.as_ref() == Some(record) {
            return Ok(());
        }
        let mut operation = self.authorization_operation(record)?;
        if operation.from == Some(Disposition::Prepared) && operation.to == Disposition::Submitted {
            // Local plan acceptance can recover an approval saved before its receipt.
            // Its caller rechecks that exact approval and current authority under leases;
            // there is no transport to replay. Keep both backend transitions explicit.
            ensure!(
                record.authorization.action.kind == crate::OperatingActionKind::AcceptPlan
                    && matches!(
                        record.authorization.origin,
                        crate::OperatingOrigin::Automatic { .. }
                    ),
                "only local automatic plan acceptance can finish a prepared receipt"
            );
            let mut pending = record.clone();
            pending.disposition = SubmissionDisposition::Submitting;
            self.save_submission_record(&pending)?;
            operation.from = Some(Disposition::Submitting);
        }
        self.db
            .backend
            .apply(Operation::Authorization(operation))
            .map_err(super::write::apply_error)
            .context("saving authorization")?;
        Ok(())
    }
}
