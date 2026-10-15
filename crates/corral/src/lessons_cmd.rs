use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use corral_ops::Ctx;

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
    cmd: Option<LessonsCmd>,
}

#[derive(Subcommand)]
enum LessonsCmd {
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

fn project(ctx: &Ctx, selected: Option<&str>) -> Result<String> {
    selected
        .map(str::to_string)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| corral_ops::infer_project(ctx, &cwd))
        })
        .or_else(|| {
            (ctx.store.config.projects.len() == 1)
                .then(|| ctx.store.config.projects.keys().next().cloned())
                .flatten()
        })
        .context("select a project with --project")
}

pub fn run(ctx: &Ctx, args: LessonsArgs) -> Result<()> {
    if args.task.is_some() && args.cmd.is_some() {
        bail!("--for cannot be combined with a lessons subcommand");
    }
    match args.cmd {
        Some(LessonsCmd::Due) => {
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
        Some(LessonsCmd::Distill { profile }) => {
            let project = project(ctx, args.project.as_deref())?;
            let task = corral_ops::lessons_distill(ctx, &project, profile.as_deref())?;
            if args.json {
                let mut value = serde_json::to_value(&task)?;
                value["body"] = serde_json::Value::String(task.body.clone());
                value["runs"] = serde_json::to_value(&task.runs)?;
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("{}: {} (filed, not dispatched)", task.id, task.title);
            }
        }
        Some(LessonsCmd::Apply { file }) => {
            let project = project(ctx, args.project.as_deref())?;
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let proposal = serde_json::from_str(&text).context("parsing lesson proposal")?;
            corral_ops::lessons_apply(ctx, &project, &proposal)?;
            println!("Applied lessons for {project}");
        }
        Some(LessonsCmd::Pin { id, unpin }) => {
            let project = project(ctx, args.project.as_deref())?;
            ctx.store.pin_lesson(&project, &id, !unpin)?;
            println!("{} {id}", if unpin { "Unpinned" } else { "Pinned" });
        }
        Some(LessonsCmd::Delete { id }) => {
            let project = project(ctx, args.project.as_deref())?;
            ctx.store.delete_lesson(&project, &id)?;
            println!("Deleted {id}");
        }
        None => {
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
