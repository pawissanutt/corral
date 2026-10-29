//! `corral migrate`: move a legacy file store into the database, roll it back, or finalize it
//! (plan T-253 section 12; runbook in section 13).

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use corral_core::migrate::{DryRun, Migrate, Status};

#[derive(clap::Args)]
#[command(group(clap::ArgGroup::new("action").required(true).args(["status", "dry_run", "apply", "rollback", "finalize"])))]
pub struct MigrateArgs {
    /// Show the migration state, step, candidate location and next command; changes nothing
    #[arg(long)]
    status: bool,
    /// List blockers (open work) and classify every legacy file; creates nothing
    #[arg(long)]
    dry_run: bool,
    /// Fence the legacy store, import it into .corral/db and publish it (resumes after a crash)
    #[arg(long)]
    apply: bool,
    /// Restore the legacy store: discard an unpublished candidate, or archive the published
    /// database under .corral/rollback/ (resumes after a crash)
    #[arg(long)]
    rollback: bool,
    /// Allow a rollback that archives exactly N v2 changes made since publication
    #[arg(long, value_name = "N", requires = "rollback")]
    discard_v2_changes: Option<u64>,
    /// Delete the legacy files of a published store (requires --yes)
    #[arg(long)]
    finalize: bool,
    /// Confirm --finalize
    #[arg(long, requires = "finalize")]
    yes: bool,
    /// Seconds --apply, --rollback and --finalize wait for other corral processes to close the store
    #[arg(long, value_name = "SECS", default_value_t = 10)]
    wait: u64,
    #[arg(long)]
    json: bool,
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn run(root: &Path, args: MigrateArgs) -> Result<()> {
    let migrate = Migrate::new(root).wait(Duration::from_secs(args.wait));
    if args.status {
        let status = migrate.status()?;
        return if args.json {
            print_json(&status)
        } else {
            print_status(&status);
            Ok(())
        };
    }
    if args.dry_run {
        let report = migrate.dry_run()?;
        if args.json {
            print_json(&report)?;
        } else {
            print_dry_run(&report);
        }
        if !report.ok {
            bail!(
                "the store cannot be migrated yet: {} blockers, {} unclassified files",
                report.blockers.len(),
                report.unclassified.len()
            );
        }
        return Ok(());
    }
    if args.apply {
        let applied = migrate.apply()?;
        if args.json {
            return print_json(&applied);
        }
        println!(
            "migration {} (resumed from {})",
            applied.state, applied.resumed_from
        );
        if let Some(import) = &applied.import {
            println!(
                "imported {} tasks, {} events, {} records, {} legacy files, {} blobs ({} reused)",
                import.tasks,
                import.events,
                import.records.values().sum::<u64>(),
                import.legacy_files,
                import.blobs,
                import.blobs_reused
            );
        }
        println!("next: corral storage check");
        return Ok(());
    }
    if args.rollback {
        let done = migrate.rollback(args.discard_v2_changes)?;
        if args.json {
            return print_json(&done);
        }
        println!(
            "rolled back from {}; the legacy store is restored",
            done.from
        );
        if let Some(archive) = &done.archive {
            println!("the v2 database is kept under .corral/{archive}/db");
        }
        for root in &done.reclaimed_build_output {
            println!(
                "build output reclaimed by storage gc: {} ({} bytes)",
                root.path, root.bytes
            );
        }
        return Ok(());
    }
    if !args.yes {
        bail!("--finalize deletes the legacy files; rerun with --finalize --yes");
    }
    let done = migrate.finalize()?;
    if args.json {
        return print_json(&done);
    }
    println!(
        "migration {}; deleted {} legacy paths",
        done.state,
        done.deleted.len()
    );
    Ok(())
}

fn print_status(status: &Status) {
    println!("state: {}", status.state);
    if let Some(step) = &status.step {
        println!("step: {step}");
    }
    println!(
        "fence: {}",
        status.fence.as_deref().map_or("absent", str::trim)
    );
    if let Some(candidate) = &status.candidate {
        println!("candidate: {candidate}");
    }
    if let Some(archive) = &status.archive {
        println!("archive: {archive}");
    }
    if let Some(changes) = status.changes {
        println!("v2 changes since publication: {changes}");
    }
    for problem in &status.problems {
        println!("problem: {problem}");
    }
    for next in &status.next {
        println!("next: {next}");
    }
}

fn print_dry_run(report: &DryRun) {
    println!(
        "state: {} (fence {})",
        report.state,
        report.fence.as_deref().map_or("absent", str::trim)
    );
    for blocker in &report.blockers {
        println!("blocker: {blocker}");
    }
    for record in &report.history {
        println!("history: {record}");
    }
    for path in &report.unclassified {
        println!("unclassified: {path}");
    }
    let schemas: Vec<String> = report
        .tasks_by_schema
        .iter()
        .map(|(schema, n)| format!("{n} {schema}"))
        .collect();
    println!("tasks: {}", schemas.join(", "));
    println!(
        "runs: {} coding, {} review, {} plan; events: {} lines",
        report.runs, report.review_runs, report.plan_runs, report.event_lines
    );
    println!(
        "artifacts: {} bytes raw, {} bytes stored; legacy review files: {} ({} bytes)",
        report.artifacts_raw_bytes,
        report.artifacts_stored_bytes,
        report.legacy_review_files.files,
        report.legacy_review_files.bytes
    );
    for (area, count) in &report.areas {
        println!("area {area}: {} files, {} bytes", count.files, count.bytes);
    }
    println!(
        "build output left in place: {} directories, {} bytes",
        report.build_output.len(),
        report.build_output_bytes
    );
    println!(
        "{}",
        if report.ok {
            "ready to migrate"
        } else {
            "not ready"
        }
    );
}
