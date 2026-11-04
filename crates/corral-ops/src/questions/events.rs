//! Durable question events. Storage atomically queues transitions with question writes;
//! reconciliation reads only that outbox. Event append/import acknowledges keyed identities
//! in the same transaction. Operations and sweeps replay after committing, so missing events
//! survive crashes without hydrating retained questions or event history.
use anyhow::Result;
#[cfg(test)]
use corral_core::Task;
use corral_core::agent::{AgentQuestion, QuestionActor, QuestionTriage};
use corral_core::{Store, TaskRef};
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

fn append_missing(
    store: &Store,
    task: &str,
    key: &str,
    owed: impl IntoIterator<Item = Owed>,
) -> Result<usize> {
    let mut written = 0;
    for event in owed {
        let mut detail = json!({"key": key, "revision": event.revision, "at": event.at});
        if store.question_event_logged(event.kind, &detail)? {
            continue;
        }
        if let (Some(detail), Value::Object(extra)) = (detail.as_object_mut(), event.detail) {
            detail.extend(extra);
        }
        store.log_event(Some(task), event.kind, detail)?;
        written += 1;
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
    let owed = entries.iter().filter_map(owed_entry);
    append_missing(store, parsed.task(), &question.key, owed).map(|_| ())
}

/// Full-task oracle for canonical, still-pending question correlation.
#[cfg(test)]
pub(crate) fn pending_keys(task: &Task) -> Vec<String> {
    super::containers(task)
        .iter()
        .filter(|c| !super::settled(task, c))
        .filter_map(|c| corral_core::agent::container_state(task, c))
        .flat_map(|state| &state.questions)
        .filter(|q| q.blocking && q.recorded_pending())
        .map(|q| q.key.clone())
        .collect()
}

/// Same planning-before-coding order as the full view, restricted to unfinished containers
/// and their pending index. Settled and historical rows cannot affect the notification.
pub(crate) fn pending_keys_ref<T: TaskRef + ?Sized>(task: &T) -> Result<Vec<String>> {
    let mut containers = task.unfinished_native_containers()?;
    containers.sort_by_key(|container| {
        !matches!(
            container,
            corral_core::agent::ContainerRef::Planner { .. }
                | corral_core::agent::ContainerRef::PlanReviewer { .. }
        )
    });
    let mut keys = Vec::new();
    for container in containers {
        if super::settled_ref(task, &container)? || task.agent_head(&container)?.is_none() {
            continue;
        }
        keys.extend(
            task.pending_agent_questions(&container)?
                .into_iter()
                .filter(|question| question.blocking && question.recorded_pending())
                .map(|question| question.key),
        );
    }
    Ok(keys)
}

/// Recovers every owed question event of one task; the sync hook calls it with the sweeps.
pub fn reconcile_events(store: &Store, task_id: &str) -> Result<usize> {
    reconcile_work(store, task_id, None)
}

fn reconcile_work(store: &Store, task: &str, key: Option<&str>) -> Result<usize> {
    let mut written = 0;
    for (kind, detail) in store.owed_question_events(task)? {
        if key.is_some_and(|key| detail["key"].as_str() != Some(key)) {
            continue;
        }
        store.log_event(Some(task), &kind, detail)?;
        written += 1;
    }
    Ok(written)
}

/// Recovers the owed events of one question, after its commit or on replay.
pub fn reconcile_question(store: &Store, key: &str) -> Result<usize> {
    let parsed = QuestionKey::parse(key)?;
    reconcile_work(store, parsed.task(), Some(key))
}
