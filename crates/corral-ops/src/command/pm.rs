//! `pm` command executors.
use anyhow::{Context, Result};
use corral_core::agent::pm_notes::{self, NoteAuthor, NoteKind, NoteMutation, PmNotes};
use corral_core::pm_inbox::{
    self, ItemKind, NewPmItem, PmInboxItem, RequestedAction, Resolution, name,
};
use corral_protocol::cmd::pm::PmCommand;

use super::{Exec, Verb};

impl Verb for PmCommand {
    fn touches_runtime(&self) -> bool {
        matches!(self, Self::InboxResolve { .. })
    }

    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::GuardResume { json } => {
                let resumed = crate::pm_guard_resume(&cx.ctx)?;
                if json {
                    cx.out.json(&resumed)?;
                } else if resumed {
                    cx.out.line(format_args!("PM usage guard resumed"));
                } else {
                    cx.out.line(format_args!("PM usage guard was not paused"));
                }
            }
            Self::Present {
                decision,
                delivery,
                confirmed,
            } => {
                crate::pm_present(
                    &cx.ctx,
                    &decision,
                    &delivery,
                    confirmed,
                    jiff::Timestamp::now(),
                )?;
                cx.out.line(format_args!(
                    "Presentation recorded; owner decision remains pending."
                ));
            }
            Self::Resolve { decision, answer } => {
                crate::pm_resolve(&cx.ctx, &decision, &answer)?;
                cx.out.line(format_args!(
                    "Owner answer recorded; task restrictions remain enforced."
                ));
            }
            Self::InboxPost {
                kind,
                subject,
                body,
                refs,
                action,
                quote,
                input,
                key,
                json,
            } => {
                let (item, created) = cx.ctx.store.pm_inbox_post(NewPmItem {
                    from: cx.invoker.inbox_author(&cx.ctx.store)?,
                    kind: pm_inbox::parse::<ItemKind>(
                        "--kind",
                        &kind,
                        "action_request, question, handoff, routing",
                    )?,
                    subject,
                    body,
                    refs,
                    requested_action: action.as_deref().map(RequestedAction::parse).transpose()?,
                    owner_words: quote.map(|quote| pm_inbox::owner_words(quote, input)),
                    key,
                })?;
                if json {
                    print_item(cx, &item, true, "")?;
                } else if created {
                    cx.out.line(format_args!(
                        "{} posted (revision 0); follow it with `corral pm inbox show {}`",
                        item.id, item.id
                    ));
                } else {
                    cx.out.line(format_args!(
                        "{} already posted with this key ({}, revision {})",
                        item.id,
                        name(&item.state),
                        item.revision
                    ));
                }
            }
            Self::InboxClaim {
                id,
                expected_revision,
                json,
            } => {
                let item = cx.ctx.store.pm_inbox_claim(&id, expected_revision, None)?;
                print_item(cx, &item, json, "claimed")?;
            }
            Self::InboxResolve {
                id,
                expected_revision,
                outcome,
                text,
                refs,
                json,
            } => {
                let resolution = pm_inbox::parse::<Resolution>(
                    "--outcome",
                    &outcome,
                    "done, declined, escalated_to_owner",
                )?;
                let item = crate::pm_inbox::resolve(
                    &cx.ctx,
                    &id,
                    expected_revision,
                    (resolution, &text, refs),
                    None,
                )?;
                print_item(cx, &item, json, &format!("resolved {}", name(&resolution)))?;
            }
            Self::InboxWithdraw {
                id,
                expected_revision,
                reason,
                json,
            } => {
                let by = cx.invoker.inbox_author(&cx.ctx.store)?;
                let item = cx
                    .ctx
                    .store
                    .pm_inbox_withdraw(&id, expected_revision, &by, &reason)?;
                print_item(cx, &item, json, "withdrawn")?;
            }
            Self::NotesAdd {
                kind,
                text,
                provenance,
                reason,
                resume_when,
                expect_revision,
                json,
            } => {
                let kind: NoteKind = serde_json::from_value(serde_json::Value::String(kind))
                    .context("kind must be instruction, deferral, hypothesis or commitment")?;
                let notes = mutate(
                    cx,
                    expect_revision,
                    NoteMutation::Add {
                        kind,
                        text,
                        provenance,
                        reason,
                        resume_when,
                    },
                )?;
                print_notes(cx, notes, json)?;
            }
            Self::NotesSupersede {
                id,
                text,
                provenance,
                reason,
                resume_when,
                expect_revision,
                json,
            } => {
                let notes = mutate(
                    cx,
                    expect_revision,
                    NoteMutation::Supersede {
                        id,
                        text,
                        provenance,
                        reason,
                        resume_when,
                    },
                )?;
                print_notes(cx, notes, json)?;
            }
            Self::NotesRemove {
                id,
                expect_revision,
                json,
            } => {
                let notes = mutate(cx, expect_revision, NoteMutation::Remove { id })?;
                print_notes(cx, notes, json)?;
            }
            Self::Other => return Err(super::unknown_command("other")),
        }
        Ok(())
    }
}

fn print_item(cx: &mut Exec<'_>, item: &PmInboxItem, json: bool, verb: &str) -> Result<()> {
    if json {
        let mut value = serde_json::to_value(item)?;
        value["body"] = item.body.clone().into();
        cx.out.json_pretty(&value)?;
    } else {
        cx.out.line(format_args!(
            "{} {verb} (revision {})",
            item.id, item.revision
        ));
    }
    Ok(())
}

fn mutate(
    cx: &Exec<'_>,
    expected_revision: Option<u64>,
    mutation: NoteMutation,
) -> Result<PmNotes> {
    // Markers and run cwd are attribution, not authenticated identity.
    crate::attention::guard(&cx.ctx, cx.invoker)?;
    let author = match cx.invoker.decision_author(&cx.ctx.store)? {
        "owner" => NoteAuthor::Owner,
        "pm" | "agent" | "assistant" | "reviewer" => NoteAuthor::Pm,
        _ => anyhow::bail!("unidentified caller: PM notes mutation refused"),
    };
    pm_notes::apply(
        &cx.ctx.store,
        "workspace",
        expected_revision,
        author,
        mutation,
    )
}

fn print_notes(cx: &mut Exec<'_>, notes: PmNotes, json: bool) -> Result<()> {
    if json {
        cx.out.json_pretty(&notes)?;
    } else {
        cx.out
            .line(format_args!("PM notes revision {}", notes.revision));
        for note in notes.entries {
            cx.out.line(format_args!(
                "{} {:?}: {}{}",
                note.id,
                note.kind,
                note.text,
                note.superseded_by
                    .map(|id| format!(" (superseded by {id})"))
                    .unwrap_or_default()
            ));
        }
    }
    Ok(())
}
