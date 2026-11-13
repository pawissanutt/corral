//! Durable assistant session inspection and lifecycle (launch and resume are `corral pi`'s).
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_core::{CoworkReview, TaskKind, TaskSize};
use corral_ops::Ctx;
use corral_protocol::cmd::{Command, FileInput, assistant::AssistantCommand};

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
        /// With --json: bounded host readback; history rows become counts
        #[arg(long, requires = "json")]
        host: bool,
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
    /// Owner-only co-work transfer and current-revision continuation (JSON)
    Handoff(crate::assistant_handoff_cmd::HandoffArgs),
    /// Evidence-only stale executor reconciliation; unknown resources remain outstanding
    Effects(crate::assistant_effects_cmd::EffectsArgs),
    /// Scratch worktrees for spikes and co-work
    Scratch(ScratchArgs),
    /// Write permissions the owner gave a session: its grants, settings and revocation
    Permissions(PermissionsArgs),
}

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct PermissionsArgs {
    #[command(subcommand)]
    pub cmd: Option<PermissionsCmd>,
    /// Session whose grants, generation settings and saved settings to list
    #[arg(required = true)]
    pub id: Option<String>,
    #[arg(long)]
    pub json: bool,
}

/// Only the owner's shell or the session's own shell revokes; nothing here creates a grant.
#[derive(Subcommand)]
pub enum PermissionsCmd {
    /// Revoke one active grant, or every active grant with --all
    Revoke {
        id: String,
        /// Grant ID, G-<n>
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        grant: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
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
    /// Submit a scratch as a task or repair; confirmed finishes require gate and independent review (owner)
    Finish {
        slug: String,
        /// Title of the new task
        #[arg(long, required_unless_present = "into", conflicts_with = "into")]
        title: Option<String>,
        /// Project for the task; default the scratch's project
        #[arg(long, short = 'p')]
        project: Option<String>,
        #[arg(long, conflicts_with = "into")]
        kind: Option<TaskKind>,
        #[arg(long, conflicts_with = "into")]
        size: Option<TaskSize>,
        /// Explicitly use ad-hoc capacity for a new task; repairs retain the existing lane
        #[arg(long, conflicts_with = "into")]
        adhoc: bool,
        /// Legacy review option; confirmed owner finishes always require independent review
        #[arg(long, default_value = "none", value_parser = ["none", "required"])]
        review: String,
        /// Add the run to this existing task instead; refused while it has a live run
        #[arg(long)]
        into: Option<String>,
        /// Context for the run summary and a new task's body
        #[arg(long)]
        note: Option<String>,
        /// Refuse `stale_candidate` unless the scratch HEAD is this commit (the candidate the owner was shown)
        #[arg(long, value_name = "SHA")]
        expected_head: Option<String>,
        /// Submit verification/repairs only, or authorize shipping (repairs retain prior intent)
        #[arg(long, value_parser = ["verify", "ship"])]
        intent: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Collect ended or idle hostless sessions' scratch, saving work first (owner or PM)
    Gc {
        /// Stop resident processes only for ended or idle hostless sessions
        #[arg(long)]
        stop_stale: bool,
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

pub fn run(inv: &crate::invocation::Invocation, args: AssistantArgs) -> Result<()> {
    match args.cmd {
        AssistantCmd::Handoff(args) => crate::assistant_handoff_cmd::run(inv.ctx()?, args)?,
        AssistantCmd::Effects(args) => crate::assistant_effects_cmd::run(inv, args)?,
        AssistantCmd::Create {
            profile,
            focus,
            pane,
            herdr_socket,
            json,
        } => {
            return inv.submit(Command::Assistant(AssistantCommand::Create {
                profile,
                focus,
                pane,
                herdr_socket,
                json,
            }));
        }
        AssistantCmd::List { json } => {
            let ctx = inv.ctx()?;
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
        AssistantCmd::Show { id, json, host } => {
            let ctx = inv.ctx()?;
            let session = ctx.store.get_assistant(&id)?;
            let (container, workflow) =
                ctx.store.inspect_assistant_container(&id, |container, _| {
                    let workflow = corral_ops::assistant::workflow::read_state(
                        &ctx.store, &id, container, 0, 3,
                    )?;
                    Ok((container.clone(), workflow))
                })?;
            let usage = corral_ops::assistant_usage(&ctx.store, &session)?;
            let host_assets =
                corral_ops::assistant::host::host_assets(&ctx.store, &container.state);
            let directory = corral_ops::assistant::directory(&ctx.store, &session)?;
            if json {
                let value = serde_json::json!({
                    "session": session, "body": session.body, "container": container, "usage": usage,
                    "host": host_assets, "workflow": workflow, "directory": directory,
                });
                if host {
                    let bounded = corral_ops::assistant::show::host_projection(value);
                    println!("{}", serde_json::to_string(&bounded)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                }
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
                if session.ended.is_none() {
                    println!("{}", host_assets.line(&session.id));
                    println!("owner actions: {}", host_assets.owner_actions.join(", "));
                }
                if container.workflow.is_some() {
                    println!("workflow: {}", serde_json::to_string(&workflow)?);
                }
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
            let ctx = inv.ctx()?;
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
            return inv.submit(Command::Assistant(AssistantCommand::Summary {
                id,
                file: FileInput {
                    path: file.display().to_string(),
                    text: body,
                },
                json,
            }));
        }
        AssistantCmd::Scratch(args) => scratch(inv.ctx()?, args.cmd)?,
        AssistantCmd::Permissions(args) => permissions(inv, args)?,
    }
    Ok(())
}

fn print_settings(label: &str, settings: Option<&corral_ops::assistant::permissions::SettingsRow>) {
    let Some(settings) = settings else {
        println!("{label}: no write_permissions setting");
        return;
    };
    println!(
        "{label}: {}, allow {}, deny {}",
        if settings.enabled {
            "enabled"
        } else {
            "disabled"
        },
        settings.allow.len(),
        settings.deny.len()
    );
    for row in &settings.allow {
        match (&row.canonical, &row.ignored) {
            (Some(canonical), _) => println!("  allow {} -> {}", row.entry, canonical.display()),
            (None, why) => println!(
                "  allow {} ignored: {}",
                row.entry,
                why.as_deref().unwrap_or("unresolved")
            ),
        }
    }
    for entry in &settings.deny {
        println!("  deny {entry}");
    }
}

fn permissions(inv: &crate::invocation::Invocation, args: PermissionsArgs) -> Result<()> {
    use corral_ops::assistant::permissions as ops;
    if let Some(PermissionsCmd::Revoke {
        id,
        grant,
        all,
        json,
    }) = args.cmd
    {
        return inv.submit(Command::Assistant(AssistantCommand::PermissionsRevoke {
            id,
            grant,
            all,
            json,
        }));
    }
    let ctx = inv.ctx()?;
    let id = args
        .id
        .context("name the session: corral assistant permissions A-<n>")?;
    let home = corral_core::agent::write_permissions::Home::from_env();
    let listing = ops::list(&ctx.store, &id, jiff::Timestamp::now(), &home)?;
    if args.json {
        return print_json(&listing);
    }
    println!(
        "{} {} generation {}",
        listing.session,
        if listing.ended { "ended" } else { "open" },
        listing.generation
    );
    println!("GRANT STATE LIFETIME SCOPE EXPIRES");
    for row in &listing.grants {
        let grant = &row.grant;
        println!(
            "{} {} {} {} -> {} {}",
            grant.id,
            row.state.as_str(),
            grant.lifetime.as_str(),
            grant.scope.shown,
            grant.scope.canonical,
            grant
                .expires_at
                .map_or_else(|| "-".into(), |at| at.to_string())
        );
    }
    print_settings(
        &format!("generation {} settings", listing.generation),
        listing.generation_settings.as_ref(),
    );
    if listing.saved_differs {
        print_settings(
            "saved settings (differ; a resume applies them)",
            listing.saved_settings.as_ref(),
        );
    } else {
        println!("saved settings: the same as the generation's");
    }
    if listing.audit_queued > 0 {
        println!("{} audit line(s) queued", listing.audit_queued);
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
        ScratchCmd::Finish {
            slug,
            title,
            project,
            kind,
            size,
            adhoc,
            review,
            into,
            note,
            expected_head,
            intent,
            session,
            json,
        } => {
            let session = ops::finish_session(session.as_deref())?;
            let review = if review == "required" {
                CoworkReview::Required
            } else {
                CoworkReview::None
            };
            let opts = ops::Finish {
                title: title.as_deref(),
                project: project.as_deref(),
                kind,
                size,
                adhoc,
                review: Some(review),
                into: into.as_deref(),
                note: note.as_deref(),
                expected_head: expected_head.as_deref(),
                intent: intent.map(|intent| {
                    if intent == "verify" {
                        corral_core::CompletionIntent::Verify
                    } else {
                        corral_core::CompletionIntent::Ship
                    }
                }),
            };
            let done = ops::finish(ctx, &session, &slug, &opts)?;
            if json {
                return print_json(&done);
            }
            println!(
                "{} {} from {session}/{slug} ({} at {}): {}; gate {}",
                done.task,
                done.run,
                done.branch,
                done.head.get(..7).unwrap_or(&done.head),
                done.status.label(),
                done.gate
            );
            match &done.reviewer {
                Some(reviewer) => println!("review required: {reviewer}"),
                None => {
                    println!("review pending; queued completion follows its intent and live policy")
                }
            }
        }
        ScratchCmd::Gc {
            dry_run,
            yes,
            json,
            stop_stale,
        } => {
            if !dry_run && !yes {
                anyhow::bail!("pass --dry-run to preview or --yes to collect");
            }
            let report = ops::gc_with_options(ctx, jiff::Timestamp::now(), dry_run, stop_stale)?;
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
                        match item.action {
                            "skip" => "keep",
                            "remove" => "collect",
                            action => action,
                        },
                        item.session,
                        item.slug,
                        item.path.display(),
                        item.bytes.map_or_else(|| "-".into(), |b| b.to_string()),
                        item.reason
                    );
                    for holder in &item.holders {
                        println!(
                            "         pid={} age={}s command={:?}",
                            holder.pid, holder.age_secs, holder.command
                        );
                    }
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
