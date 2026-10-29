//! `--apply`: the forward path A1-A7 of plan section 12 and its resume column.

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::blockers::LegacyBoard;
use super::durable::{self, Seen};
use super::journal::{Journal, JournalState, fence_value, read_fence, write_fence};
use super::rollback::check_candidate;
use super::scan::{Manifest, classify_all, walk};
use super::status::DirState;
use super::{DB, MIGRATING, Migrate, import, locks};
use crate::blob::FsBlobStore;
use crate::sqlite::{SqliteBackend, SqliteOptions, migrate::checkpoint_file};
use crate::store::db::FENCE;

#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    pub state: String,
    /// The state the journal was in when this run started (`none` for a fresh apply).
    pub resumed_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import: Option<import::ImportReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_seq: Option<u64>,
}

pub(super) fn apply(m: &Migrate) -> Result<Applied> {
    let corral = m.corral_dir();
    let _migrate = locks::migrate_lock(corral, m.wait)?;
    let journal = Journal::read(corral)?;
    let resumed_from = journal
        .as_ref()
        .map_or("none", |j| j.state.as_str())
        .to_owned();
    let mut report = None;
    let pre_publish = journal
        .as_ref()
        .is_none_or(|j| matches!(j.state, JournalState::Fenced | JournalState::Importing));
    let dispatch = if pre_publish {
        Some(locks::dispatch_lock(corral, m.wait)?)
    } else {
        None
    };
    let mut journal = match journal {
        None => fresh(m)?,
        Some(j) if j.state == JournalState::RollingBack => {
            bail!(
                "a rollback is in progress; finish the rollback first (corral migrate --rollback)"
            )
        }
        Some(j) => j,
    };
    if journal.state == JournalState::Fenced {
        fence(m, &journal)?;
    }
    if journal.state == JournalState::Importing || journal.state == JournalState::Fenced {
        report = Some(import(m, &mut journal)?);
    }
    drop(dispatch);
    if journal.state == JournalState::Verified {
        publish(m, &mut journal)?;
    }
    Ok(Applied {
        state: journal.state.as_str().to_owned(),
        resumed_from,
        import: report,
        change_seq: journal.change_seq_at_publish,
    })
}

/// A1-A2 (the caller holds `dispatch`): the drain check, no database directory, then `fenced`.
fn fresh(m: &Migrate) -> Result<Journal> {
    let corral = m.corral_dir();
    let original = read_fence(corral)?;
    if fence_value(original.as_deref()) == Some(FENCE) {
        bail!(
            "{} is already a v2 store (fence {FENCE}) with no migration journal; nothing to migrate",
            corral.display()
        );
    }
    drain_check(m)?;
    let journal = Journal {
        state: JournalState::Fenced,
        step: None,
        original_fence: original,
        m0_digest: None,
        migration_digest: None,
        import_seq: None,
        change_seq_at_publish: None,
        rollback: None,
        legacy_paths: Vec::new(),
        started: jiff::Timestamp::now(),
    };
    journal.write(corral)?;
    m.hit("B1")?;
    Ok(journal)
}

/// A1: no database directory may exist, every legacy file is classified and nothing is open.
fn drain_check(m: &Migrate) -> Result<()> {
    let corral = m.corral_dir();
    let dirs = DirState::observe(corral)?;
    for (name, seen) in [
        (DB, dirs.db),
        (MIGRATING, dirs.migrating),
        (super::DISCARD, dirs.discard),
    ] {
        ensure!(
            seen == Seen::Absent,
            "{} exists ({seen:?}) without a migration journal; refusing and changing nothing",
            corral.join(name).display()
        );
    }
    let (_, unclassified) = classify_all(&walk(corral)?);
    ensure!(
        unclassified.is_empty(),
        "{} legacy files are not classified: {}; refusing and changing nothing",
        unclassified.len(),
        unclassified.join(", ")
    );
    let board = LegacyBoard::read(corral)?;
    let (blockers, _) = board.blockers(corral)?;
    if !blockers.is_empty() {
        let list: Vec<String> = blockers.iter().map(ToString::to_string).collect();
        bail!(
            "the board has open work; drain it first ({} blockers):\n  {}",
            blockers.len(),
            list.join("\n  ")
        );
    }
    let missing = board.missing_snapshots(corral);
    ensure!(
        missing.is_empty(),
        "{} registered artifacts have no snapshot to import: {}; refusing and changing nothing",
        missing.len(),
        missing.join("; ")
    );
    Ok(())
}

/// A3: fence := 4 (redone after a kill that left the old fence).
fn fence(m: &Migrate, journal: &Journal) -> Result<()> {
    let corral = m.corral_dir();
    let dirs = DirState::observe(corral)?;
    ensure!(
        !dirs.db.present() && !dirs.migrating.present() && !dirs.discard.present(),
        "the fenced journal does not match the database directories; refusing ({})",
        dirs.describe()
    );
    if fence_value(read_fence(corral)?.as_deref()) != Some(FENCE) {
        if journal.original_fence == read_fence(corral)? {
            drain_check(m)?;
        }
        write_fence(corral, Some("4\n"), || Ok(()))?;
    }
    m.hit("B2")
}

/// A4-A5: M0, `importing`, a fresh candidate in `db.migrating`, the import and its checks,
/// then `verified` with the candidate's digest and change count.
fn import(m: &Migrate, journal: &mut Journal) -> Result<import::ImportReport> {
    let corral = m.corral_dir();
    let dirs = DirState::observe(corral)?;
    ensure!(
        !dirs.db.present() && !dirs.discard.present() && dirs.migrating != Seen::Symlink,
        "the {} journal does not match the database directories; refusing ({})",
        journal.state.as_str(),
        dirs.describe()
    );
    let m0 = Manifest::capture(corral)?;
    let m0_digest = m0.save(corral)?;
    journal.state = JournalState::Importing;
    journal.m0_digest = Some(m0_digest.clone());
    journal.write(corral)?;
    let candidate_dir = corral.join(MIGRATING);
    durable::remove_tree(&candidate_dir, || Ok(()))?;
    std::fs::create_dir(&candidate_dir)
        .with_context(|| format!("creating {}", candidate_dir.display()))?;
    durable::sync_dir(corral)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(format!(
            "{m0_digest}:{}:{}",
            jiff::Timestamp::now(),
            std::process::id()
        ))
    );
    let db_file = candidate_dir.join(crate::sqlite::DB_FILE);
    let backend = SqliteBackend::open_file(&db_file, SqliteOptions::default())?;
    backend.mark_migration(&digest)?;
    let blobs = FsBlobStore::open(corral)?;
    let report = import::run(m, &backend, &blobs, &m0)?;
    import::verify(corral, &backend, &blobs, &m0, &report)?;
    let problems = crate::sqlite::migrate::integrity_problems(&db_file)?;
    ensure!(
        problems.is_empty(),
        "the imported database failed integrity_check: {}",
        problems.join("; ")
    );
    let differences = m0.differences(&Manifest::capture(corral)?);
    ensure!(
        differences.is_empty(),
        "legacy files changed during the import ({}; first: {})",
        differences.len(),
        differences.first().map_or("", String::as_str)
    );
    let import_seq = backend.change_seq()?;
    let checkpoint = backend.checkpoint_truncate()?;
    ensure!(
        !checkpoint.busy,
        "the candidate checkpoint did not complete"
    );
    drop(backend);
    let checkpoint = checkpoint_file(&db_file)?;
    ensure!(
        !checkpoint.busy,
        "the candidate checkpoint did not complete"
    );
    durable::sync_file(&db_file)?;
    durable::sync_dir(&candidate_dir)?;
    durable::sync_dir(corral)?;
    journal.state = JournalState::Verified;
    journal.migration_digest = Some(digest);
    journal.import_seq = Some(import_seq);
    journal.write(corral)?;
    m.hit("B4")?;
    Ok(report)
}

/// A6-A7 (and their resume): under the exclusive gate, rename the candidate to `db`, then
/// record `published`.
fn publish(m: &Migrate, journal: &mut Journal) -> Result<()> {
    let corral = m.corral_dir();
    let gate = locks::store_gate(corral, m.wait, || m.hit("gate_busy"))?;
    m.hit("B5")?;
    let dirs = DirState::observe(corral)?;
    match (dirs.db, dirs.migrating, dirs.discard) {
        (Seen::Absent, Seen::Dir, Seen::Absent) => {
            check_candidate(corral, journal, MIGRATING)?;
            durable::rename_dir(&corral.join(MIGRATING), &corral.join(DB))?;
            m.hit("B6")?;
        }
        (Seen::Dir, Seen::Absent, Seen::Absent) => check_candidate(corral, journal, DB)?,
        _ => bail!(
            "the verified journal does not match the database directories; refusing and changing nothing ({})",
            dirs.describe()
        ),
    }
    journal.state = JournalState::Published;
    journal.change_seq_at_publish = journal.import_seq;
    journal.write(corral)?;
    m.hit("B7")?;
    drop(gate);
    Ok(())
}
