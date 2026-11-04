//! Output-sized access to the durable native question-event outbox.
use crate::{Store, backend::question_events};
use anyhow::Result;
use serde_json::Value;

impl Store {
    /// Only committed, unacknowledged transitions; no task or event history hydration.
    pub fn owed_question_events(&self, task: &str) -> Result<Vec<(String, Value)>> {
        let mut rows = self.record_rows(&question_events::area(task))?;
        rows.sort_by_key(|row| {
            (
                row.doc["detail"]["key"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                row.doc["detail"]["at"]
                    .as_str()
                    .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                    .unwrap_or(jiff::Timestamp::UNIX_EPOCH),
                row.doc["detail"]["revision"].as_u64().unwrap_or_default(),
                row.doc["order"].as_u64().unwrap_or_default(),
            )
        });
        rows.into_iter()
            .map(|row| {
                let kind = row.doc["kind"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid question event kind"))?
                    .to_owned();
                Ok((kind, row.doc["detail"].clone()))
            })
            .collect()
    }

    /// Keyed identity lookup, also used before draining a retained triage entry.
    pub fn question_event_logged(&self, kind: &str, detail: &Value) -> Result<bool> {
        let Some(key) = question_events::identity(kind, detail) else {
            return Ok(false);
        };
        Ok(self.record_row(question_events::ACKS, &key)?.is_some())
    }
}
