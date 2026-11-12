//! Caller-attributed PM context notes.
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_core::agent::pm_notes::{self, NoteAuthor, NoteKind, NoteMutation, PmNotes};
use std::path::Path;

#[derive(Args)]
pub struct Fields {
    #[arg(long)]
    text: String,
    #[arg(long)]
    provenance: Option<String>,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    resume_when: Option<String>,
    #[arg(long)]
    expect_revision: Option<u64>,
    #[arg(long)]
    json: bool,
}
#[derive(Subcommand)]
pub enum PmNotesCmd {
    /// Read the complete bounded record
    List {
        #[arg(long)]
        json: bool,
    },
    /// Add caller-attributed context (not an authorization)
    Add {
        #[arg(long)]
        kind: String,
        #[command(flatten)]
        fields: Fields,
    },
    /// Replace a note, retaining its supersession link
    Supersede {
        id: String,
        #[command(flatten)]
        fields: Fields,
    },
    /// Remove a note
    Remove {
        id: String,
        #[arg(long)]
        expect_revision: Option<u64>,
        #[arg(long)]
        json: bool,
    },
}

fn mutate(
    ctx: &corral_ops::Ctx,
    entry: &crate::entry::Entry,
    expected_revision: Option<u64>,
    mutation: NoteMutation,
) -> Result<PmNotes> {
    // Match owner parks: environment markers and run cwd are attribution, not
    // authenticated identity. A caller that clears its markers can evade this guard.
    corral_ops::attention::guard(ctx)?;
    let author = match entry.decision_author(&ctx.store)? {
        "owner" => NoteAuthor::Owner,
        "pm" | "agent" | "assistant" | "reviewer" => NoteAuthor::Pm,
        _ => anyhow::bail!("unidentified caller: PM notes mutation refused"),
    };
    pm_notes::apply(&ctx.store, "workspace", expected_revision, author, mutation)
}

pub fn run(start: &Path, cmd: PmNotesCmd, entry: &crate::entry::Entry) -> Result<()> {
    let ctx = entry.open(start)?;
    let (notes, json) = match cmd {
        PmNotesCmd::List { json } => (pm_notes::load(&ctx.store, "workspace")?, json),
        PmNotesCmd::Add { kind, fields } => {
            let kind: NoteKind = serde_json::from_value(serde_json::Value::String(kind))
                .context("kind must be instruction, deferral, hypothesis or commitment")?;
            let mutation = NoteMutation::Add {
                kind,
                text: fields.text,
                provenance: fields.provenance,
                reason: fields.reason,
                resume_when: fields.resume_when,
            };
            (
                mutate(&ctx, entry, fields.expect_revision, mutation)?,
                fields.json,
            )
        }
        PmNotesCmd::Supersede { id, fields } => {
            let mutation = NoteMutation::Supersede {
                id,
                text: fields.text,
                provenance: fields.provenance,
                reason: fields.reason,
                resume_when: fields.resume_when,
            };
            (
                mutate(&ctx, entry, fields.expect_revision, mutation)?,
                fields.json,
            )
        }
        PmNotesCmd::Remove {
            id,
            expect_revision,
            json,
        } => (
            mutate(&ctx, entry, expect_revision, NoteMutation::Remove { id })?,
            json,
        ),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&notes)?);
    } else {
        println!("PM notes revision {}", notes.revision);
        for note in notes.entries {
            println!(
                "{} {:?}: {}{}",
                note.id,
                note.kind,
                note.text,
                note.superseded_by
                    .map(|id| format!(" (superseded by {id})"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}
