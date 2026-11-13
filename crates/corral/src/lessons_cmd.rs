use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use corral_protocol::cmd::{Command, FileInput, knowledge::KnowledgeCommand};

#[derive(Args)]
pub struct LessonsArgs {
    #[arg(long, global = true)]
    project: Option<String>,
    /// Preview relevant lessons, or show a run's exact dispatch snapshot with --applied
    #[arg(long = "for")]
    task: Option<String>,
    #[arg(long, requires = "task")]
    applied: bool,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    pub(crate) cmd: Option<LessonsCmd>,
}

#[derive(Subcommand)]
pub(crate) enum LessonsCmd {
    /// File one bounded spike on the configured strong profile; does not dispatch
    Distill {
        #[arg(long)]
        profile: Option<String>,
    },
    /// Validate and apply the spike's evidence-bound JSON proposal
    Apply {
        #[arg(long)]
        file: PathBuf,
    },
    /// Keep a lesson from decaying or being removed by distillation
    Pin {
        id: String,
        #[arg(long)]
        unpin: bool,
    },
    /// Remove a lesson from its project document and matching index
    Delete { id: String },
    /// Show due schedules and pending distillation tasks without writing
    Due,
}

pub fn run(inv: &crate::invocation::Invocation, args: LessonsArgs) -> Result<()> {
    if args.task.is_some() && args.cmd.is_some() {
        bail!("--for cannot be combined with a lessons subcommand");
    }
    match args.cmd {
        Some(LessonsCmd::Distill { profile }) => {
            return inv.submit(Command::Knowledge(KnowledgeCommand::Distill {
                project: args.project,
                profile,
                json: args.json,
            }));
        }
        Some(LessonsCmd::Apply { file }) => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            return inv.submit(Command::Knowledge(KnowledgeCommand::Apply {
                project: args.project,
                file: FileInput {
                    path: file.display().to_string(),
                    text,
                },
            }));
        }
        Some(LessonsCmd::Pin { id, unpin }) => {
            return inv.submit(Command::Knowledge(KnowledgeCommand::Pin {
                project: args.project,
                id,
                unpin,
            }));
        }
        Some(LessonsCmd::Delete { id }) => {
            return inv.submit(Command::Knowledge(KnowledgeCommand::Delete {
                project: args.project,
                id,
            }));
        }
        Some(LessonsCmd::Due) => {
            let ctx = inv.ctx()?;
            let due = corral_ops::lessons_due(ctx, jiff::Timestamp::now())?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&due)?);
            } else {
                for d in due {
                    println!(
                        "{}: {}; profile {}; pending {}",
                        d.project,
                        d.reasons.join(", "),
                        d.profile
                            .as_deref()
                            .unwrap_or("(configure a strong profile)"),
                        d.pending.as_deref().unwrap_or("none")
                    );
                }
            }
        }
        None => {
            let ctx = inv.ctx()?;
            let lessons = if let Some(id) = args.task {
                let task = ctx.store.get(&id)?;
                if args
                    .project
                    .as_ref()
                    .is_some_and(|p| Some(p) != task.project.as_ref())
                {
                    bail!("task {} does not belong to the selected project", task.id);
                }
                if args.applied {
                    let run = task.current_run().context("task has no run")?;
                    corral_ops::applied_lessons(ctx, &task, run)?
                        .context("legacy run has no recorded lesson exposure")?
                } else {
                    corral_ops::lessons_for(ctx, &task, task.current_run())?
                }
            } else if let Some(p) = args.project {
                ctx.store.project_path(&p)?;
                ctx.store.lessons(&p)?
            } else {
                let mut all = Vec::new();
                for p in ctx.store.config.projects.keys() {
                    for lesson in ctx.store.lessons(p)? {
                        all.push(serde_json::json!({"project":p,"lesson":lesson}));
                    }
                }
                println!("{}", serde_json::to_string_pretty(&all)?);
                return Ok(());
            };
            if args.json {
                println!("{}", serde_json::to_string_pretty(&lessons)?);
            } else {
                for lesson in lessons {
                    println!(
                        "{}{}{} ({:.2}): {}\n  {}",
                        lesson.id,
                        if lesson.pinned { " [pinned]" } else { "" },
                        if lesson.contradicted {
                            " [contradicted]"
                        } else {
                            ""
                        },
                        lesson.confidence,
                        lesson.rule,
                        lesson.why
                    );
                    for e in lesson.evidence {
                        println!("  Evidence: {} {} {}", e.task, e.ts, e.source);
                    }
                }
            }
        }
    }
    Ok(())
}
