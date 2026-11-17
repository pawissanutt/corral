//! `corral storage`: integrity check, garbage collection by class and verified backups (plan
//! T-253 sections 7, 8 and 10).

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use corral_core::Store;
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use corral_ops::Ctx;
use corral_ops::storage_gc::{GcReport, backup_storage, check_storage, storage_gc};
use corral_protocol::cmd::{ClientKind, Principal, Via};

#[derive(clap::Subcommand)]
pub enum StorageCmd {
    /// Verify database integrity and that every live blob decodes to its hash; exits 1 on a problem
    Check {
        #[arg(long)]
        json: bool,
    },
    /// Repair stale sequence and derived status-count metadata; requires the owner's local CLI
    RepairMetadata {
        #[arg(long)]
        json: bool,
    },
    /// Report storage by class with bytes (--dry-run changes nothing) or reclaim it (--yes)
    Gc {
        /// Report what would be reclaimed and why the rest stays; change nothing
        #[arg(long, conflicts_with = "yes", required_unless_present = "yes")]
        dry_run: bool,
        /// Remove clean finished worktrees (branches kept), reclaimable scratch, terminal agent
        /// sessions after promoting their transcripts, legacy build output and unreferenced
        /// blobs past the grace period; one audited event per deletion
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Inspect or reclaim only explicitly declared private rebuildable outputs of inactive tasks
    Outputs {
        #[arg(long, conflicts_with_all = ["yes", "due"], required_unless_present_any = ["yes", "due"])]
        dry_run: bool,
        #[arg(long, conflicts_with = "due")]
        yes: bool,
        #[arg(long)]
        json: bool,
        /// Internal bounded asynchronous lifecycle worker
        #[arg(long, hide = true)]
        due: bool,
    },
    /// Copy the database and every live blob into DIR/.corral under a storage lease, each blob
    /// verified against its hash; the copy opens with `corral --root DIR`
    Backup {
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(start: &Path, cmd: StorageCmd, entry: &crate::entry::Entry) -> Result<()> {
    let ctx = if matches!(cmd, StorageCmd::RepairMetadata { .. }) {
        let caller = &entry.invoker().caller;
        if caller.client == ClientKind::App
            || caller.via == Via::App
            || matches!(&caller.principal, Principal::Device { .. })
        {
            bail!("metadata repair requires the owner's local CLI");
        }
        // Ordinary opens may observe config edits before the counter can be repaired.
        let store = Store::open_unobserved(&Store::find_root(start)?)?;
        corral_ops::require_automation_owner(&store, entry.invoker())?;
        Ctx::from_store(store)
    } else {
        entry.open(start)?
    };
    match cmd {
        StorageCmd::Check { json } => {
            let check = check_storage(&ctx.store)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&check)?);
            } else {
                for problem in &check.database {
                    println!("database: {problem}");
                }
                for problem in &check.blobs {
                    println!("{problem}");
                }
                for hash in &check.unreferenced_missing {
                    println!("unreferenced blob {hash} has no content (collectable)");
                }
                println!(
                    "storage check: {} ({} blobs verified)",
                    if check.is_ok() { "ok" } else { "FAILED" },
                    check.blobs_checked
                );
            }
            if !check.is_ok() {
                bail!("storage check found problems");
            }
            Ok(())
        }
        StorageCmd::RepairMetadata { json } => {
            let backend = SqliteBackend::open(ctx.store.dir(), SqliteOptions::default())?;
            let report = backend.repair_metadata()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if report.previous_change_seq == report.change_seq
                && !report.status_counts_rebuilt
            {
                println!("storage metadata is consistent; no repair needed");
            } else {
                println!(
                    "repaired storage metadata: change sequence {} -> {}, status counts rebuilt={} (audit recorded)",
                    report.previous_change_seq, report.change_seq, report.status_counts_rebuilt
                );
            }
            Ok(())
        }
        StorageCmd::Gc { yes, json, .. } => {
            let report = storage_gc(&ctx, yes)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
            Ok(())
        }
        StorageCmd::Outputs { yes, json, due, .. } => {
            if due {
                return corral_ops::storage_gc::outputs::output_pass(&ctx, ctx.store.now());
            }
            let entries = corral_ops::storage_gc::outputs::output_gc(&ctx, yes)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for entry in entries {
                    println!(
                        "{}:{} [{}] {} bytes={} reclaimed={} {:?}: {}",
                        entry.task,
                        entry.run.as_deref().unwrap_or("unknown"),
                        entry.output_class,
                        entry.path.display(),
                        entry
                            .measured_bytes
                            .map(|b| b.to_string())
                            .unwrap_or_else(|| "unknown".into()),
                        entry.reclaimed_bytes,
                        entry.disposition,
                        entry.reason
                    );
                }
            }
            Ok(())
        }
        StorageCmd::Backup { dir, json } => {
            let report = backup_storage(&ctx.store, &dir)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "backed up the database at change {} and {} blobs ({} bytes) into {}",
                    report.change_seq,
                    report.blobs,
                    report.blob_bytes,
                    dir.join(".corral").display()
                );
            }
            Ok(())
        }
    }
}

fn print_report(report: &GcReport) {
    for entry in &report.entries {
        let disposition = serde_json::to_value(entry.disposition).unwrap_or_default();
        println!(
            "{:<20} {:<12} {:>12}  {}{}",
            entry.class,
            disposition.as_str().unwrap_or(""),
            entry.bytes,
            entry.target,
            entry
                .reason
                .as_deref()
                .map(|reason| format!("  ({reason})"))
                .unwrap_or_default()
        );
    }
    println!(
        "\n{} totals (bytes):",
        if report.dry_run { "dry-run" } else { "gc" }
    );
    for (bucket, bytes) in &report.totals {
        let bucket = serde_json::to_value(bucket).unwrap_or_default();
        println!("  {:<26} {bytes}", bucket.as_str().unwrap_or(""));
    }
}
