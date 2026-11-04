//! Question groups: one batched ask opens 1-4 questions that share `group` (the first member's
//! key). Readers present the members together; one answer command answers several of them, each
//! at its own current revision, so stale-revision protection stays per question.
use anyhow::{Context, Result};
use serde::Deserialize;

use super::{
    Actor, AnswerInput, Applied, QuestionKey, QuestionView, RefusalKind, refuse, show, views,
};
use crate::Ctx;

/// One member's answer in a group answer; `text` falls back to the shared text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupAnswer {
    pub key: String,
    pub rev: u32,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub option: Option<String>,
}

/// Every member of the group that `key` (the group key or any member's key) belongs to, in
/// key order. An ungrouped question is a group of one.
pub fn group(ctx: &Ctx, key: &str) -> Result<Vec<QuestionView>> {
    let view = show(ctx, key)?;
    let Some(group) = view.question.group.clone() else {
        return Ok(vec![view]);
    };
    let parsed = QuestionKey::parse(&group)?;
    let task = ctx.store.get(parsed.task())?;
    Ok(views(&task)
        .into_iter()
        .filter(|v| v.question.group.as_deref() == Some(group.as_str()))
        .collect())
}

/// Answers several members of one group. Every entry is checked first (membership, distinct
/// keys, current revision, pending or, for the owner, answered); then each member is answered
/// through the single-question path. A failure after the first commit names what was answered.
pub fn answer_group(
    ctx: &Ctx,
    key: &str,
    owner: bool,
    shared: &AnswerInput,
    answers: &[GroupAnswer],
) -> Result<Vec<Applied>> {
    if answers.is_empty() {
        return Err(refuse(
            RefusalKind::Bounds,
            "a group answer names at least one member",
        ));
    }
    let members = group(ctx, key)?;
    let mut seen = Vec::new();
    for entry in answers {
        let member = members
            .iter()
            .find(|m| m.question.key == entry.key)
            .ok_or_else(|| {
                refuse(
                    RefusalKind::NotFound,
                    format!("{} is not in the group of {key}", entry.key),
                )
            })?;
        if seen.contains(&entry.key.as_str()) {
            return Err(refuse(
                RefusalKind::Bounds,
                format!("{} is answered twice", entry.key),
            ));
        }
        seen.push(entry.key.as_str());
        if member.question.revision != entry.rev {
            return Err(refuse(
                RefusalKind::StaleRevision,
                format!(
                    "{} is at revision {}, not {}",
                    entry.key, member.question.revision, entry.rev
                ),
            ));
        }
        if entry.text.is_none() && shared.text.trim().is_empty() {
            return Err(refuse(
                RefusalKind::Bounds,
                format!("{} has no answer text and no shared --text", entry.key),
            ));
        }
    }
    let mut applied = Vec::new();
    for entry in answers {
        let input = AnswerInput {
            text: entry.text.clone().unwrap_or_else(|| shared.text.clone()),
            option: entry.option.clone(),
            basis_refs: shared.basis_refs.clone(),
            scope_note: shared.scope_note.clone(),
        };
        let actor = if owner { Actor::Owner } else { Actor::Pm(None) };
        let done: Vec<&str> = answers[..applied.len()]
            .iter()
            .map(|a| a.key.as_str())
            .collect();
        applied.push(
            super::answer(ctx, &entry.key, entry.rev, actor, input)
                .with_context(|| format!("answering {} (already answered: {done:?})", entry.key))?,
        );
    }
    Ok(applied)
}
