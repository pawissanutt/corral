//! Owner-authored PM context notes.
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_core::agent::pm_notes::{self, NoteAuthor, NoteKind, NoteMutation};
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
    /// Add owner context (not an authorization)
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
                pm_notes::apply(
                    &ctx.store,
                    "workspace",
                    fields.expect_revision,
                    NoteAuthor::Owner,
                    mutation,
                )?,
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
                pm_notes::apply(
                    &ctx.store,
                    "workspace",
                    fields.expect_revision,
                    NoteAuthor::Owner,
                    mutation,
                )?,
                fields.json,
            )
        }
        PmNotesCmd::Remove {
            id,
            expect_revision,
            json,
        } => (
            pm_notes::apply(
                &ctx.store,
                "workspace",
                expect_revision,
                NoteAuthor::Owner,
                NoteMutation::Remove { id },
            )?,
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
