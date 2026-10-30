//! Durable assistant session inspection and lifecycle (launch and resume are `corral pi`'s).
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_ops::Ctx;

#[derive(Args)]
pub struct AssistantArgs {
    #[command(subcommand)]
    pub cmd: AssistantCmd,
}

#[derive(Subcommand)]
pub enum AssistantCmd {
    /// Allocate a session record and unbound container, without a task or coding slot
    Create {
        #[arg(long, default_value = "pi")]
        profile: String,
        #[arg(long)]
        focus: Option<String>,
        #[arg(long)]
        pane: Option<String>,
        #[arg(long)]
        herdr_socket: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List assistant sessions: live (host running), open (resumable) or ended
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show a session, its durable container and usage
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// End a session (stopping a running host), keeping its record and transcript references
    End {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Replace a session's Markdown summary from a UTF-8 file
    Summary {
        id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Scratch worktrees for spikes and co-work
    Scratch(ScratchArgs),
}

#[derive(Args)]
pub struct ScratchArgs {
    #[command(subcommand)]
    pub cmd: ScratchCmd,
}

/// An assistant shell acts for its own session; the owner names one with `--session`.
#[derive(Subcommand)]
pub enum ScratchCmd {
    /// Create a scratch worktree at <repo>/.worktrees/A-<n>-<slug> on corral/A-<n>/<slug>
    New {
        #[arg(long, short = 'p')]
        project: String,
        /// Lowercase letters, digits and hyphens; default scratch-<k>
        #[arg(long)]
        slug: Option<String>,
        /// Ref to branch from; default the project's base, else its current branch
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List scratch worktrees (all sessions for the owner unless --session)
    List {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove a scratch and its branch, saving any work as a session diff artifact first
    Discard {
        slug: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// File a Backlog task with the scratch diff attached; the scratch stays unchanged
    Promote {
        slug: String,
        #[arg(long)]
        title: String,
        /// Project for the task; default the scratch's project
        #[arg(long, short = 'p')]
        project: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Collect idle scratch of ended sessions, saving work as diff artifacts first (owner)
    Gc {
        /// List what would be collected without changing anything
        #[arg(long, conflicts_with = "yes")]
        dry_run: bool,
        /// Collect; required for a real run
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
}

fn status(ended: bool, live: bool) -> &'static str {
    match (ended, live) {
        (true, _) => "ended",
        (false, true) => "live",
        (false, false) => "open",
    }
}

pub fn run(ctx: &Ctx, args: AssistantArgs) -> Result<()> {
    match args.cmd {
        AssistantCmd::Create {
            profile,
            focus,
            pane,
            herdr_socket,
            json,
        } => {
            let session = ctx.store.create_assistant(
                &profile,
                focus.as_deref(),
                pane.as_deref(),
                herdr_socket,
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&session)?);
            } else {
                println!("{}", session.id);
            }
        }
        AssistantCmd::List { json } => {
            let rows = corral_ops::assistant_stats(&ctx.store, None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                println!("SESSION STATUS FOCUS PANE COST SCRATCH INBOX");
                for row in rows {
                    println!(
                        "{} {} {} {} {} {} {}",
                        row.session,
                        status(row.ended.is_some(), row.live),
                        row.focus,
                        row.pane.as_deref().unwrap_or("-"),
                        row.cost,
                        row.scratch,
                        row.inbox_items
                    );
                }
            }
        }
        AssistantCmd::Show { id, json } => {
            let session = ctx.store.get_assistant(&id)?;
            let container = ctx.store.get_assistant_container(&id)?;
            let usage = corral_ops::assistant_usage(&ctx.store, &session)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "session": session, "body": session.body, "container": container, "usage": usage,
                    }))?
                );
            } else {
                println!(
                    "{} {} focus={} profile={} pane={}",
                    session.id,
                    if session.ended.is_some() {
                        "ended"
                    } else {
                        "open"
                    },
                    session.focus,
                    session.profile,
                    container.pane.as_deref().unwrap_or("-")
                );
                println!(
                    "cost: {}",
                    usage
                        .map(|u| u.summary())
                        .unwrap_or_else(|| "unknown".into())
                );
                for generation in &container.state.generations {
                    if let Some(bound) = &generation.bound {
                        println!(
                            "g{} transcript: {}",
                            generation.n,
                            bound.session_file.display()
                        );
                    }
                }
                println!("{}", session.body);
            }
        }
        AssistantCmd::End { id, json } => {
            let session = corral_ops::agent::pi::end_assistant(ctx, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&session)?);
            } else {
                println!("{} ended", session.id);
            }
        }
        AssistantCmd::Summary { id, file, json } => {
            let body = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let session = ctx.store.update_assistant(&id, |session| {
                session.summary = body.lines().next().unwrap_or("").to_owned();
                session.body = body;
                Ok(())
            })?;
            ctx.store.log_event(
                None,
                "assistant_summary",
                serde_json::json!({"session": id}),
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&session)?);
            } else {
                println!("{} summary saved", session.id);
            }
        }
        AssistantCmd::Scratch(args) => scratch(ctx, args.cmd)?,
    }
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn scratch(ctx: &Ctx, cmd: ScratchCmd) -> Result<()> {
    use corral_ops::assistant::scratch as ops;
    match cmd {
        ScratchCmd::New {
            project,
            slug,
            base,
            session,
            json,
        } => {
            let session = ops::session_for(session.as_deref())?;
            let made = ops::create(ctx, &session, &project, slug.as_deref(), base.as_deref())?;
            if json {
                return print_json(&made);
            }
            println!(
                "{session}/{} {} on {}",
                made.slug,
                made.path.display(),
                made.branch
            );
        }
        ScratchCmd::List { session, json } => {
            let session = match (std::env::var_os("CORRAL_ASSISTANT"), session) {
                (None, None) => None,
                (_, named) => Some(ops::session_for(named.as_deref())?),
            };
            let rows = ops::list(ctx, session.as_deref())?;
            if json {
                return print_json(&rows);
            }
            println!("SCRATCH KIND PROJECT BRANCH PATH");
            for row in rows {
                println!(
                    "{}/{} {} {} {} {}",
                    row.session,
                    row.scratch.slug,
                    row.scratch.kind,
                    row.scratch.project,
                    row.scratch.branch,
                    row.scratch.path.display()
                );
            }
        }
        ScratchCmd::Discard {
            slug,
            session,
            json,
        } => {
            let session = ops::session_for(session.as_deref())?;
            let removed = ops::discard(ctx, &session, &slug)?;
            if json {
                return print_json(&removed);
            }
            if removed.artifacts.is_empty() {
                println!("{session}/{slug} discarded; nothing beyond its base");
            } else {
                println!(
                    "{session}/{slug} discarded; work saved as {}",
                    removed.artifacts.join(", ")
                );
            }
        }
        ScratchCmd::Promote {
            slug,
            title,
            project,
            body_file,
            session,
            json,
        } => {
            let session = ops::session_for(session.as_deref())?;
            let body = body_file
                .map(|path| {
                    std::fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))
                })
                .transpose()?
                .unwrap_or_default();
            let promoted = ops::promote(ctx, &session, &slug, &title, project.as_deref(), &body)?;
            if json {
                return print_json(&promoted);
            }
            println!("{} (diff {})", promoted.task, promoted.artifact);
        }
        ScratchCmd::Gc { dry_run, yes, json } => {
            if !dry_run && !yes {
                anyhow::bail!("pass --dry-run to preview or --yes to collect");
            }
            let report = ops::gc(ctx, jiff::Timestamp::now(), dry_run)?;
            if json {
                print_json(&report)?;
            } else {
                println!(
                    "Scratch GC {} (idle {}d)",
                    if dry_run { "dry run" } else { "applied" },
                    report.idle_days
                );
                for item in &report.items {
                    println!(
                        "{:<8} {}/{} {} bytes={} {}",
                        item.action,
                        item.session,
                        item.slug,
                        item.path.display(),
                        item.bytes.map_or_else(|| "-".into(), |b| b.to_string()),
                        item.reason
                    );
                }
                if let Some(why) = &report.panes_unavailable {
                    println!("pane checks skipped (terminal server unavailable: {why})");
                }
            }
            if report.failed() > 0 {
                anyhow::bail!(
                    "{} scratch worktree(s) could not be collected",
                    report.failed()
                );
            }
        }
    }
    Ok(())
}
