//! Derived durable outbox and acknowledgements for native question transitions.
//! Both backends call these helpers inside their row/event transaction (including import).
use super::{ApplyError, EventSeq, HistoryRow, NewEvent, RecordWrite, StoredRecord, TaskPart};
use crate::agent::{AgentQuestion, QuestionActor};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) const ACKS: &str = "question-event-acks";
pub(crate) fn area(task: &str) -> String {
    format!("question-event-work/{task}")
}

pub(crate) fn identity(kind: &str, detail: &Value) -> Option<String> {
    if !matches!(
        kind,
        "question_opened"
            | "question_answered"
            | "question_escalated"
            | "question_expired"
            | "question_cancelled"
            | "question_withdrawn"
            | "question_unattended"
            | "question_deadline_set"
    ) {
        return None;
    }
    let tuple = (
        kind,
        detail["key"].as_str()?,
        detail["revision"].as_u64()?,
        detail["at"].as_str()?,
    );
    Some(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&tuple).ok()?)
    ))
}

/// Bounded by the question's retained triage; unrelated history is never decoded.
pub(crate) fn transitions(row: &HistoryRow) -> anyhow::Result<Vec<(String, Value)>> {
    if row.part != TaskPart::AgentRecord || !row.key.contains("/questions/") {
        return Ok(Vec::new());
    }
    let q: AgentQuestion = serde_json::from_value(row.doc.clone())?;
    let mut events = vec![(
        "question_opened".to_owned(),
        json!({
            "key": q.key, "revision": 1, "at": q.asked.at.to_string(),
            "request_id": q.asked.request_id, "blocking": q.blocking
        }),
    )];
    for entry in &q.triage {
        let kind = match entry.action.as_str() {
            "answer" | "supersede" => "question_answered",
            "escalate" => "question_escalated",
            "expire" => "question_expired",
            "cancel" | "terminalize" => "question_cancelled",
            "withdraw" => "question_withdrawn",
            "unattended" => "question_unattended",
            "deadline" => "question_deadline_set",
            _ => continue,
        };
        let by = match entry.by {
            QuestionActor::Agent => "agent",
            QuestionActor::Pm => "pm",
            QuestionActor::Owner => "owner",
            QuestionActor::System => "corral",
        };
        let mut detail = json!({"key": q.key, "revision": entry.revision,
            "at": entry.at.to_string(), "by": by});
        if entry.action == "supersede" {
            detail["supersedes_revision"] = json!(entry.revision.saturating_sub(1));
        }
        if let Some(note) = &entry.note {
            detail["note"] = json!(note);
        }
        events.push((kind.to_owned(), detail));
    }
    Ok(events)
}

type Applied<T> = Result<T, ApplyError>;
pub(crate) trait QuestionEventTx {
    fn question_record(&self, area: &str, key: &str) -> Applied<Option<StoredRecord>>;
    fn question_put(&mut self, write: RecordWrite) -> Applied<()>;
    fn question_now(&self) -> jiff::Timestamp;

    fn question_import_skip(&self, row: &StoredRecord) -> Applied<bool> {
        if row.area != ACKS && !row.area.starts_with("question-event-work/") {
            return Ok(false);
        }
        Ok(self.question_record(&row.area, &row.key)?.is_some()
            || (row.area != ACKS && self.question_record(ACKS, &row.key)?.is_some()))
    }

    fn enqueue_question(&mut self, task: &str, row: &HistoryRow) -> Applied<()> {
        let area = area(task);
        for (order, (kind, detail)) in transitions(row)
            .map_err(ApplyError::Io)?
            .into_iter()
            .enumerate()
        {
            let Some(key) = identity(&kind, &detail) else {
                continue;
            };
            if self.question_record(ACKS, &key)?.is_some()
                || self.question_record(&area, &key)?.is_some()
            {
                continue;
            }
            self.question_put(RecordWrite {
                area: area.clone(),
                key,
                expected: None,
                doc: Some(json!({"kind": kind, "detail": detail, "order": order})),
                updated: self.question_now(),
            })?;
        }
        Ok(())
    }

    fn question_logged(&self, event: &NewEvent) -> Applied<Option<EventSeq>> {
        let Some(key) = identity(&event.kind, &event.detail) else {
            return Ok(None);
        };
        Ok(self
            .question_record(ACKS, &key)?
            .and_then(|r| r.doc.as_u64())
            .map(EventSeq))
    }

    fn acknowledge_question(&mut self, event: &NewEvent, seq: EventSeq) -> Applied<()> {
        let Some(key) = identity(&event.kind, &event.detail) else {
            return Ok(());
        };
        if self.question_record(ACKS, &key)?.is_none() {
            self.question_put(RecordWrite {
                area: ACKS.into(),
                key: key.clone(),
                expected: None,
                doc: Some(json!(seq.0)),
                updated: self.question_now(),
            })?;
        }
        if let Some(task) = &event.task {
            let area = area(task);
            if let Some(row) = self.question_record(&area, &key)? {
                self.question_put(RecordWrite {
                    area,
                    key,
                    expected: Some(row.version),
                    doc: None,
                    updated: self.question_now(),
                })?;
            }
        }
        Ok(())
    }
}
