//! The `storage-gc` claim and proof that its holder died (plan T-253 sections 1 and 7).
//!
//! The claim document records the holder's [`ProcessIdentity`]. Another process takes the claim
//! over only after [`proven_dead`] shows that no live process has that pid with that start time
//! in this boot; an unreadable process table or an unrecognized claim document is no proof.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use super::all_pages;
use crate::backend::{
    ApplyError, Claim, ClaimRow, Clock, Operation, ReleaseClaim, STORAGE_GC_CLAIM, StorageQuery,
    StorageRow, StoreBackend,
};

/// One process on this host: pid, kernel start time and boot id (empty where the platform
/// does not expose them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: String,
    pub boot: String,
}

impl ProcessIdentity {
    pub fn current() -> Result<Self> {
        Self::of(std::process::id())?.context("read this process's start time")
    }

    /// The identity of the live process `pid`; `None` when there is none.
    pub fn of(pid: u32) -> Result<Option<Self>> {
        let Some(start_time) = process_start(pid)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            pid,
            start_time,
            boot: boot_id()?,
        }))
    }

    /// The claim and reservation owner name of this process.
    pub fn owner(&self) -> String {
        format!("pid-{}-{}-{}", self.pid, self.start_time, self.boot)
    }
}

/// True only when `holder` is provably gone: another boot, no live process with its pid, or a
/// live one with a different start time (pid reuse).
pub fn proven_dead(holder: &ProcessIdentity) -> Result<bool> {
    let boot = boot_id()?;
    if !boot.is_empty() && !holder.boot.is_empty() && boot != holder.boot {
        return Ok(true);
    }
    Ok(match process_start(holder.pid)? {
        None => true,
        Some(start) => {
            !start.is_empty() && !holder.start_time.is_empty() && start != holder.start_time
        }
    })
}

#[cfg(target_os = "linux")]
fn process_start(pid: u32) -> Result<Option<String>> {
    let path = format!("/proc/{pid}/stat");
    let stat = match std::fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {path}")),
    };
    // The command may hold spaces and parentheses; after its final ')' the state is the first
    // field and starttime (field 22 of the line) the twentieth.
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default();
    if matches!(fields.first(), Some(&"Z" | &"X")) {
        return Ok(None);
    }
    let start = fields
        .get(19)
        .with_context(|| format!("no start time in {path}"))?;
    Ok(Some((*start).to_owned()))
}

#[cfg(target_os = "linux")]
fn boot_id() -> Result<String> {
    let id =
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").context("read the boot id")?;
    Ok(id.trim().to_owned())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_start(pid: u32) -> Result<Option<String>> {
    let pid = libc::pid_t::try_from(pid).context("pid out of range")?;
    // SAFETY: signal 0 delivers nothing; it only asks whether the process exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(Some(String::new()));
    }
    let gone = std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    Ok((!gone).then(String::new))
}

#[cfg(not(unix))]
fn process_start(_pid: u32) -> Result<Option<String>> {
    Ok(Some(String::new()))
}

#[cfg(not(target_os = "linux"))]
fn boot_id() -> Result<String> {
    Ok(String::new())
}

pub(super) enum Acquired {
    Yes,
    Held { owner: String },
}

pub(super) struct GcClaim<'a> {
    pub backend: &'a dyn StoreBackend,
    pub clock: &'a dyn Clock,
    pub me: &'a ProcessIdentity,
}

impl GcClaim<'_> {
    /// Takes the claim, first releasing it from a holder proven dead.
    pub fn acquire(&self) -> Result<Acquired> {
        if let Some(held) = gc_claim(self.backend)? {
            if !holder_proven_dead(&held)? {
                return Ok(Acquired::Held { owner: held.owner });
            }
            let release = Operation::ReleaseClaim(ReleaseClaim {
                name: STORAGE_GC_CLAIM.to_owned(),
                owner: held.owner.clone(),
            });
            match self.backend.apply(release) {
                // A precondition failure means another process released it first; the claim
                // below decides which of us holds it.
                Ok(_) | Err(ApplyError::Precondition(_)) => {}
                Err(error) => {
                    return Err(anyhow!(
                        "release the storage-gc claim of dead holder {}: {error}",
                        held.owner
                    ));
                }
            }
        }
        let claim = Operation::Claim(Claim {
            name: STORAGE_GC_CLAIM.to_owned(),
            owner: self.me.owner(),
            acquired: self.clock.now(),
            doc: serde_json::to_value(self.me)?,
            reservations: Vec::new(),
        });
        match self.backend.apply(claim) {
            Ok(_) => Ok(Acquired::Yes),
            Err(ApplyError::Conflict(_)) => Ok(Acquired::Held {
                owner: gc_claim(self.backend)?
                    .map(|row| row.owner)
                    .unwrap_or_default(),
            }),
            Err(error) => Err(anyhow!("take the storage-gc claim: {error}")),
        }
    }

    pub fn release(&self) -> Result<()> {
        let release = Operation::ReleaseClaim(ReleaseClaim {
            name: STORAGE_GC_CLAIM.to_owned(),
            owner: self.me.owner(),
        });
        self.backend
            .apply(release)
            .map(drop)
            .map_err(|error| anyhow!("release the storage-gc claim: {error}"))
    }
}

fn gc_claim(backend: &dyn StoreBackend) -> Result<Option<ClaimRow>> {
    let view = backend.view()?;
    let rows = all_pages(|page| view.storage_rows(&StorageQuery::Claims, page))?;
    Ok(rows.into_iter().find_map(|row| match row {
        StorageRow::Claim(claim) if claim.name == STORAGE_GC_CLAIM => Some(claim),
        _ => None,
    }))
}

fn holder_proven_dead(claim: &ClaimRow) -> Result<bool> {
    match serde_json::from_value::<ProcessIdentity>(claim.doc.clone()) {
        Ok(holder) if holder.owner() == claim.owner => proven_dead(&holder),
        _ => Ok(false),
    }
}
