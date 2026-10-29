//! `--rollback` (plan section 12): from `published` (R0-R7, archiving the whole database), from
//! a pre-publish state (U0-U6 then R5-R7, discarding the candidate wherever A6 left it), and the
//! resume table while `rolling_back`. Every refusal changes nothing.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::durable::{self, Seen};
use super::journal::{Candidate, Journal, JournalState, RollbackRecord, RollbackStep, write_fence};
use super::scan::{BuildRoot, Manifest};
use super::status::DirState;
use super::{DB, DISCARD, MANIFEST, Migrate, ROLLBACK_DIR, locks};
use crate::sqlite::SqliteBackend;
use crate::sqlite::migrate::{checkpoint_file, read_meta};

#[derive(Debug, Clone, Serialize)]
pub struct RolledBack {
    pub from: String,
    /// The step a resumed rollback continued from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discarded_changes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
    /// CACHEDIR.TAG build output `storage gc` reclaimed since the import (reported only).
    pub reclaimed_build_output: Vec<BuildRoot>,
}

pub(super) fn rollback(m: &Migrate, discard: Option<u64>) -> Result<RolledBack> {
    let corral = m.corral_dir();
    let _migrate = locks::migrate_lock(corral, m.wait)?;
    let Some(journal) = Journal::read(corral)? else {
        bail!(
            "nothing to roll back: {} has no migration journal",
            corral.display()
        );
    };
    if matches!(
        journal.state,
        JournalState::Finalizing | JournalState::Finalized
    ) {
        bail!(
            "the migration is {}; rollback is refused once finalize has started",
            journal.state.as_str()
        );
    }
    let _dispatch = locks::dispatch_lock(corral, m.wait)?;
    let _gate = locks::store_gate(corral, m.wait, || m.hit("gate_busy"))?;
    match journal.state {
        JournalState::Published => published(m, journal, discard),
        JournalState::Fenced | JournalState::Importing | JournalState::Verified => {
            pre_publish(m, journal)
        }
        JournalState::RollingBack => resume(m, journal),
        JournalState::Finalizing | JournalState::Finalized => unreachable!("refused above"),
    }
}

fn refuse_dirs(dirs: &DirState, what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{what}; refusing and changing nothing ({})",
        dirs.describe()
    )
}

fn published(m: &Migrate, journal: Journal, discard: Option<u64>) -> Result<RolledBack> {
    let corral = m.corral_dir();
    let dirs = DirState::observe(corral)?;
    if dirs.db != Seen::Dir || dirs.migrating.present() || dirs.discard.present() {
        return Err(refuse_dirs(&dirs, "a published rollback requires only db"));
    }
    let db_file = SqliteBackend::db_path(corral);
    let meta = read_meta(&db_file)?;
    let at_publish = journal
        .change_seq_at_publish
        .context("the published journal has no change_seq_at_publish")?;
    let n = meta.change_seq.saturating_sub(at_publish);
    if n != 0 && discard != Some(n) {
        bail!(
            "{n} v2 changes were committed since publication (change {} -> {}); rerun with \
             --discard-v2-changes {n} to archive them with the database; nothing changed",
            at_publish,
            meta.change_seq
        );
    }
    if let Some(named) = discard.filter(|named| *named != n) {
        bail!(
            "--discard-v2-changes {named} does not match the {n} v2 changes committed since publication; nothing changed"
        );
    }
    let m0 = Manifest::load(
        corral,
        journal
            .m0_digest
            .as_deref()
            .context("the journal has no M0")?,
    )?;
    let now = Manifest::capture(corral)?;
    let changed = m0.differences(&now);
    ensure!(
        changed.is_empty(),
        "legacy files changed since the import ({} differences, first: {}); nothing changed",
        changed.len(),
        changed.first().map_or("", String::as_str)
    );
    let reclaimed = m0.reclaimed_build_roots(&now);
    let checkpoint = checkpoint_file(&db_file)?;
    ensure!(
        !checkpoint.busy,
        "the database checkpoint did not complete; nothing changed"
    );
    let wal = wal_path(&db_file);
    ensure!(
        fs::metadata(&wal).map_or(true, |meta| meta.len() == 0),
        "{} is not empty after the checkpoint; nothing changed",
        wal.display()
    );
    m.hit("B8")?;
    let digest = journal.migration_digest.clone().unwrap_or_default();
    let archive = make_archive(corral, &digest, n, meta.change_seq)?;
    let mut journal = journal;
    journal.state = JournalState::RollingBack;
    journal.step = Some(RollbackStep::Intent);
    journal.rollback = Some(RollbackRecord {
        from: JournalState::Published,
        archive: Some(archive.clone()),
        discard: Some(n),
        change_seq_observed: Some(meta.change_seq),
        candidate: None,
    });
    journal.write(corral)?;
    m.hit("B9")?;
    archive_db(m, &mut journal, &archive)?;
    finish(m, journal, "intent", reclaimed)
}

fn wal_path(db_file: &Path) -> PathBuf {
    let mut wal = db_file.as_os_str().to_owned();
    wal.push("-wal");
    PathBuf::from(wal)
}

/// R2: a fresh `rollback/<UTC time>-<digest[:12]>` (suffix `-k` when taken) holding a durable
/// `ROLLBACK.json`.
fn make_archive(corral: &Path, digest: &str, n: u64, s: u64) -> Result<String> {
    let root = corral.join(ROLLBACK_DIR);
    fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let now = jiff::Timestamp::now();
    let stamp = now.strftime("%Y%m%dT%H%M%SZ").to_string();
    let short = &digest[..digest.len().min(12)];
    let base = format!("{stamp}-{short}");
    let mut name = base.clone();
    let mut k = 1;
    loop {
        match fs::create_dir(root.join(&name)) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                k += 1;
                name = format!("{base}-{k}");
            }
            Err(error) => return Err(error).with_context(|| format!("creating rollback {name}")),
        }
    }
    let dir = root.join(&name);
    let record = serde_json::json!({
        "from": "published", "discarded_changes": n, "change_seq": s,
        "migration_digest": digest, "at": now,
    });
    durable::write(
        &dir.join("ROLLBACK.json"),
        format!("{record:#}\n").as_bytes(),
        || Ok(()),
    )?;
    durable::sync_dir(&root)?;
    durable::sync_dir(corral)?;
    Ok(format!("{ROLLBACK_DIR}/{name}"))
}

/// R3-R4: move `C/db` into the archive, then record `archived`.
fn archive_db(m: &Migrate, journal: &mut Journal, archive: &str) -> Result<()> {
    let corral = m.corral_dir();
    let target = corral.join(archive).join(DB);
    durable::rename_dir(&corral.join(DB), &target)?;
    durable::sync_dir(&corral.join(archive))?;
    m.hit("B10")?;
    journal.step = Some(RollbackStep::Archived);
    journal.write(corral)?;
    m.hit("B11")
}

/// R5-R7: restore the fence, record `fence_restored`, unlink the journal and its manifest.
fn finish(
    m: &Migrate,
    mut journal: Journal,
    resumed: &str,
    reclaimed: Vec<BuildRoot>,
) -> Result<RolledBack> {
    let corral = m.corral_dir();
    if journal.step != Some(RollbackStep::FenceRestored) {
        write_fence(corral, journal.original_fence.as_deref(), || m.hit("B14"))?;
        m.hit("B12")?;
        journal.step = Some(RollbackStep::FenceRestored);
        journal.write(corral)?;
        m.hit("B13")?;
    }
    durable::unlink(&corral.join(MANIFEST))?;
    Journal::remove(corral)?;
    let record = journal.rollback()?;
    Ok(RolledBack {
        from: record.from.as_str().to_owned(),
        resumed_from: (resumed != "intent").then(|| resumed.to_owned()),
        archive: record.archive.clone(),
        discarded_changes: record.discard,
        candidate: record.candidate.map(|c| c.as_str().to_owned()),
        reclaimed_build_output: reclaimed,
    })
}

/// U1: where the candidate of a pre-publish journal is, by the classification table.
fn classify_candidate(corral: &Path, journal: &Journal, dirs: &DirState) -> Result<Candidate> {
    use Seen::{Absent, Dir};
    let candidate = match (journal.state, dirs.db, dirs.migrating, dirs.discard) {
        (JournalState::Fenced, Absent, Absent, Absent) => Candidate::None,
        (JournalState::Importing, Absent, Dir, Absent) => Candidate::Migrating,
        (JournalState::Importing, Absent, Absent, Absent) => Candidate::None,
        (JournalState::Verified, Absent, Dir, Absent) => Candidate::Migrating,
        (JournalState::Verified, Dir, Absent, Absent) => Candidate::Db,
        _ => {
            return Err(refuse_dirs(
                dirs,
                &format!(
                    "the {} journal does not match the database directories",
                    journal.state.as_str()
                ),
            ));
        }
    };
    if journal.state == JournalState::Verified
        && let Some(dir) = candidate.dir()
    {
        check_candidate(corral, journal, dir)?;
    }
    Ok(candidate)
}

/// A verified candidate must be this migration's database at its recorded change count.
pub(super) fn check_candidate(corral: &Path, journal: &Journal, dir: &str) -> Result<()> {
    let file = corral.join(dir).join(crate::sqlite::DB_FILE);
    let meta = read_meta(&file)?;
    let expected = journal.migration_digest.as_deref();
    ensure!(
        meta.migration_digest.as_deref() == expected,
        "{dir}/{} has migration_digest {:?}, the journal records {:?}; refusing and changing nothing",
        crate::sqlite::DB_FILE,
        meta.migration_digest,
        expected
    );
    ensure!(
        Some(meta.change_seq) == journal.import_seq,
        "{dir}/{} has change_seq {}, the journal records import_seq {:?}; refusing and changing nothing",
        crate::sqlite::DB_FILE,
        meta.change_seq,
        journal.import_seq
    );
    Ok(())
}

fn pre_publish(m: &Migrate, mut journal: Journal) -> Result<RolledBack> {
    let corral = m.corral_dir();
    let dirs = DirState::observe(corral)?;
    let candidate = classify_candidate(corral, &journal, &dirs)?;
    journal.rollback = Some(RollbackRecord {
        from: journal.state,
        archive: None,
        discard: None,
        change_seq_observed: None,
        candidate: Some(candidate),
    });
    journal.state = JournalState::RollingBack;
    journal.step = Some(RollbackStep::Intent);
    journal.write(corral)?;
    m.hit("B15")?;
    quarantine(m, &mut journal, candidate)?;
    clean(m, &mut journal)?;
    finish(m, journal, "intent", Vec::new())
}

/// U3-U4: one rename of the candidate to `db.discard`, then `quarantined`.
fn quarantine(m: &Migrate, journal: &mut Journal, candidate: Candidate) -> Result<()> {
    let corral = m.corral_dir();
    if let Some(dir) = candidate.dir() {
        durable::rename_dir(&corral.join(dir), &corral.join(DISCARD))?;
    }
    m.hit("B16")?;
    journal.step = Some(RollbackStep::Quarantined);
    journal.write(corral)
}

/// U5-U6: delete `db.discard` (no-follow, idempotent), then `cleaned`.
fn clean(m: &Migrate, journal: &mut Journal) -> Result<()> {
    let corral = m.corral_dir();
    durable::remove_tree(&corral.join(DISCARD), || m.hit("B17"))?;
    durable::sync_dir(corral)?;
    journal.step = Some(RollbackStep::Cleaned);
    journal.write(corral)?;
    m.hit("B24")
}

fn resume(m: &Migrate, mut journal: Journal) -> Result<RolledBack> {
    use Seen::{Absent, Dir};
    let corral = m.corral_dir();
    let dirs = DirState::observe(corral)?;
    let step = journal
        .step
        .context("the rolling_back journal has no step")?;
    let record = journal.rollback()?.clone();
    if step == RollbackStep::FenceRestored {
        return finish(m, journal, step.as_str(), Vec::new());
    }
    if record.from == JournalState::Published {
        let archive = record
            .archive
            .clone()
            .context("the rollback records no archive")?;
        let archived = Seen::Dir == durable::seen(&corral.join(&archive).join(DB))?;
        let half_built = !corral.join(&archive).join("ROLLBACK.json").is_file();
        if dirs.migrating.present() || dirs.discard.present() || half_built {
            return Err(refuse_dirs(
                &dirs,
                &format!("rollback archive {archive} is not resumable"),
            ));
        }
        match (step, dirs.db, archived) {
            (RollbackStep::Intent, Dir, false) => archive_db(m, &mut journal, &archive)?,
            (RollbackStep::Intent, Absent, true) => {
                journal.step = Some(RollbackStep::Archived);
                journal.write(corral)?;
                m.hit("B11")?;
            }
            (RollbackStep::Archived, Absent, true) => {}
            _ => {
                return Err(refuse_dirs(
                    &dirs,
                    &format!(
                        "step {} with {archive}/db {}",
                        step.as_str(),
                        if archived { "present" } else { "absent" }
                    ),
                ));
            }
        }
        return finish(m, journal, step.as_str(), Vec::new());
    }
    let candidate = record.candidate.unwrap_or(Candidate::None);
    let other_present = match candidate {
        Candidate::Db => dirs.migrating.present(),
        Candidate::Migrating => dirs.db.present(),
        Candidate::None => dirs.db.present() || dirs.migrating.present(),
    };
    let symlink = [dirs.db, dirs.migrating, dirs.discard]
        .iter()
        .any(|s| !matches!(s, Seen::Absent | Seen::Dir));
    if other_present || symlink {
        return Err(refuse_dirs(
            &dirs,
            "a database directory other than the recorded candidate exists",
        ));
    }
    let at = candidate
        .dir()
        .map_or(Ok(Seen::Absent), |dir| durable::seen(&corral.join(dir)))?;
    match step {
        RollbackStep::Intent => match (candidate, at.present(), dirs.discard.present()) {
            (Candidate::None, _, false) | (_, true, false) => {
                quarantine(m, &mut journal, candidate)?;
            }
            (Candidate::Db | Candidate::Migrating, false, true) => {
                journal.step = Some(RollbackStep::Quarantined);
                journal.write(corral)?;
            }
            _ => {
                return Err(refuse_dirs(
                    &dirs,
                    &format!("candidate {} and db.discard", candidate.as_str()),
                ));
            }
        },
        RollbackStep::Quarantined if !at.present() => {}
        RollbackStep::Cleaned if !at.present() && !dirs.discard.present() => {
            return finish(m, journal, step.as_str(), Vec::new());
        }
        _ => {
            return Err(refuse_dirs(
                &dirs,
                &format!("rollback step {} is not resumable here", step.as_str()),
            ));
        }
    }
    clean(m, &mut journal)?;
    finish(m, journal, step.as_str(), Vec::new())
}
