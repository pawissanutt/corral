//! Caller-attributed PM context notes.
use anyhow::Result;
use clap::{Args, Subcommand};
use corral_core::agent::pm_notes;
use corral_protocol::cmd::{Command, pm::PmCommand};

use crate::invocation::Invocation;

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

pub fn run(inv: &Invocation, cmd: PmNotesCmd) -> Result<()> {
    let (notes, json) = match cmd {
        PmNotesCmd::List { json } => {
            let ctx = inv.ctx()?;
            (pm_notes::load(&ctx.store, "workspace")?, json)
        }
        PmNotesCmd::Add { kind, fields } => {
            return inv.submit(Command::Pm(PmCommand::NotesAdd {
                kind,
                text: fields.text,
                provenance: fields.provenance,
                reason: fields.reason,
                resume_when: fields.resume_when,
                expect_revision: fields.expect_revision,
                json: fields.json,
            }));
        }
        PmNotesCmd::Supersede { id, fields } => {
            return inv.submit(Command::Pm(PmCommand::NotesSupersede {
                id,
                text: fields.text,
                provenance: fields.provenance,
                reason: fields.reason,
                resume_when: fields.resume_when,
                expect_revision: fields.expect_revision,
                json: fields.json,
            }));
        }
        PmNotesCmd::Remove {
            id,
            expect_revision,
            json,
        } => {
            return inv.submit(Command::Pm(PmCommand::NotesRemove {
                id,
                expect_revision,
                json,
            }));
        }
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
