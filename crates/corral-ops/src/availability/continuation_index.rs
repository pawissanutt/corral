//! Transactional pointers into canonical intents. No role path enumerates historical intents.
//! Missing indexes fail closed when a canonical intent is addressed directly; financial
//! episode ownership remains the authoritative fallback when preparing a successor.
use super::*;

const SOURCES: RecordArea<Pointer> = RecordArea::new("role-continuation-sources");
const CONTAINERS: RecordArea<Pointer> = RecordArea::new("role-continuation-containers");
const TASKS: RecordArea<Pointer> = RecordArea::new("role-continuation-tasks");

#[derive(Serialize, Deserialize)]
struct Pointer {
    intent: String,
    created: Timestamp,
}

fn container_key(container: &ContainerRef) -> Result<String> {
    Ok(agent::paths::sha256_hex(&serde_json::to_vec(container)?))
}
fn task_key(task: &str, role: AgentRole) -> Result<String> {
    Ok(agent::paths::sha256_hex(&serde_json::to_vec(&(
        task, role,
    ))?))
}
fn load(
    store: &Store,
    area: &RecordArea<Pointer>,
    key: &str,
) -> Result<Option<ContinuationIntent>> {
    area.load(store, key)?
        .map(|pointer| {
            let intent = INTENTS
                .load(store, &pointer.intent)?
                .context("SuccessorAmbiguous: continuation index has no intent")?;
            ensure!(
                intent.version == 1
                    && pointer.intent
                        == super::key(&intent.successor.container, intent.successor.generation)?
                    && pointer.created == intent.created,
                "SuccessorAmbiguous: invalid continuation index"
            );
            validate(store, &intent)?;
            Ok(intent)
        })
        .transpose()
}

/// One keyed read when absent; present intents validate a bounded set of keyed pointers.
pub(crate) fn from_source(
    store: &Store,
    source: &ContainerRef,
) -> Result<Option<ContinuationIntent>> {
    let intent = load(store, &SOURCES, &container_key(source)?)?;
    ensure!(
        intent
            .as_ref()
            .is_none_or(|i| i.source.container == *source),
        "SuccessorAmbiguous: foreign source index"
    );
    Ok(intent)
}
pub(crate) fn latest_for(
    store: &Store,
    container: &ContainerRef,
) -> Result<Option<ContinuationIntent>> {
    let intent = load(store, &CONTAINERS, &container_key(container)?)?;
    ensure!(intent.as_ref().is_none_or(|i| i.source.container == *container || i.successor.container == *container), "SuccessorAmbiguous: foreign container index");
    Ok(intent)
}
pub(crate) fn task_intent(
    store: &Store,
    task: &str,
    role: AgentRole,
) -> Result<Option<ContinuationIntent>> {
    let intent = load(store, &TASKS, &task_key(task, role)?)?;
    ensure!(
        intent
            .as_ref()
            .is_none_or(|i| i.subject.task.as_deref() == Some(task) && i.role == role),
        "SuccessorAmbiguous: foreign task index"
    );
    Ok(intent)
}

fn advance(
    store: &Store,
    area: &RecordArea<Pointer>,
    key: &str,
    pointer: &Pointer,
    exclusive: bool,
) -> Result<()> {
    if let Some(previous) = area.load(store, key)? {
        if previous.intent == pointer.intent || previous.created > pointer.created {
            return Ok(());
        }
        if exclusive {
            let old = INTENTS
                .load(store, &previous.intent)?
                .context("SuccessorAmbiguous: missing indexed intent")?;
            ensure!(
                old.phase != IntentPhase::Prepared,
                "SuccessorAmbiguous: another successor is prepared"
            );
        }
    }
    area.save(store, key, pointer)
}

/// Called by canonical save in the same record/financial transaction on every phase change.
pub(super) fn publish(store: &Store, intent: &ContinuationIntent) -> Result<()> {
    let pointer = Pointer {
        intent: super::key(&intent.successor.container, intent.successor.generation)?,
        created: intent.created,
    };
    advance(
        store,
        &SOURCES,
        &container_key(&intent.source.container)?,
        &pointer,
        false,
    )?;
    advance(
        store,
        &CONTAINERS,
        &container_key(&intent.source.container)?,
        &pointer,
        false,
    )?;
    if intent.source.container != intent.successor.container {
        advance(
            store,
            &CONTAINERS,
            &container_key(&intent.successor.container)?,
            &pointer,
            false,
        )?;
    }
    if let Some(task) = intent.subject.task.as_deref() {
        advance(store, &TASKS, &task_key(task, intent.role)?, &pointer, true)?;
    }
    Ok(())
}

/// Historical generations can have newer pointers, but an absent/stale pointer is never
/// treated as permission to activate or prepare an unindexed canonical successor.
pub(super) fn validate(store: &Store, intent: &ContinuationIntent) -> Result<()> {
    let present = |area: &RecordArea<Pointer>, key: &str| -> Result<()> {
        ensure!(
            area.load(store, key)?
                .is_some_and(|p| p.created >= intent.created),
            "SuccessorAmbiguous: missing continuation index"
        );
        Ok(())
    };
    let source_key = container_key(&intent.source.container)?;
    present(&SOURCES, &source_key)?;
    present(&CONTAINERS, &source_key)?;
    if intent.source.container != intent.successor.container {
        present(&CONTAINERS, &container_key(&intent.successor.container)?)?;
    }
    if let Some(task) = intent.subject.task.as_deref() {
        present(&TASKS, &task_key(task, intent.role)?)?;
    }
    Ok(())
}

pub(super) fn matching(
    intent: ContinuationIntent,
    source: &ContainerRef,
    successor: &ContainerRef,
    target: &ConnectionBinding,
    subject: &SubjectRevision,
    context: &ContextSource,
) -> Result<ContinuationIntent> {
    ensure!(
        intent.source.container == *source
            && intent.successor.container == *successor
            && intent.successor.binding == *target
            && intent.subject == *subject
            && intent.context == *context
            && matches!(intent.phase, IntentPhase::Prepared | IntentPhase::Active),
        "SuccessorAmbiguous"
    );
    Ok(intent)
}
