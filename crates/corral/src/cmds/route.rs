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

    pub(super) fn dispatch_opts(
        self,
        headless: bool,
        base: Option<String>,
        queue: bool,
    ) -> DispatchOpts {
        let choice = self.choice();
        DispatchOpts {
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
        return explain(
            ctx,
            id,
            &ProfileChoice {
                profile: opts.profile,
                model: opts.model,
                effort: opts.effort,
            },
            json,
        );
    }
    print_outcome(corral_ops::dispatch(ctx, id, &opts)?);
    Ok(())
}
