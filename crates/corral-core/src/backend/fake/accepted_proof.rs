//! Atomic retirement of revision-bound integrity observations on artifact/run changes.
use super::apply::{Applied, Tx};
use crate::accepted_proof::{INTEGRITY_CHECKS, changed_runs};
use crate::backend::{HistoryRow, RecordWrite, TaskNum, TaskPart};

impl Tx<'_> {
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
