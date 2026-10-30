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
    }
    Ok(())
}
