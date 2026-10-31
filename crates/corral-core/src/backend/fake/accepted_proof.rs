//! Atomic retirement of revision-bound integrity observations on artifact/run changes.
use super::apply::{Applied, Tx};
use crate::accepted_proof::{INTEGRITY_CHECKS, changed_runs};
use crate::backend::{HistoryRow, RecordWrite, TaskNum, TaskPart};

impl Tx<'_> {
    pub(super) fn maintain_artifact_count(
        &mut self,
        task: TaskNum,
        part: TaskPart,
        key: &str,
        after: Option<&HistoryRow>,
    ) -> Applied<()> {
        if part != TaskPart::Artifact {
            return Ok(());
        }
        let changes = crate::backend::artifact_counts::changes(
            self.row(task, part, key).map(|r| &r.doc),
            after.map(|r| &r.doc),
        );
        for (run, delta) in changes {
            let key = format!("{}/{run}", self.task_id(task)?);
            let stored = self
                .state
                .records
                .get(&(crate::backend::artifact_counts::AREA.into(), key.clone()));
            let doc = crate::backend::artifact_counts::adjusted(stored.map(|r| &r.doc), delta)
                .map_err(crate::backend::ApplyError::Io)?;
            let write = RecordWrite {
                area: crate::backend::artifact_counts::AREA.into(),
                key,
                expected: stored.map(|r| r.version),
                doc: Some(doc),
                updated: self.now,
            };
            self.put_record(&write)?;
        }
        Ok(())
    }

    pub(super) fn invalidate_accepted_proof(
        &mut self,
        task: TaskNum,
        part: TaskPart,
        key: &str,
        after: Option<&HistoryRow>,
    ) -> Applied<()> {
        let runs = changed_runs(self.row(task, part, key), after);
        for run in runs {
            let key = format!("{}/{run}", self.task_id(task)?);
            if let Some(record) = self
                .state
                .records
                .get(&(INTEGRITY_CHECKS.into(), key.clone()))
            {
                let write = RecordWrite {
                    area: INTEGRITY_CHECKS.into(),
                    key,
                    expected: Some(record.version),
                    doc: None,
                    updated: self.now,
                };
                self.put_record(&write)?;
            }
        }
        Ok(())
    }
}
