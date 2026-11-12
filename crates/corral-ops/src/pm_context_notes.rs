//! Launch-only durable context projection; never an authorization source.
use anyhow::{Context, Result, ensure};
use corral_core::{
    Store,
    agent::pm_notes::{self, Note, NoteAuthor, NoteKind, PmNotes},
};
use serde_json::{Value, json};

const SNAPSHOT_MAX_BYTES: usize = 64 * 1024;
const NOTES_MAX_BYTES: usize = 24 * 1024;

pub(super) fn snapshot(store: &Store, mut context: Value) -> Result<Vec<u8>> {
    context["cache_key"] = serde_json::to_value(store.config_layers()?.config()?.pm.cache_key)?;
    context["notes"] = Value::Null;
    let base_len = serde_json::to_vec(&context)?.len();
    ensure!(
        base_len <= SNAPSHOT_MAX_BYTES,
        "Pi PM operating context exceeds 64 KiB"
    );
    match pm_notes::load(store, "workspace")
        .context("loading workspace PM notes for launch context")
    {
        Ok(notes) => {
            // Replacing `null` adds only the block's own bytes minus four.
            let budget = NOTES_MAX_BYTES.min(SNAPSHOT_MAX_BYTES - base_len + 4);
            context["notes"] = project(&notes, budget)?;
        }
        Err(error) => eprintln!("corral: {error:#}; launching without PM notes"),
    }
    Ok(serde_json::to_vec(&context)?)
}

fn priority(note: &Note) -> u8 {
    match (note.kind, note.author) {
        (NoteKind::Instruction, NoteAuthor::Owner) => 0,
        (NoteKind::Instruction, NoteAuthor::Pm) => 1,
        (NoteKind::Deferral, _) => 2,
        (NoteKind::Commitment, _) => 3,
        (NoteKind::Hypothesis, _) => 4,
    }
}

fn project(notes: &PmNotes, budget: usize) -> Result<Value> {
    let mut live: Vec<_> = notes
        .entries
        .iter()
        .filter(|note| note.superseded_by.is_none())
        .collect();
    live.sort_by(|a, b| {
        priority(a)
            .cmp(&priority(b))
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| b.id.cmp(&a.id))
    });
    let mut block = json!({"revision": notes.revision, "entries": [], "omitted": live.len()});
    let mut length = serde_json::to_vec(&block)?.len();
    if length > budget {
        return Ok(Value::Null);
    }
    let total = live.len();
    let mut entries = Vec::new();
    for note in live {
        let entry = json!({
            "id": note.id, "kind": note.kind, "text": note.text,
            "provenance": note.provenance, "reason": note.reason,
            "resume_when": note.resume_when, "author": note.author,
            "created_at": note.created_at,
        });
        let added = serde_json::to_vec(&entry)?.len() + usize::from(!entries.is_empty());
        if length + added > budget {
            break;
        }
        length += added;
        entries.push(entry);
    }
    // The initial omitted count has at least as many digits as the final one.
    block["omitted"] = json!(total - entries.len());
    block["entries"] = Value::Array(entries);
    Ok(block)
}

#[cfg(test)]
#[path = "pm_context_notes_tests.rs"]
mod tests;
