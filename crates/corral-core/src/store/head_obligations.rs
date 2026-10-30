//! Keyed and open obligation rows of a coding run or named review child.
use super::head::TaskHead;
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::work::Counter;
use crate::{DecisionRecord, ResumeReceipt, RunObligations, SlotWait, WaitRecord};
use anyhow::Result;

impl TaskHead {
    fn obligation_rows(
        &self,
        run: &str,
        kind: &str,
        token: Option<String>,
        open: bool,
    ) -> Result<Vec<HistoryRow>> {
        let Some(owner) = self.run_owner(run)? else {
            return Ok(Vec::new());
        };
        self.owner_obligation_rows(&owner, kind, token, open)
    }

    fn owner_obligation_rows(
        &self,
        owner: &str,
        kind: &str,
        token: Option<String>,
        open: bool,
    ) -> Result<Vec<HistoryRow>> {
        let mut query = RowQuery::under(TaskPart::RunObligation, format!("{owner}{kind}"));
        query.index = token;
        query.open_only = open;
        self.query(
            &query,
            if query.index.is_some() { 1 } else { u32::MAX },
            Counter::NestedRowDecodes,
        )
    }

    pub fn wait(&self, run: &str, id: &str) -> Result<Option<WaitRecord>> {
        Ok(
            Self::decode(self.obligation_rows(run, "wait/", Some(format!("id:{id}")), false)?)?
                .into_iter()
                .next(),
        )
    }

    pub fn decision(&self, run: &str, id: &str) -> Result<Option<DecisionRecord>> {
        Ok(Self::decode(self.obligation_rows(
            run,
            "decision/",
            Some(format!("id:{id}")),
            false,
        )?)?
        .into_iter()
        .next())
    }

    /// First decision by request identity in run/child order. A wait with the same id is
    /// not a candidate; its different kind token cannot increase the read budget.
    pub fn decision_by_id(&self, id: &str) -> Result<Option<DecisionRecord>> {
        Ok(Self::decode(self.query(
            &RowQuery::part(TaskPart::RunObligation).with_index(format!("decision:id:{id}")),
            1,
            Counter::NestedRowDecodes,
        )?)?
        .into_iter()
        .next())
    }

    /// The resume receipt `id` of a wait or decision of `run`.
    pub fn receipt(&self, run: &str, id: &str) -> Result<Option<ResumeReceipt>> {
        let rows = self.obligation_rows(run, "", Some(format!("receipt:{id}")), false)?;
        Ok(rows.into_iter().find_map(|row| {
            serde_json::from_value::<ResumeReceipt>(row.doc.get("receipt")?.clone()).ok()
        }))
    }

    fn open_owner_obligations(
        &self,
        owner: &str,
        slot_wait: Option<SlotWait>,
    ) -> Result<RunObligations> {
        Ok(RunObligations {
            slot_wait,
            waits: Self::decode(self.owner_obligation_rows(owner, "wait/", None, true)?)?,
            decisions: Self::decode(self.owner_obligation_rows(owner, "decision/", None, true)?)?,
        })
    }

    /// Waits and decisions of `run` whose observation can still change, in order.
    pub fn open_obligations(&self, run: &str) -> Result<RunObligations> {
        let Some(owner) = self.run_owner(run)? else {
            return Ok(RunObligations::default());
        };
        let slot_wait =
            crate::TaskRef::run_head(self, run)?.and_then(|run| run.obligations.slot_wait);
        self.open_owner_obligations(&owner, slot_wait)
    }

    /// The same indexed open set, scoped to a named review child rather than its worker.
    pub fn open_review_obligations(&self, run: &str, review: &str) -> Result<RunObligations> {
        let Some(owner) = self.owner_key(run, Some(review))? else {
            return Ok(RunObligations::default());
        };
        let slot_wait = self
            .review_child(run, review)?
            .and_then(|child| child.obligations.slot_wait);
        self.open_owner_obligations(&format!("{owner}/"), slot_wait)
    }
}
