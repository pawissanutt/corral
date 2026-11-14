//! Bounds for typed consequences and canonical evidence references. Evidence is a reference,
//! not permission to read a path: app readers resolve it through the owning canonical service.
use corral_core::agent::{
    QUESTION_CONTEXT_REFS_MAX, QUESTION_EVIDENCE_MAX, QUESTION_OPTIONS_MAX, QuestionEvidenceRef,
};

use super::{
    AskPayload, NOTE_MAX_BYTES, OPTION_MAX_BYTES, QuestionRefusal, REF_MAX_BYTES, RefusalKind,
    bounded,
};

pub(super) fn validate(payload: &AskPayload) -> Result<(), QuestionRefusal> {
    let bad = |reason| QuestionRefusal::new(RefusalKind::Bounds, reason);
    if payload.option_details.len() > QUESTION_OPTIONS_MAX
        || payload.evidence.len() > QUESTION_EVIDENCE_MAX
        || payload.owner_decisions.len() > QUESTION_CONTEXT_REFS_MAX
    {
        return Err(bad(
            "too many option consequences, evidence refs or owner decisions",
        ));
    }
    for (index, option) in payload.option_details.iter().enumerate() {
        bounded(&option.label, OPTION_MAX_BYTES, "option label")?;
        if !payload.options.contains(&option.label)
            || payload.option_details[..index]
                .iter()
                .any(|prior| prior.label == option.label)
        {
            return Err(bad(
                "option consequences must name distinct declared options",
            ));
        }
        for value in [&option.next, &option.cost].into_iter().flatten() {
            bounded(value, NOTE_MAX_BYTES, "option consequence")?;
        }
        if option.unblocks.len() > QUESTION_CONTEXT_REFS_MAX
            || option
                .evidence
                .is_some_and(|index| index >= payload.evidence.len())
        {
            return Err(bad("option unblocks or evidence index is out of bounds"));
        }
        for id in &option.unblocks {
            bounded(id, REF_MAX_BYTES, "unblocked reference")?;
        }
    }
    for reference in &payload.evidence {
        let check = |value: &str| bounded(value, REF_MAX_BYTES, "evidence reference");
        match reference {
            QuestionEvidenceRef::Task { id } => check(id)?,
            QuestionEvidenceRef::Plan { task, id }
            | QuestionEvidenceRef::Artifact { task, id }
            | QuestionEvidenceRef::Decision { task, id } => {
                check(task)?;
                check(id)?;
            }
            QuestionEvidenceRef::Run { task, run } | QuestionEvidenceRef::Log { task, run } => {
                check(task)?;
                check(run)?;
            }
            QuestionEvidenceRef::Diff { task, run, path } => {
                check(task)?;
                check(run)?;
                check(path)?;
            }
        }
    }
    for decision in &payload.owner_decisions {
        bounded(decision, REF_MAX_BYTES, "owner decision")?;
    }
    Ok(())
}
