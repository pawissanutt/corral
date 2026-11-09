//! `--status` (changes nothing) and the directory-state invariant of plan section 12.

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use super::durable::{Seen, seen};
use super::journal::{Candidate, Journal, JournalState, RollbackStep, read_fence};
use super::{DB, DISCARD, MIGRATING, Migrate, locks};
use crate::sqlite::migrate::read_meta;

/// What `lstat` sees at `C/db`, `C/db.migrating` and `C/db.discard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DirState {
    pub db: Seen,
    pub migrating: Seen,
    pub discard: Seen,
}

impl DirState {
    pub fn observe(corral: &Path) -> Result<DirState> {
        Ok(DirState {
            db: seen(&corral.join(DB))?,
            migrating: seen(&corral.join(MIGRATING))?,
            discard: seen(&corral.join(DISCARD))?,
        })
    }

    pub fn describe(&self) -> String {
        format!(
            "db: {:?}, db.migrating: {:?}, db.discard: {:?}",
            self.db, self.migrating, self.discard
        )
    }

    fn symlink(&self) -> bool {
        [self.db, self.migrating, self.discard]
            .iter()
            .any(|s| !matches!(s, Seen::Absent | Seen::Dir))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    pub fence: Option<String>,
    pub dirs: DirState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    /// v2 changes since publication (`published` only, when the gate could be shared).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<u64>,
    pub next: Vec<String>,
    pub problems: Vec<String>,
}

/// Violations of the directory-state invariant for `journal` (none when it holds).
pub fn directory_problems(corral: &Path, journal: Option<&Journal>) -> Result<Vec<String>> {
    let dirs = DirState::observe(corral)?;
    let mut problems = Vec::new();
    if dirs.symlink() {
        problems.push(format!(
            "a symlink or non-directory is at a database path ({})",
            dirs.describe()
        ));
        return Ok(problems);
    }
    let (d, m, q) = (
        dirs.db.present(),
        dirs.migrating.present(),
        dirs.discard.present(),
    );
    let require = |ok: bool, what: &str, problems: &mut Vec<String>| {
        if !ok {
            problems.push(format!("{what} ({})", dirs.describe()));
        }
    };
    match journal {
        None => require(
            !m && !q,
            "no journal, but db.migrating or db.discard exists",
            &mut problems,
        ),
        Some(j) => match j.state {
            JournalState::Fenced => require(
                !d && !m && !q,
                "fenced requires no database directory",
                &mut problems,
            ),
            JournalState::Importing => require(
                !d && !q,
                "importing allows only db.migrating",
                &mut problems,
            ),
            JournalState::Verified => require(
                d != m && !q,
                "verified requires exactly one of db and db.migrating",
                &mut problems,
            ),
            JournalState::Published | JournalState::Finalizing | JournalState::Finalized => {
                require(
                    d && !m && !q,
                    "a published store requires only db",
                    &mut problems,
                );
            }
            JournalState::RollingBack => {
                let record = j.rollback()?;
                let step = j.step.unwrap_or(RollbackStep::Intent);
                if record.from == JournalState::Published {
                    let archived = record
                        .archive
                        .as_ref()
                        .is_some_and(|a| corral.join(a).join(DB).is_dir());
                    if step == RollbackStep::Intent {
                        require(
                            d != archived && !m && !q,
                            "rollback intent requires db xor the archived db",
                            &mut problems,
                        );
                    } else {
                        require(
                            !d && archived && !m && !q,
                            "an archived rollback requires only <archive>/db",
                            &mut problems,
                        );
                    }
                } else {
                    let candidate = record.candidate.unwrap_or(Candidate::None);
                    match step {
                        RollbackStep::Intent => {
                            let (at, other) = match candidate {
                                Candidate::Db => (d, m),
                                Candidate::Migrating => (m, d),
                                Candidate::None => (false, d || m),
                            };
                            let ok = !other
                                && if candidate == Candidate::None {
                                    !q
                                } else {
                                    at != q
                                };
                            require(
                                ok,
                                "rollback intent requires the candidate xor db.discard",
                                &mut problems,
                            );
                        }
                        RollbackStep::Quarantined => require(
                            !d && !m,
                            "a quarantined rollback allows only db.discard",
                            &mut problems,
                        ),
                        RollbackStep::Cleaned
                        | RollbackStep::FenceRestored
                        | RollbackStep::Archived => {
                            require(
                                !d && !m && !q,
                                "a cleaned rollback requires no database directory",
                                &mut problems,
                            );
                        }
                    }
                }
            }
        },
    }
    Ok(problems)
}

fn next_commands(
    journal: Option<&Journal>,
    fence: Option<&str>,
    changes: Option<u64>,
) -> Vec<String> {
    let cmd = |s: &str| format!("corral migrate {s}");
    match journal.map(|j| j.state) {
        None if crate::store::db::supported_fence(super::journal::fence_value(fence)) => Vec::new(),
        None => vec![cmd("--dry-run"), cmd("--apply")],
        Some(JournalState::Fenced | JournalState::Importing | JournalState::Verified) => {
            vec![cmd("--apply"), cmd("--rollback")]
        }
        Some(JournalState::Published) => {
            let rollback = match changes {
                Some(0) => cmd("--rollback"),
                Some(n) => cmd(&format!("--rollback --discard-v2-changes {n}")),
                None => cmd("--status (after maintenance) for the rollback command"),
            };
            vec![rollback, cmd("--finalize --yes")]
        }
        Some(JournalState::RollingBack) => vec![cmd("--rollback")],
        Some(JournalState::Finalizing) => vec![cmd("--finalize --yes")],
        Some(JournalState::Finalized) => Vec::new(),
    }
}

pub(super) fn status(m: &Migrate) -> Result<Status> {
    let corral = m.corral_dir();
    let journal = Journal::read(corral)?;
    let fence = read_fence(corral)?;
    let dirs = DirState::observe(corral)?;
    let mut status = Status {
        state: match &journal {
            Some(j) => j.state.as_str().to_owned(),
            None if crate::store::db::supported_fence(super::journal::fence_value(
                fence.as_deref(),
            )) =>
            {
                "v2".to_owned()
            }
            None => "legacy".to_owned(),
        },
        step: journal
            .as_ref()
            .and_then(|j| j.step)
            .map(|s| s.as_str().to_owned()),
        fence: fence.clone(),
        dirs,
        candidate: None,
        archive: None,
        changes: None,
        next: Vec::new(),
        problems: directory_problems(corral, journal.as_ref())?,
    };
    if let Some(j) = &journal {
        match j.state {
            JournalState::Verified => {
                status.candidate = match (dirs.db, dirs.migrating) {
                    (Seen::Dir, Seen::Absent) => Some(DB.to_owned()),
                    (Seen::Absent, Seen::Dir) => Some(MIGRATING.to_owned()),
                    _ => None,
                };
            }
            JournalState::Importing if dirs.migrating == Seen::Dir => {
                status.candidate = Some(MIGRATING.to_owned());
            }
            JournalState::RollingBack => {
                let record = j.rollback()?;
                status.archive.clone_from(&record.archive);
                status.candidate = record.candidate.map(|c| c.as_str().to_owned());
            }
            JournalState::Published => {
                if let Some(_gate) = locks::store_gate_shared(corral)? {
                    let meta = read_meta(&crate::sqlite::SqliteBackend::db_path(corral))?;
                    let at = j.change_seq_at_publish.unwrap_or(meta.change_seq);
                    status.changes = Some(meta.change_seq.saturating_sub(at));
                } else {
                    status.problems.push(
                        "maintenance holds the store gate; the change count is unknown".into(),
                    );
                }
            }
            _ => {}
        }
    }
    status.next = next_commands(journal.as_ref(), fence.as_deref(), status.changes);
    Ok(status)
}
