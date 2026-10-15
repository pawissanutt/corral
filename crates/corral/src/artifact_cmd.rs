use std::io::Write;

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{ArtifactKind, ArtifactTier, Store};
use corral_ops::Ctx;

use crate::ArtifactCmd;

pub fn run(ctx: &Ctx, cmd: ArtifactCmd) -> Result<()> {
    match cmd {
        ArtifactCmd::Add {
            id,
            path,
            kind,
            title,
        } => {
            let kind = kind
                .as_deref()
                .map(|k| {
                    ArtifactKind::parse(k).ok_or_else(|| anyhow!("unknown artifact kind '{k}'"))
                })
                .transpose()?;
            let path = std::path::absolute(&path).unwrap_or(path);
            let a = corral_ops::add_artifact(ctx, &id, &path, kind, title.as_deref())?;
            println!(
                "{} {} {} {}",
                a.run,
                a.kind.as_str(),
                a.tier().as_str(),
                a.path
            );
        }
        ArtifactCmd::Show { reference } => {
            let artifact = corral_ops::resolve_artifact(ctx, &reference)?;
            let bytes = corral_ops::artifact_read(ctx, &artifact)?;
            std::io::stdout()
                .lock()
                .write_all(&bytes)
                .context("writing artifact content")?;
        }
        ArtifactCmd::SetTier { reference, tier } => {
            let tier = ArtifactTier::parse(&tier).context("unknown artifact tier")?;
            let a = corral_ops::set_artifact_tier(ctx, &reference, tier)?;
            println!("{} {}", a.path, a.tier().as_str());
        }
        ArtifactCmd::Gc { dry_run, yes, json } => {
            if !dry_run && !yes {
                bail!("artifact gc requires --yes to apply retention, or --dry-run to preview");
            }
            let report = corral_ops::artifact_gc(ctx, dry_run)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_gc(&report);
            }
        }
        ArtifactCmd::List { task, json } => {
            let want = task.as_deref().map(Store::normalize_id).transpose()?;
            let rows: Vec<_> = corral_ops::all_artifacts(ctx)?
                .into_iter()
                .filter(|r| want.as_ref().is_none_or(|w| &r.task_id == w))
                .collect();
            if json {
                let v: Vec<_> = rows
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "task": r.task_id, "task_title": r.task_title, "project": r.project,
                            "tier": r.artifact.tier(), "artifact": r.artifact,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                for r in &rows {
                    println!(
                        "{:<7} {:<3} {:<10} {:<9} {:>10} {}  {}{}",
                        r.task_id,
                        r.artifact.run,
                        r.artifact.kind.as_str(),
                        r.artifact.tier().as_str(),
                        r.artifact.bytes,
                        r.artifact.title,
                        r.artifact.path,
                        if r.artifact.purged_at.is_some() {
                            " [purged]"
                        } else {
                            ""
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

fn print_gc(report: &corral_ops::GcReport) {
    println!(
        "{}",
        if report.dry_run {
            "Artifact GC dry run"
        } else {
            "Artifact GC applied"
        }
    );
    for action in &report.actions {
        println!("{:<8} {} {}", action.action, action.task_id, action.path);
    }
    println!("bytes saved: {}", report.bytes_saved);
}

pub fn storage(ctx: &Ctx, json: bool) -> Result<()> {
    let stats = corral_ops::storage_stats(ctx)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!(
            "{:<12} {:>16} {:>16}",
            "TASK", "ORIGINAL BYTES", "STORED BYTES"
        );
        for task in &stats.tasks {
            println!(
                "{:<12} {:>16} {:>16}",
                task.task_id, task.original_bytes, task.stored_bytes
            );
        }
        println!(
            "{:<12} {:>16} {:>16}",
            "workspace", stats.original_bytes, stats.stored_bytes
        );
        println!("Workspace stored totals count shared content once.");
        for warning in &stats.warnings {
            println!("warning: {warning}");
        }
    }
    Ok(())
}
