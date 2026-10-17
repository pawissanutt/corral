use anyhow::{Context, Result, anyhow, bail};
use corral_core::{ArtifactKind, ArtifactTier, Store};
use corral_ops::Ctx;

use crate::ArtifactCmd;

/// A validated, one-based inclusive source line range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineRange {
    pub start: u64,
    pub end: u64,
}

impl std::str::FromStr for LineRange {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let invalid = || anyhow!("invalid line range '{value}': expected a-b with 1 <= a <= b");
        let (start, end) = value.split_once('-').ok_or_else(invalid)?;
        if start.is_empty()
            || end.is_empty()
            || !start.bytes().all(|b| b.is_ascii_digit())
            || !end.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(invalid());
        }
        let start = start.parse::<u64>().map_err(|_| invalid())?;
        let end = end.parse::<u64>().map_err(|_| invalid())?;
        if start == 0 || end < start {
            return Err(invalid());
        }
        Ok(Self { start, end })
    }
}

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
        ArtifactCmd::Show {
            reference,
            raw,
            lines,
        } => {
            let lines = lines.as_deref().map(str::parse).transpose()?;
            crate::artifact_show::show(ctx, &reference, raw, lines)?;
        }
        ArtifactCmd::Serve {
            reference,
            task,
            no_open,
            stop,
            daemon,
            idle_secs,
        } => serve(ctx, reference, task, !no_open, stop, daemon, idle_secs)?,
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
        ArtifactCmd::List { task, json, tier } => {
            let want = task.as_deref().map(Store::normalize_id).transpose()?;
            let tier = tier
                .as_deref()
                .map(|t| {
                    ArtifactTier::parse(t)
                        .ok_or_else(|| anyhow!("unknown tier '{t}' (keep, normal, transient)"))
                })
                .transpose()?;
            let mut rows: Vec<_> = corral_ops::all_artifacts(ctx)?
                .into_iter()
                .filter(|r| want.as_ref().is_none_or(|w| &r.task_id == w))
                .filter(|r| tier.is_none_or(|t| r.artifact.tier() == t))
                .collect();
            rows.sort_by(|a, b| corral_ops::cmp_importance(&a.artifact, &b.artifact));
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

fn serve(
    ctx: &Ctx,
    reference: Option<String>,
    task: Option<String>,
    open: bool,
    stop: bool,
    daemon: bool,
    idle_secs: u64,
) -> Result<()> {
    if daemon {
        return corral_ops::run_server(
            ctx.clone(),
            std::time::Duration::from_secs(idle_secs),
            |info| {
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                writeln!(out, "{}", serde_json::to_string(info)?)?;
                out.flush()
                    .context("announcing the artifact server address")
            },
        );
    }
    if stop {
        println!(
            "{}",
            if corral_ops::stop_server(ctx)? {
                "artifact server stopped"
            } else {
                "no artifact server running"
            }
        );
        return Ok(());
    }
    let served = match reference {
        Some(r) => corral_ops::serve_artifact(ctx, &corral_ops::resolve_artifact(ctx, &r)?, open)?,
        None => corral_ops::serve_index(ctx, task.as_deref(), open)?,
    };
    println!("{}", served.url);
    match (&served.launched, &served.note) {
        (Some(argv), _) => println!("opened with {}", argv.join(" ")),
        (None, Some(note)) => println!("{note}"),
        (None, None) => {}
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

#[cfg(test)]
mod tests {
    use super::LineRange;

    #[test]
    fn line_ranges_are_positive_inclusive_and_ordered() {
        assert_eq!(
            "2-4".parse::<LineRange>().unwrap(),
            LineRange { start: 2, end: 4 }
        );
        assert_eq!(
            "1-18446744073709551615".parse::<LineRange>().unwrap().end,
            u64::MAX
        );
        for invalid in [
            "",
            "1",
            "1-",
            "-2",
            "0-1",
            "2-1",
            "1-2-3",
            " 1-2",
            "1-2 ",
            "+1-2",
            "1-18446744073709551616",
        ] {
            assert!(invalid.parse::<LineRange>().is_err(), "{invalid:?}");
        }
    }
}
