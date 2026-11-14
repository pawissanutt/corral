//! Explicit cross-container financial commit. The callback that computed these documents is
//! NOT the transaction: backends CAS the episode plus every affected native row/task here.
use super::*;
use crate::paid_episode::{AREA, PaidEpisode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitPaidEpisode {
    pub episode: RecordWrite,
    pub task: Option<UpdateTask>,
    pub records: PutRecords,
}
impl CommitPaidEpisode {
    pub fn reservations(&self) -> Vec<ReservationUse> {
        self.records
            .reservations
            .iter()
            .chain(self.task.iter().flat_map(|t| &t.change.reservations))
            .cloned()
            .collect()
    }
    pub fn validate(&self) -> Result<(), ApplyError> {
        let episode: PaidEpisode = self
            .episode
            .doc
            .clone()
            .ok_or_else(|| ApplyError::precondition("paid episode cannot be deleted"))
            .and_then(|doc| {
                serde_json::from_value(doc).map_err(|e| ApplyError::precondition(e.to_string()))
            })?;
        episode
            .validate()
            .map_err(|e| ApplyError::precondition(e.to_string()))?;
        if self.episode.area != AREA
            || self.episode.key != episode.id
            || self
                .records
                .writes
                .iter()
                .chain(self.task.iter().flat_map(|t| &t.change.records))
                .any(|w| w.area == AREA)
        {
            return Err(ApplyError::precondition(
                "one canonical episode per financial transaction",
            ));
        }
        Ok(())
    }
}
impl Operation {
    /// Store callbacks can stage their ordinary native/task writes. Selecting this explicit
    /// operation does not weaken their existing CAS or task-revision preconditions.
    pub(crate) fn with_paid_episode(self) -> anyhow::Result<Self> {
        let mut operation = self;
        let writes = match &mut operation {
            Self::PutRecords(records) => &mut records.writes,
            Self::UpdateTask(task) => &mut task.change.records,
            _ => return Ok(operation),
        };
        let positions: Vec<_> = writes
            .iter()
            .enumerate()
            .filter(|(_, w)| w.area == AREA)
            .map(|(i, _)| i)
            .collect();
        let Some(&index) = positions.first() else {
            return Ok(operation);
        };
        anyhow::ensure!(positions.len() == 1, "one paid episode per commit");
        let episode = writes.remove(index);
        let (task, records) = match operation {
            Self::PutRecords(records) => (None, records),
            Self::UpdateTask(task) => (
                Some(task),
                PutRecords {
                    writes: Vec::new(),
                    events: Vec::new(),
                    reservations: Vec::new(),
                },
            ),
            _ => unreachable!(),
        };
        let paid = CommitPaidEpisode {
            episode,
            task,
            records,
        };
        paid.validate().map_err(anyhow::Error::new)?;
        Ok(Self::CommitPaidEpisode(paid))
    }
}
