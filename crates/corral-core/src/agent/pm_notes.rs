//! Revisioned PM context, never an authorization source.
use crate::{Store, records::RecordArea};
use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub const MAX_LIVE: usize = 64;
pub const MAX_TEXT_BYTES: usize = 1024;
pub const MAX_OPTION_BYTES: usize = 256;
pub const MAX_SUPERSEDED: usize = 32;
pub const MAX_RECORD_BYTES: usize = 32 * 1024;
const AREA: RecordArea<PmNotes> = RecordArea::new("pm-notes");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    Instruction,
    Deferral,
    Hypothesis,
    Commitment,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteAuthor {
    Owner,
    Pm,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub kind: NoteKind,
    pub text: String,
    #[serde(default)]
    pub provenance: Option<String>,
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub resume_when: Option<String>,
    pub author: NoteAuthor,
    pub created_at: Timestamp,
    #[serde(default)]
    pub superseded_by: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PmNotes {
    pub version: u32,
    pub revision: u64,
    pub next_id: u64,
    pub entries: Vec<Note>,
}
impl Default for PmNotes {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            next_id: 1,
            entries: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum NoteMutation {
    Add {
        kind: NoteKind,
        text: String,
        provenance: Option<String>,
        reason: Option<String>,
        resume_when: Option<String>,
    },
    Supersede {
        id: String,
        text: String,
        provenance: Option<String>,
        reason: Option<String>,
        resume_when: Option<String>,
    },
    Remove {
        id: String,
    },
}
#[derive(Debug, PartialEq, Eq)]
pub enum NotesError {
    Conflict { expected: u64, actual: u64 },
    OwnerEntry,
    Invalid(String),
}
impl std::fmt::Display for NotesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { expected, actual } => write!(
                f,
                "notes revision conflict: expected {expected}, actual {actual}"
            ),
            Self::OwnerEntry => f.write_str("PM cannot change an owner note"),
            Self::Invalid(reason) => f.write_str(reason),
        }
    }
}
impl std::error::Error for NotesError {}
fn invalid(reason: &str) -> anyhow::Error {
    NotesError::Invalid(reason.into()).into()
}
pub fn load(store: &Store, scope: &str) -> Result<PmNotes> {
    let notes = AREA.load(store, scope)?.unwrap_or_default();
    anyhow::ensure!(notes.version == 1, "unsupported PM notes version");
    Ok(notes)
}
pub fn apply(
    store: &Store,
    scope: &str,
    expected_revision: Option<u64>,
    author: NoteAuthor,
    mutation: NoteMutation,
) -> Result<PmNotes> {
    let _lock = store.lock("pm-notes")?;
    store.records_transaction(|| {
        let mut notes = load(store, scope)?;
        if let Some(expected) = expected_revision
            && expected != notes.revision
        {
            return Err(NotesError::Conflict {
                expected,
                actual: notes.revision,
            }
            .into());
        }
        let added = match mutation {
            NoteMutation::Add {
                kind,
                text,
                provenance,
                reason,
                resume_when,
            } => Some((kind, text, provenance, reason, resume_when, None)),
            NoteMutation::Supersede {
                id,
                text,
                provenance,
                reason,
                resume_when,
            } => {
                let old = notes
                    .entries
                    .iter_mut()
                    .find(|n| n.id == id)
                    .ok_or_else(|| invalid("unknown note"))?;
                if author == NoteAuthor::Pm && old.author == NoteAuthor::Owner {
                    return Err(NotesError::OwnerEntry.into());
                }
                if old.superseded_by.is_some() {
                    return Err(invalid("note is already superseded"));
                }
                old.superseded_by = Some(format!("n-{}", notes.next_id));
                Some((old.kind, text, provenance, reason, resume_when, Some(id)))
            }
            NoteMutation::Remove { id } => {
                let index = notes
                    .entries
                    .iter()
                    .position(|n| n.id == id)
                    .ok_or_else(|| invalid("unknown note"))?;
                if author == NoteAuthor::Pm && notes.entries[index].author == NoteAuthor::Owner {
                    return Err(NotesError::OwnerEntry.into());
                }
                notes.entries.remove(index);
                None
            }
        };
        if let Some((kind, text, provenance, reason, resume_when, supersedes)) = added {
            if text.len() > MAX_TEXT_BYTES {
                return Err(invalid("note text exceeds byte bound"));
            }
            for field in [&provenance, &reason, &resume_when, &supersedes] {
                if field.as_ref().is_some_and(|s| s.len() > MAX_OPTION_BYTES) {
                    return Err(invalid("note field exceeds byte bound"));
                }
            }
            let present =
                |value: &Option<String>| value.as_ref().is_some_and(|s| !s.trim().is_empty());
            if kind == NoteKind::Instruction && author == NoteAuthor::Pm && !present(&provenance) {
                return Err(invalid("PM instruction requires provenance"));
            }
            if kind == NoteKind::Deferral && (!present(&reason) || !present(&resume_when)) {
                return Err(invalid("deferral requires reason and resume_when"));
            }
            notes.entries.push(Note {
                id: format!("n-{}", notes.next_id),
                kind,
                text,
                provenance,
                supersedes,
                reason,
                resume_when,
                author,
                created_at: Timestamp::now(),
                superseded_by: None,
            });
            notes.next_id = notes.next_id.checked_add(1).context("note id overflow")?;
        }
        if notes
            .entries
            .iter()
            .filter(|n| n.superseded_by.is_none())
            .count()
            > MAX_LIVE
        {
            return Err(invalid("too many live notes"));
        }
        let mut excess = notes
            .entries
            .iter()
            .filter(|n| n.superseded_by.is_some())
            .count()
            .saturating_sub(MAX_SUPERSEDED);
        notes.entries.retain(|n| {
            if excess > 0 && n.superseded_by.is_some() {
                excess -= 1;
                false
            } else {
                true
            }
        });
        notes.revision = notes
            .revision
            .checked_add(1)
            .context("notes revision overflow")?;
        if serde_json::to_vec(&notes)?.len() > MAX_RECORD_BYTES {
            return Err(invalid("notes record exceeds byte bound"));
        }
        AREA.save(store, scope, &notes)?;
        Ok(notes)
    })
}
#[cfg(test)]
#[path = "pm_notes_tests.rs"]
mod tests;
