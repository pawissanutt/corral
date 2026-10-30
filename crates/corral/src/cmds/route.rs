use anyhow::Result;
use corral_core::{ProfileChoice, Run, RunMode};
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};

use crate::ProfileArgs;

impl ProfileArgs {
    pub(super) fn choice(self) -> ProfileChoice {
        ProfileChoice {
            profile: self.profile,
            model: self.model,
            effort: self.effort,
        }
    }

    pub(crate) fn dispatch_opts(
        self,
        headless: bool,
        base: Option<String>,
        queue: bool,
    ) -> DispatchOpts {
        let choice = self.choice();
        DispatchOpts {
            plan: false,
            profile: choice.profile,
            model: choice.model,
            effort: choice.effort,
            mode: if headless {
                RunMode::Headless
            } else {
                RunMode::Interactive
            },
            base,
            queue_if_blocked: queue,
            force_resources: false,
        }
    }
}

/// "profile omo, harness omo, model opus, effort high" (absent parts omitted).
pub(crate) fn run_profile(r: &Run) -> String {
    let mut parts = vec![format!("profile {}", r.agent)];
    for (label, value) in [
        ("harness", &r.harness),
        ("model", &r.model),
        ("effort", &r.effort),
    ] {
        if let Some(v) = value {
            parts.push(format!("{label} {v}"));
        }
    }
    parts.join(", ")
}

pub(super) fn print_outcome(o: DispatchOutcome) {
    match o {
        DispatchOutcome::Started {
            task,
            run_id,
            pane_id,
        } => {
            println!("{} started as {run_id} in pane {pane_id}", task.id)
        }
        DispatchOutcome::Queued { task, reason } => println!("{} queued: {reason}", task.id),
    }
}

pub(super) fn retry(ctx: &Ctx, id: &str, run: Option<&str>, choice: &ProfileChoice) -> Result<()> {
    let outcome = match run {
        Some(run) => corral_ops::retry_run(ctx, id, run, choice)?,
        None => corral_ops::retry(ctx, id, choice)?,
    };
    print_outcome(outcome);
    Ok(())
}

pub(super) fn explain(ctx: &Ctx, id: &str, choice: &ProfileChoice, json: bool) -> Result<()> {
    explain_plan(ctx, id, choice, json, false)
}

fn explain_plan(ctx: &Ctx, id: &str, choice: &ProfileChoice, json: bool, plan: bool) -> Result<()> {
    let explanation = corral_ops::route_with_plan(ctx, id, choice, plan)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&explanation)?);
    } else {
        let task = ctx.store.get(id)?;
        let size = task
            .size
            .map(|size| size.to_string())
            .unwrap_or_else(|| "not assessed".into());
        let difficulty = task
            .difficulty
            .map(|difficulty| difficulty.to_string())
            .unwrap_or_else(|| "not assessed".into());
        let legacy = if task.difficulty.is_none() {
            "; legacy routing applies"
        } else {
            ""
        };
        println!("Task: {size} size, {difficulty} difficulty{legacy}");
        let decision = &explanation.implementer;
        println!("profile: {}", decision.profile);
        println!("harness: {}", decision.harness);
        println!(
            "model:   {}",
            decision.model.as_deref().unwrap_or("default")
        );
        println!(
            "effort:  {}",
            decision.effort.as_deref().unwrap_or("default")
        );
        println!("source:  {}", decision.route);
        println!(
            "cross-family review: {} ({})",
            if explanation.require_cross_family {
                "required"
            } else {
                "not required"
            },
            explanation.review_policy_source
        );
        for warning in &decision.warnings {
            println!("warning: {warning}");
        }
        print_reviewer("code reviewer", &explanation.review.reviewer);
        if let Some(planner) = &explanation.review.planner {
            println!("\nplanner:");
            println!("profile: {}", planner.profile);
            println!("harness: {}", planner.harness);
            println!("model:   {}", planner.model.as_deref().unwrap_or("default"));
            println!(
                "vendor:  {}",
                planner.vendor.as_deref().unwrap_or("unknown")
            );
            println!("source:  {}", planner.source);
            for warning in &planner.warnings {
                println!("warning: {warning}");
            }
        }
        if let Some(reviewer) = &explanation.review.plan_reviewer {
            print_reviewer("plan reviewer", reviewer);
        }
    }
    Ok(())
}

fn print_reviewer(label: &str, mapping: &corral_ops::ReviewMapping) {
    println!("\n{label}:");
    println!("reviewer: {}", mapping.selection);
    println!("profile: {}", mapping.profile.as_deref().unwrap_or("none"));
    println!("harness: {}", mapping.harness.as_deref().unwrap_or("none"));
    println!("model:   {}", mapping.model.as_deref().unwrap_or("default"));
    println!(
        "vendor:  {}",
        mapping.vendor.as_deref().unwrap_or("unknown")
    );
    println!("source:  {} ({})", mapping.source, mapping.policy_source);
    if let Some(rule) = &mapping.rule {
        println!("rule:    {rule}");
    }
    for warning in &mapping.warnings {
        println!("warning: {warning}");
    }
}

pub(super) fn dispatch(
    ctx: &Ctx,
    id: &str,
    opts: DispatchOpts,
    explain_only: bool,
    json: bool,
) -> Result<()> {
    if explain_only {
        explain_plan(
            ctx,
            id,
            &ProfileChoice {
                profile: opts.profile,
                model: opts.model,
                effort: opts.effort,
            },
            json,
            opts.plan,
        )?;
        if !json
            && let Some(entry) = corral_ops::dispatch_queue(ctx)?
                .into_iter()
                .find(|e| e.id == id)
        {
            println!("queue:   {}", entry.reason);
        }
        return Ok(());
    }
    if opts.queue_if_blocked {
        let task = corral_ops::enqueue(ctx, id, &opts)?;
        println!("{} Ready (waiting for automatic admission)", task.id);
    } else {
        print_outcome(corral_ops::dispatch(ctx, id, &opts)?);
    }
    Ok(())
}
