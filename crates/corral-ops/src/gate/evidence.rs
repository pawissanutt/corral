//! Gate evidence on the storage-v2 model (plan T-253 sections 3, 7 and 8): each attempt's
//! control files and live log sit in a registered scratch directory `gates/<stem>` under the
//! scratch root; `GateStart` records the identity, commands and context documents, and
//! `GateResult` the log and result documents, as exact-byte blobs of one `gate_attempts` row
//! and closes the directory. A closed directory is reclaimed only once no process uses it.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::backend::{
    GateAttempt, GateLaunched, Operation, ReservationUse, ScratchKind, ScratchState,
};
use corral_core::blob_ref::BlobRef;
use corral_core::{CandidateIdentity, GateOutcome, GateResult, ScratchReclaim, Store};
use jiff::Timestamp;
use serde_json::Value;

use crate::Ctx;

/// The `gate_attempts.scope` of a task run's gate.
pub(crate) const RUN_SCOPE: &str = "run";

/// The failed command of an attempt that ended without a result and was replaced.
pub(crate) const SUPERSEDED_LOST: &str =
    "process_lost: the gate ended without a result and was superseded";

#[cfg(test)]
type FailpointHook = Option<Box<dyn FnMut(&str)>>;

#[cfg(test)]
thread_local! {
    pub(crate) static FAILPOINT: std::cell::RefCell<FailpointHook> =
        const { std::cell::RefCell::new(None) };
}

/// Test-only interruption points of gate commits; a no-op in shipped builds.
pub(crate) fn failpoint(name: &str) {
    #[cfg(test)]
    FAILPOINT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(name);
        }
    });
    #[cfg(not(test))]
    let _ = name;
}

#[cfg(all(test, unix))]
mod tests;

/// `<scratch_root>/gates`; under `.corral/state/gates` only when the configured roots are
/// invalid, where every registration is refused before anything is written.
pub(crate) fn gates_root(ctx: &Ctx) -> PathBuf {
    ctx.store.storage_layout().map_or_else(
        |_| ctx.store.dir().join("state").join("gates"),
        |layout| layout.scratch_root().join("gates"),
    )
}

/// The slim gate event fields of a tier receipt: tier, context digest and carry method.
pub(crate) fn tier_detail(detail: &mut Value, tier: Option<&corral_core::GateTierReceipt>) {
    let Some(tier) = tier else {
        return;
    };
    detail["tier"] = Value::from(tier.tier.as_str());
    detail["context_sha256"] = Value::from(tier.context_sha256.as_str());
    if tier.carried.is_some() {
        detail["carry"] = Value::from("clean_replay");
    } else if tier.promoted_from.is_some() {
        detail["carry"] = Value::from("promoted");
    }
}

fn scratch_id(stem: &str) -> String {
    format!("gate-{stem}")
}

/// Registers the attempt's scratch directory, or returns it while its registration is still
/// open (a resumed start). A collected attempt's directory is never reused.
pub(crate) fn open_dir(ctx: &Ctx, task: &str, attempt: &str, stem: &str) -> Result<PathBuf> {
    let id = scratch_id(stem);
    if let Some(row) = ctx.store.scratch_row(&id)? {
        ensure!(
            row.state == ScratchState::Open,
            "gate attempt {stem} was already collected; start a new attempt"
        );
        return Ok(gates_root(ctx).join(stem));
    }
    let rel = format!("gates/{stem}");
    ctx.store
        .register_scratch(ScratchKind::Gate, &id, task, attempt, &rel)
}

/// What [`put_start`] stores for one attempt.
pub(crate) struct Start<'a> {
    pub(crate) task: &'a str,
    pub(crate) run: &'a str,
    pub(crate) attempt: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) sha: &'a str,
    pub(crate) tier: Option<&'a str>,
    pub(crate) identity: &'a CandidateIdentity,
    pub(crate) commands: &'a [String],
    pub(crate) context: Option<&'a Path>,
}

/// A started attempt's row with its put identity, commands and context blobs, ready to commit.
pub(crate) struct StagedStart {
    row: GateAttempt,
    reservations: Vec<ReservationUse>,
}

impl StagedStart {
    /// Joins the open task update, so the row commits with the update's gate marker. It may
    /// be staged again when the update retries its callback.
    pub(crate) fn stage(&self, store: &Store) -> Result<()> {
        store.stage_gate_start(&self.row, self.reservations.clone())
    }
}

/// Puts the attempt's identity, commands and context documents as exact-byte blobs and
/// builds its row; `None` when the attempt already has a row.
pub(crate) fn put_start(ctx: &Ctx, start: &Start<'_>) -> Result<Option<StagedStart>> {
    if ctx
        .store
        .gate_attempt(start.task, start.run, start.attempt)?
        .is_some()
    {
        return Ok(None);
    }
    let identity = ctx
        .store
        .put_gate_blob(&serde_json::to_vec(start.identity)?)?;
    let commands = ctx
        .store
        .put_gate_blob(&serde_json::to_vec(start.commands)?)?;
    // A recovered start whose context file is gone records none; its receipt keeps the digest.
    let context = match start.context.map(std::fs::read) {
        Some(Ok(bytes)) => Some(ctx.store.put_gate_blob(&bytes)?),
        Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).context("reading gate context");
        }
        _ => None,
    };
    let mut reservations = vec![identity.reservation, commands.reservation];
    let row = GateAttempt {
        run: start.run.to_owned(),
        attempt: start.attempt.to_owned(),
        scope: start.scope.to_owned(),
        sha: start.sha.to_owned(),
        tier: start.tier.map(str::to_owned),
        identity: identity.blob,
        commands: commands.blob,
        context: context.as_ref().map(|put| put.blob.clone()),
        launched: None,
        result: None,
        failed_cmd: None,
        log: None,
        result_doc: None,
        started: Timestamp::now(),
        ended: None,
    };
    reservations.extend(context.map(|put| put.reservation));
    Ok(Some(StagedStart { row, reservations }))
}

/// Records the attempt's pane once (`GateLaunched`); a row-less (legacy) attempt is left alone.
pub(crate) fn record_launched(
    ctx: &Ctx,
    task: &str,
    run: &str,
    attempt: &str,
    pane: &str,
) -> Result<()> {
    match ctx.store.gate_attempt(task, run, attempt)? {
        Some(row) if row.launched.is_none() && row.ended.is_none() => {
            ctx.store
                .apply_gate(task, Vec::new(), Vec::new(), |change| {
                    Operation::GateLaunched(GateLaunched {
                        change,
                        run: run.to_owned(),
                        attempt: attempt.to_owned(),
                        pane: Value::String(pane.to_owned()),
                    })
                })
        }
        _ => Ok(()),
    }
}

/// An attempt's put log and result blobs and its open scratch registration, ready to end its
/// row in the task update that records the gate's final outcome.
pub(crate) struct StagedResult {
    run: String,
    attempt: String,
    log: Option<BlobRef>,
    result_doc: Option<BlobRef>,
    close_scratch: Vec<String>,
    reservations: Vec<ReservationUse>,
}

impl StagedResult {
    /// Joins the open task update with `outcome`, so the update commits as one `GateResult`:
    /// its marker and events, the ended row and the scratch closure. It may be staged again
    /// when the update retries its callback.
    pub(crate) fn stage(&self, store: &Store, outcome: (GateResult, Option<&str>)) -> Result<()> {
        store.stage_gate_result(
            GateOutcome {
                run: self.run.clone(),
                attempt: self.attempt.clone(),
                result: outcome.0.as_str().to_owned(),
                failed_cmd: outcome.1.map(str::to_owned),
                log: self.log.clone(),
                result_doc: self.result_doc.clone(),
                close_scratch: self.close_scratch.clone(),
            },
            self.reservations.clone(),
        )
    }
}

/// Puts the log and result files' exact bytes as blobs for ending the attempt's row; `None`
/// for a row-less (legacy) or already ended attempt.
pub(crate) fn put_result(
    ctx: &Ctx,
    task: &str,
    run: &str,
    attempt: &str,
    stem: &str,
    paths: &super::GatePaths,
) -> Result<Option<StagedResult>> {
    let Some(row) = ctx.store.gate_attempt(task, run, attempt)? else {
        return Ok(None);
    };
    if row.ended.is_some() {
        return Ok(None);
    }
    let mut reservations = Vec::new();
    let mut put = |path: &Path| -> Result<Option<BlobRef>> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let blob = ctx.store.put_gate_blob(&bytes)?;
        reservations.push(blob.reservation);
        Ok(Some(blob.blob))
    };
    let log = put(&paths.log)?;
    let result_doc = put(&paths.result)?;
    let id = scratch_id(stem);
    let close_scratch = ctx
        .store
        .scratch_row(&id)?
        .filter(|row| row.state == ScratchState::Open)
        .map(|_| id)
        .into_iter()
        .collect();
    Ok(Some(StagedResult {
        run: run.to_owned(),
        attempt: attempt.to_owned(),
        log,
        result_doc,
        close_scratch,
        reservations,
    }))
}

/// The exact bytes of the attempt's recorded log; `None` without a row, log or live blob, and
/// when the blob's file is gone or fails verification, which proves nothing as a missing or
/// changed log file did. Other read errors are returned.
pub(crate) fn attempt_log(
    ctx: &Ctx,
    task: &str,
    run: &str,
    attempt: &str,
) -> Result<Option<Vec<u8>>> {
    let Some(GateAttempt { log: Some(log), .. }) = ctx.store.gate_attempt(task, run, attempt)?
    else {
        return Ok(None);
    };
    match ctx.store.blob_bytes(&log) {
        Ok(bytes) => Ok(bytes),
        Err(error) if lost_or_corrupt(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn lost_or_corrupt(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData
            )
        })
}

/// Reclaims the closed scratch directory of attempt `stem` unless a process still uses it;
/// an open (running) attempt is never touched.
pub(crate) fn reclaim(ctx: &Ctx, stem: &str) -> Result<ScratchReclaim> {
    let owner = format!("gate-{}", std::process::id());
    ctx.store.reclaim_scratch(&scratch_id(stem), &owner, in_use)
}

/// Why `dir` is still in use: a process whose working directory or open file is under it,
/// or a held `.cargo-lock` inside it. Other platforms cannot tell, so they always defer.
fn in_use(dir: &Path) -> Result<Option<String>> {
    let dir = match std::fs::canonicalize(dir) {
        Ok(dir) => dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("resolving {}", dir.display())),
    };
    if cfg!(not(target_os = "linux")) {
        return Ok(Some(
            "process use cannot be checked on this platform".into(),
        ));
    }
    if let Some(user) = process_under(&dir)? {
        return Ok(Some(user));
    }
    held_cargo_lock(&dir)
}

fn process_under(dir: &Path) -> Result<Option<String>> {
    let processes = std::fs::read_dir("/proc").context("listing /proc")?;
    for process in processes.flatten() {
        let name = process.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let base = process.path();
        let mut links: Vec<PathBuf> = vec![base.join("cwd")];
        // A process that exits or is not ours to inspect mid-scan uses nothing we can see.
        if let Ok(fds) = std::fs::read_dir(base.join("fd")) {
            links.extend(fds.flatten().map(|fd| fd.path()));
        }
        if let Some(path) = links
            .iter()
            .filter_map(|link| std::fs::read_link(link).ok())
            .find(|target| target.starts_with(dir))
        {
            return Ok(Some(format!(
                "process {} uses {}",
                name.to_string_lossy(),
                path.display()
            )));
        }
    }
    Ok(None)
}

fn held_cargo_lock(dir: &Path) -> Result<Option<String>> {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let entries = match std::fs::read_dir(&next) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("listing {}", next.display())),
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", next.display()))?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.file_name() == ".cargo-lock" {
                let file = std::fs::File::open(entry.path())?;
                match file.try_lock() {
                    Ok(()) => file.unlock()?,
                    Err(std::fs::TryLockError::WouldBlock) => {
                        return Ok(Some(format!("{} is held", entry.path().display())));
                    }
                    Err(std::fs::TryLockError::Error(error)) => {
                        return Err(error)
                            .with_context(|| format!("locking {}", entry.path().display()));
                    }
                }
            }
        }
    }
    Ok(None)
}
