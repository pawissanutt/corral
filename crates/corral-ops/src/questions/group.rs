//! Question groups: one batched ask opens 1-4 questions that share `group` (the first member's
//! key). Readers present the members together; `triage::answer_group` answers several of them in
//! one transaction, each at its own current revision, so stale-revision protection stays per
//! question.
use anyhow::{Context, Result};
use corral_core::agent::RequestReceipt;
use serde::Deserialize;

use super::{Actor, AnswerInput, QuestionKey, QuestionView, show, views};
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

/// The corral-pi PM's group form of `question_answer`: `answers` instead of `rev`/`option`, each
/// member at its own revision, sharing text, basis refs and scope note.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PmAnswerGroup {
    pub key: String,
    pub answers: Vec<GroupAnswer>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub basis_refs: Vec<String>,
    #[serde(default)]
    pub scope_note: Option<String>,
}

impl PmAnswerGroup {
    /// Answers the group in one transaction and returns its PM receipt and whether it replayed.
    pub fn apply(self, ctx: &Ctx, actor: Actor<'_>) -> Result<(RequestReceipt, bool)> {
        let shared = AnswerInput {
            text: self.text,
            option: None,
            basis_refs: self.basis_refs,
            scope_note: self.scope_note,
        };
        let group = super::answer_group(ctx, &self.key, actor, &shared, &self.answers)?;
        let receipt = group
            .receipt
            .context("a PM group answer committed without a receipt")?;
        Ok((receipt, group.replayed))
    }
}
