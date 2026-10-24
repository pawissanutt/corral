//! Durable question events. A question's ask and every triage entry are committed with the
//! mutation, so they are the owed post-commit effects: `reconcile` appends each event that the
//! log lacks, keyed by kind, question key, revision and timestamp. Every operation reconciles
//! after its commit and on replay, and the sweeps reconcile their task, so an event lost to a
//! crash or a failed write is recovered and never written before the task commit.
use std::collections::HashSet;

use anyhow::Result;
use corral_core::agent::{AgentQuestion, QuestionActor, QuestionTriage};
use corral_core::{Store, Task};
use serde_json::{Value, json};

use super::QuestionKey;

pub const QUESTION_EVENT_KINDS: [&str; 8] = [
    "question_opened",
    "question_answered",
    "question_escalated",
    "question_expired",
    "question_cancelled",
    "question_withdrawn",
    "question_unattended",
    "question_deadline_set",
];

struct Owed {
    kind: &'static str,
    revision: u32,
    at: String,
    detail: Value,
}

fn actor(by: QuestionActor) -> &'static str {
    match by {
        QuestionActor::Agent => "agent",
        QuestionActor::Pm => "pm",
        QuestionActor::Owner => "owner",
        QuestionActor::System => "corral",
    }
}

fn opened(question: &AgentQuestion) -> Owed {
    Owed {
        kind: "question_opened",
        revision: 1,
        at: question.asked.at.to_string(),
        detail: json!({"request_id": question.asked.request_id, "blocking": question.blocking}),
    }
}

fn owed_entry(entry: &QuestionTriage) -> Option<Owed> {
    let mut detail = json!({"by": actor(entry.by)});
    let kind = match entry.action.as_str() {
        "answer" => "question_answered",
        "supersede" => {
            detail["supersedes_revision"] = json!(entry.revision.saturating_sub(1));
            "question_answered"
        }
        "escalate" => "question_escalated",
        "expire" => "question_expired",
        "cancel" | "terminalize" => "question_cancelled",
        "withdraw" => "question_withdrawn",
        "unattended" => "question_unattended",
        "deadline" => "question_deadline_set",
        _ => return None,
    };
    if let Some(note) = &entry.note {
        detail["note"] = json!(note);
    }
    Some(Owed {
        kind,
        revision: entry.revision,
        at: entry.at.to_string(),
        detail,
    })
}

type Logged = HashSet<(String, String, u64, String)>;

fn logged(store: &Store, task: &str) -> Result<Logged> {
    Ok(store
        .events_shared()?
        .iter()
        .filter(|e| e.task.as_deref() == Some(task))
        .filter(|e| QUESTION_EVENT_KINDS.contains(&e.kind.as_str()))
        .map(|e| {
            let field = |name: &str| e.detail[name].as_str().unwrap_or_default().to_string();
            let revision = e.detail["revision"].as_u64().unwrap_or_default();
            (e.kind.clone(), field("key"), revision, field("at"))
        })
        .collect())
}

fn append_missing(
    store: &Store,
    task: &str,
    key: &str,
    owed: impl IntoIterator<Item = Owed>,
    logged: &Logged,
) -> Result<usize> {
    let mut written = 0;
    for event in owed {
        let id = (
            event.kind.to_string(),
            key.to_string(),
            u64::from(event.revision),
            event.at.clone(),
        );
        if logged.contains(&id) {
            continue;
        }
        let mut detail = json!({"key": key, "revision": event.revision, "at": event.at});
        if let (Some(detail), Value::Object(extra)) = (detail.as_object_mut(), event.detail) {
            detail.extend(extra);
        }
        store.log_event(Some(task), event.kind, detail)?;
        written += 1;
    }
    Ok(written)
}

/// Appends the missing events of the named questions of one task (all when `keys` is `None`).
fn reconcile_task(store: &Store, task: &Task, keys: Option<&[String]>) -> Result<usize> {
    let questions: Vec<&AgentQuestion> = super::containers(task)
        .iter()
        .filter_map(|c| corral_core::agent::container_state(task, c))
        .flat_map(|state| &state.questions)
        .filter(|q| keys.is_none_or(|keys| keys.contains(&q.key)))
        .collect();
    if questions.is_empty() {
        return Ok(0);
    }
    let logged = logged(store, &task.id)?;
    let mut written = 0;
    for question in questions {
        let owed =
            std::iter::once(opened(question)).chain(question.triage.iter().filter_map(owed_entry));
        written += append_missing(store, &task.id, &question.key, owed, &logged)?;
    }
    Ok(written)
}

/// Logs the still-owed events of triage entries about to be evicted from `question`.
pub(super) fn drain(
    store: &Store,
    question: &AgentQuestion,
    entries: &[QuestionTriage],
) -> Result<()> {
    let parsed = QuestionKey::parse(&question.key)?;
    let logged = logged(store, parsed.task())?;
    let owed = entries.iter().filter_map(owed_entry);
    append_missing(store, parsed.task(), &question.key, owed, &logged).map(|_| ())
}

/// Recovers every owed question event of one task; the sync hook calls it with the sweeps.
pub fn reconcile_events(store: &Store, task_id: &str) -> Result<usize> {
    reconcile_task(store, &store.get(task_id)?, None)
}

/// Recovers the owed events of one question, after its commit or on replay.
pub fn reconcile_question(store: &Store, key: &str) -> Result<usize> {
    let parsed = QuestionKey::parse(key)?;
    let task = store.get(parsed.task())?;
    reconcile_task(store, &task, Some(&[key.to_string()]))
}
