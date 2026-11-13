//! `task` command executors.
use anyhow::{Context, Result, anyhow, bail};
use corral_core::{CommentAuthor, Origin, ProfileChoice, Status, Store};
use corral_protocol::cmd::task::TaskCommand;

use super::{Exec, Verb};

fn status(value: &str) -> Result<Status> {
    Status::parse(value).ok_or_else(|| anyhow!("unknown status '{value}' (idea, backlog, ready, running, blocked, review, done, dropped)"))
}

impl Verb for TaskCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::New {
                title,
                origin,
                project,
                status: target,
                body,
                profile,
                depends,
                priority,
                kind,
                size,
                difficulty,
                design,
                labels,
            } => {
                let origin = match origin.as_str() {
                    "owner" => Origin::Owner,
                    "pm" => Origin::Pm,
                    "agent" => Origin::Agent,
                    _ => bail!("origin must be owner, pm or agent"),
                };
                let target = status(&target)?;
                let deps = depends
                    .iter()
                    .map(|d| Store::normalize_id(d))
                    .collect::<Result<Vec<_>>>()?;
                let kind = kind
                    .map(|v| {
                        v.parse::<corral_core::TaskKind>()
                            .map_err(anyhow::Error::msg)
                    })
                    .transpose()?;
                let size = size
                    .map(|v| {
                        v.parse::<corral_core::TaskSize>()
                            .map_err(anyhow::Error::msg)
                    })
                    .transpose()?;
                let difficulty = difficulty
                    .map(|v| {
                        v.parse::<corral_core::TaskDifficulty>()
                            .map_err(anyhow::Error::msg)
                    })
                    .transpose()?;
                let cwd = cx.invoker.cwd().context("reading the current directory")?;
                let task = crate::new_task_initialized(
                    &cx.ctx,
                    &title.join(" "),
                    project.as_deref(),
                    &cwd,
                    target,
                    &body,
                    |t| {
                        t.origin = Some(origin);
                        t.profile = profile;
                        t.depends_on = deps;
                        if let Some(p) = priority {
                            t.priority = p.to_ascii_uppercase();
                        }
                        t.kind = kind.map(|v| v.to_string());
                        t.size = size;
                        t.difficulty = difficulty;
                        t.design = design;
                        t.labels = labels;
                        Ok(())
                    },
                )?;
                cx.out.line(format_args!("{}", task.id));
            }
            Self::Comment {
                id,
                text,
                author,
                send,
            } => {
                let author = match author.as_str() {
                    "owner" => CommentAuthor::Owner,
                    "pm" => CommentAuthor::Pm,
                    _ => bail!("unknown author '{author}' (owner, pm)"),
                };
                let task = crate::comment(&cx.ctx, &id, &text, author, send)?;
                cx.out.line(format_args!("{} commented", task.id));
            }
            Self::Move {
                id,
                status: target,
                force,
                owner_ok,
                reason,
            } => {
                let task = crate::move_task_with_decision(
                    &cx.ctx,
                    &id,
                    status(&target)?,
                    force,
                    reason.as_deref(),
                    cx.invoker.decision_author(&cx.ctx.store)?,
                    owner_ok,
                )?;
                cx.out
                    .line(format_args!("{} -> {}", task.id, task.status.label()));
            }
            Self::Idea {
                id,
                action,
                reason,
                into,
                body,
                owner_ok,
            } => {
                let task = crate::decide_idea(
                    &cx.ctx,
                    &id,
                    &action,
                    &reason,
                    crate::IdeaDecisionOptions {
                        into: into.as_deref(),
                        body: body.as_deref(),
                        owner_ok,
                        actor: cx.invoker.decision_author(&cx.ctx.store)?,
                    },
                )?;
                cx.out.line(format_args!(
                    "{}: {} ({})",
                    task.id,
                    action,
                    task.status.label()
                ));
            }
            Self::Route {
                id,
                set_reviewer,
                clear_reviewer,
                profile,
                model,
                effort,
                json,
            } => {
                if set_reviewer.is_some() || clear_reviewer {
                    crate::reviewer_set(&cx.ctx, &id, set_reviewer.as_deref())?;
                }
                explain(
                    cx,
                    &id,
                    &ProfileChoice {
                        profile,
                        model,
                        effort,
                    },
                    json,
                )?;
            }
            Self::Hold { id, reason } => crate::hold::hold(&cx.ctx, &id, reason.as_deref())?,
            Self::Release { id } => crate::hold::release(&cx.ctx, &id)?,
            Self::WaiveReview { id, reason } => {
                crate::review_waiver::waive(&cx.ctx, &id, &reason, cx.invoker)?;
                cx.out.line(format_args!("{id}: review waived; it lands once its gate passes, marked landed without review"));
            }
            Self::Focus { id, all } => crate::hold::focus(&cx.ctx, id.as_deref(), all)?,
            Self::RecoverInfrastructure { id } => {
                crate::recover_infrastructure(&cx.ctx, &id)?;
                cx.out.line(format_args!(
                    "{id} returned to Review; autopilot will resume it on sync"
                ));
            }
            Self::Other => return Err(super::unknown_command("other")),
        }
        Ok(())
    }

    fn touches_runtime(&self) -> bool {
        matches!(
            self,
            Self::Comment { send: true, .. }
                | Self::Hold { .. }
                | Self::Release { .. }
                | Self::Focus { .. }
        )
    }
}

fn explain(cx: &mut Exec<'_>, id: &str, choice: &ProfileChoice, json: bool) -> Result<()> {
    let explanation = crate::route_with_plan(&cx.ctx, id, choice, false)?;
    if json {
        cx.out.json_pretty(&explanation)?;
    } else {
        let task = cx.ctx.store.get(id)?;
        let size = task
            .size
            .map(|v| v.to_string())
            .unwrap_or_else(|| "not assessed".into());
        let difficulty = task
            .difficulty
            .map(|v| v.to_string())
            .unwrap_or_else(|| "not assessed".into());
        let legacy = if task.difficulty.is_none() {
            "; legacy routing applies"
        } else {
            ""
        };
        cx.out.line(format_args!(
            "Task: {size} size, {difficulty} difficulty{legacy}"
        ));
        let decision = &explanation.implementer;
        cx.out.line(format_args!("profile: {}", decision.profile));
        cx.out.line(format_args!("harness: {}", decision.harness));
        cx.out.line(format_args!(
            "model:   {}",
            decision.model.as_deref().unwrap_or("default")
        ));
        cx.out.line(format_args!(
            "effort:  {}",
            decision.effort.as_deref().unwrap_or("default")
        ));
        cx.out.line(format_args!("source:  {}", decision.route));
        cx.out.line(format_args!(
            "cross-family review: {} ({})",
            if explanation.require_cross_family {
                "required"
            } else {
                "not required"
            },
            explanation.review_policy_source
        ));
        for warning in &decision.warnings {
            cx.out.line(format_args!("warning: {warning}"));
        }
        print_reviewer(cx, "code reviewer", &explanation.review.reviewer);
        if let Some(planner) = &explanation.review.planner {
            cx.out.line(format_args!("\nplanner:"));
            cx.out.line(format_args!("profile: {}", planner.profile));
            cx.out.line(format_args!("harness: {}", planner.harness));
            cx.out.line(format_args!(
                "model:   {}",
                planner.model.as_deref().unwrap_or("default")
            ));
            cx.out.line(format_args!(
                "vendor:  {}",
                planner.vendor.as_deref().unwrap_or("unknown")
            ));
            cx.out.line(format_args!("source:  {}", planner.source));
            for warning in &planner.warnings {
                cx.out.line(format_args!("warning: {warning}"));
            }
        }
        if let Some(reviewer) = &explanation.review.plan_reviewer {
            print_reviewer(cx, "plan reviewer", reviewer);
        }
    }
    Ok(())
}

fn print_reviewer(cx: &mut Exec<'_>, label: &str, mapping: &crate::ReviewMapping) {
    cx.out.line(format_args!("\n{label}:"));
    cx.out.line(format_args!("reviewer: {}", mapping.selection));
    cx.out.line(format_args!(
        "profile: {}",
        mapping.profile.as_deref().unwrap_or("none")
    ));
    cx.out.line(format_args!(
        "harness: {}",
        mapping.harness.as_deref().unwrap_or("none")
    ));
    cx.out.line(format_args!(
        "model:   {}",
        mapping.model.as_deref().unwrap_or("default")
    ));
    cx.out.line(format_args!(
        "vendor:  {}",
        mapping.vendor.as_deref().unwrap_or("unknown")
    ));
    cx.out.line(format_args!(
        "source:  {} ({})",
        mapping.source, mapping.policy_source
    ));
    if let Some(rule) = &mapping.rule {
        cx.out.line(format_args!("rule:    {rule}"));
    }
    for warning in &mapping.warnings {
        cx.out.line(format_args!("warning: {warning}"));
    }
}
