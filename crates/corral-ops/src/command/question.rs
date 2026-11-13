//! `question` command executors.
use anyhow::{Context, Result};
use corral_core::agent::QuestionKind;
use corral_protocol::cmd::question::QuestionCommand;

use super::{Exec, Output, Verb};
use crate::questions::{self, Actor, AnswerInput, Applied, GroupAnswer};

fn actor(owner: bool) -> Actor<'static> {
    if owner { Actor::Owner } else { Actor::Pm(None) }
}

fn print_applied(out: &mut Output, applied: &Applied, json: bool) -> Result<()> {
    let Some(outcome) = applied.outcome() else {
        anyhow::bail!("the CLI never replays a host request");
    };
    print_outcome(out, outcome, json)
}

fn print_outcome(out: &mut Output, outcome: &questions::Outcome, json: bool) -> Result<()> {
    if json {
        out.json_pretty(outcome)?;
    } else {
        let message = outcome
            .message_id
            .as_deref()
            .map(|id| format!("; message {id}"))
            .unwrap_or_default();
        out.line(format_args!(
            "{} {:?} (revision {}){message}; delivery {}",
            outcome.key, outcome.state, outcome.revision, outcome.delivery
        ));
    }
    Ok(())
}

impl Verb for QuestionCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        let ctx = &cx.ctx;
        match self {
            Self::Answer {
                key,
                rev,
                text,
                option,
                basis,
                scope_note,
                owner,
                json,
            } => {
                let input = AnswerInput {
                    text,
                    option,
                    basis_refs: basis,
                    scope_note,
                };
                print_applied(
                    &mut cx.out,
                    &questions::answer(ctx, &key, rev, actor(owner), input)?,
                    json,
                )
            }
            Self::AnswerGroup {
                key,
                answers,
                text,
                basis,
                scope_note,
                owner,
                json,
            } => {
                let answers: Vec<GroupAnswer> = match answers {
                    Some(raw) => serde_json::from_str(&raw)
                        .context("--answers is not a JSON array of {key, rev, text?, option?}")?,
                    None => questions::group(ctx, &key)?
                        .into_iter()
                        .filter(|view| view.pending)
                        .map(|view| GroupAnswer {
                            key: view.question.key,
                            rev: view.question.revision,
                            text: None,
                            option: None,
                        })
                        .collect(),
                };
                let shared = AnswerInput {
                    text,
                    option: None,
                    basis_refs: basis,
                    scope_note,
                };
                let applied = questions::answer_group(ctx, &key, actor(owner), &shared, &answers)?;
                for outcome in &applied.outcomes {
                    print_outcome(&mut cx.out, outcome, json)?;
                }
                Ok(())
            }
            Self::Escalate {
                key,
                rev,
                note,
                kind,
                json,
            } => {
                let kind = kind.map(|kind| match kind.as_str() {
                    "clarification" => Ok(QuestionKind::Clarification),
                    "technical_choice" => Ok(QuestionKind::TechnicalChoice),
                    "permission" => Ok(QuestionKind::Permission),
                    "owner_only" => Ok(QuestionKind::OwnerOnly),
                    _ => anyhow::bail!("invalid value {kind:?} for '--kind <KIND>'\n  [possible values: clarification, technical-choice, permission, owner-only]"),
                }).transpose()?;
                print_applied(
                    &mut cx.out,
                    &questions::escalate(ctx, &key, rev, actor(false), note, kind)?,
                    json,
                )
            }
            Self::Cancel {
                key,
                rev,
                reason,
                owner,
                json,
            } => print_applied(
                &mut cx.out,
                &questions::cancel(ctx, &key, rev, actor(owner), &reason)?,
                json,
            ),
            Self::Deadline {
                key,
                rev,
                at,
                instruction,
                json,
            } => {
                let at: jiff::Timestamp = at
                    .parse()
                    .with_context(|| format!("--at {at:?} is not an RFC 3339 timestamp"))?;
                print_applied(
                    &mut cx.out,
                    &questions::deadline(ctx, &key, rev, at, &instruction)?,
                    json,
                )
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }
}
