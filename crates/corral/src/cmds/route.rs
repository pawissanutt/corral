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
    let decision = corral_ops::route(ctx, id, choice)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&decision)?);
    } else {
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
        for warning in &decision.warnings {
            println!("warning: {warning}");
        }
    }
    Ok(())
}

pub(super) fn dispatch(
    ctx: &Ctx,
    id: &str,
    opts: DispatchOpts,
    explain_only: bool,
    json: bool,
) -> Result<()> {
    if explain_only {
        explain(
            ctx,
            id,
            &ProfileChoice {
                profile: opts.profile,
                model: opts.model,
                effort: opts.effort,
            },
            json,
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
    print_outcome(corral_ops::dispatch(ctx, id, &opts)?);
    Ok(())
}
