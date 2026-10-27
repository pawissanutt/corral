//! Store entry points for scratch directories below the validated scratch root (plan T-253
//! section 8): attempt directories, long-lived spools, the reads the reclaim pass needs and the
//! once-per-attempt usage warning. Every read here is limited to registrations in the named
//! states, so none of them grows with directories already reclaimed.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use jiff::Timestamp;

use super::{NewScratch, Reclaimed, ScratchArea};
use crate::Store;
use crate::backend::{Operation, ScratchKind, ScratchOp, ScratchRow, ScratchState};

#[cfg(test)]
#[path = "store_api_tests.rs"]
mod tests;

/// A new attempt directory: registration `id` at `rel` below the scratch root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptScratch<'a> {
    pub kind: ScratchKind,
    pub id: &'a str,
    pub task: &'a str,
    /// The container the attempt belongs to (run, plan or aggregate attempt).
    pub subject: Option<&'a str>,
    pub attempt: &'a str,
    pub rel: &'a str,
}

impl Store {
    fn scratch_area(&self) -> Result<ScratchArea<'_>> {
        let layout = self.storage_layout()?;
        ScratchArea::open(self.backend(), &layout)
    }

    /// `<scratch_root>/<rel>`: a path only; nothing is read from the store or created.
    pub fn scratch_path(&self, rel: &str) -> Result<PathBuf> {
        Ok(self.storage_layout()?.scratch_root().join(rel))
    }

    /// The directory of spool `name` (`<scratch_root>/<name>`): one registration that stays open
    /// for the short-lived files of many attempts. It is registered on first use and recreated
    /// when it was removed.
    pub fn scratch_spool(&self, kind: ScratchKind, name: &str) -> Result<PathBuf> {
        let area = self.scratch_area()?;
        let id = format!("spool-{name}");
        if area.open_row(&id)?.is_none() {
            let new = NewScratch {
                id: id.clone(),
                kind,
                task: None,
                subject: None,
                attempt: None,
                path: name.to_owned(),
            };
            // A concurrent first use may have registered it in between.
            if let Err(error) = area.register(new, Timestamp::now())
                && area.open_row(&id)?.is_none()
            {
                return Err(error);
            }
        }
        let dir = area.root().join(name);
        crate::fs_guard::create_dir_all_nofollow(&dir)?;
        Ok(dir)
    }

    /// `<scratch_root>/tmp`, the spool of short-lived staging files (it replaces the legacy
    /// `.corral` tmp directory).
    pub fn scratch_tmp(&self) -> Result<PathBuf> {
        self.scratch_spool(ScratchKind::TmpStaging, "tmp")
    }

    /// The directory of one attempt: returned as is while its registration is open (a resumed
    /// launch), otherwise newly registered and created. A closed registration is never reopened.
    pub fn attempt_scratch(&self, new: &AttemptScratch<'_>) -> Result<PathBuf> {
        let area = self.scratch_area()?;
        let dir = area.root().join(new.rel);
        if area.open_row(new.id)?.is_some() {
            crate::fs_guard::create_dir_all_nofollow(&dir)?;
            return Ok(dir);
        }
        area.register(
            NewScratch {
                id: new.id.to_owned(),
                kind: new.kind,
                task: Some(new.task.to_owned()),
                subject: new.subject.map(str::to_owned),
                attempt: Some(new.attempt.to_owned()),
                path: new.rel.to_owned(),
            },
            Timestamp::now(),
        )
    }

    /// Closes registration `id` when it is open; `false` when it is not open (already closed,
    /// reclaimed or never registered).
    pub fn close_open_scratch(&self, id: &str) -> Result<bool> {
        let area = self.scratch_area()?;
        if area.open_row(id)?.is_none() {
            return Ok(false);
        }
        area.close(id, Timestamp::now())?;
        Ok(true)
    }

    /// The registrations in `states`; never every registration ever made.
    pub fn scratch_rows_in(&self, states: &[ScratchState]) -> Result<Vec<ScratchRow>> {
        anyhow::ensure!(!states.is_empty(), "name the scratch states to read");
        self.scratch_area()?.rows_in(states)
    }

    /// Records attempt `id`'s one usage warning at `bytes` (one `storage_scratch_warning`
    /// event): `true` when this call recorded it, `false` when it was already warned.
    pub fn warn_scratch(&self, id: &str, bytes: u64) -> Result<bool> {
        let warn = Operation::Scratch(ScratchOp::Warn {
            id: id.into(),
            evidence_bytes: bytes,
            at: Timestamp::now(),
        });
        match self.backend().apply(warn) {
            Ok(_) => Ok(true),
            Err(error) => {
                let warned = self
                    .scratch_area()?
                    .rows_in(&[ScratchState::Open])?
                    .into_iter()
                    .find(|row| row.id == id)
                    .is_some_and(|row| row.warned.is_some());
                if warned {
                    Ok(false)
                } else {
                    Err(anyhow!("warn scratch {id}: {error}"))
                }
            }
        }
    }

    /// Records closed or deferred registration `id` (now in `from`) as `deferred` until
    /// `next_check_at`, because `reason` still uses its directory.
    pub fn defer_scratch(
        &self,
        id: &str,
        from: ScratchState,
        reason: &str,
        next_check_at: Timestamp,
    ) -> Result<()> {
        self.backend()
            .apply(Operation::Scratch(ScratchOp::Defer {
                id: id.into(),
                from,
                reason: reason.into(),
                next_check_at,
            }))
            .map_err(|error| anyhow!("defer scratch {id}: {error}"))?;
        Ok(())
    }

    /// Reclaims registration `id` as `owner`, or completes the reclaim `owner` began and never
    /// recorded (a crash between deleting and recording): exactly one `storage_reclaimed` or
    /// `storage_reclaim_failed` event results, whether or not the directory was already gone.
    /// The caller has established that nothing uses the directory.
    pub fn reclaim_scratch_entry(&self, id: &str, owner: &str) -> Result<Reclaimed> {
        self.scratch_area()?.reclaim(id, owner, Timestamp::now())
    }
}
