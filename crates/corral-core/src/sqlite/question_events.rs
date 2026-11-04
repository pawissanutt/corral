//! One-time additive backfill. The schema transaction makes the marker and all derived
//! records atomic; subsequent opens do no question/event scan.
use super::{codec::*, load, tx::Tx};

#[cfg(test)]
#[path = "question_events_tests.rs"]
mod tests;
use crate::backend::{question_events::QuestionEventTx, *};

impl Tx<'_> {
    pub(super) fn backfill_question_events(&mut self) -> Result<(), ApplyError> {
        let mut events = self.db.prepare(
            "SELECT seq, ts, task, kind, actor, via, run, detail, malformed FROM events WHERE kind LIKE 'question_%' ORDER BY seq"
        )?;
        let mut rows = events.query([])?;
        while let Some(row) = rows.next()? {
            let event = load::event(row).or_io()?;
            if !event.malformed {
                self.acknowledge_question(&event.event, event.seq)?;
            }
        }
        let mut questions = self.db.prepare(
            "SELECT tasks.id, r.part, r.key, r.current, r.open, r.doc, r.idx FROM task_rows r JOIN tasks ON tasks.num = r.task WHERE r.part = ?1 AND r.key LIKE '%/questions/%'"
        )?;
        let mut rows = questions.query([name(&TaskPart::AgentRecord).or_io()?])?;
        while let Some(row) = rows.next()? {
            let task = load::text(row, 0).or_io()?;
            let question = HistoryRow {
                part: TaskPart::AgentRecord,
                key: load::text(row, 2).or_io()?,
                current: load::flag(row, 3).or_io()?,
                open: load::flag(row, 4).or_io()?,
                doc: load::doc(row, 5).or_io()?,
                index: parse_json(&load::text(row, 6).or_io()?).or_io()?,
            };
            self.enqueue_question(&task, &question)?;
        }
        Ok(())
    }
}
