use anyhow::{Result, bail};
use clap::Subcommand;
use corral_ops::Ctx;

#[derive(Subcommand)]
pub enum HooksCmd {
    /// Inspect landed status, readiness, commands, logs and retained policy receipts
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Reconcile interrupted or pending operations without retrying failed commands
    Reconcile,
    /// Explicit bounded retry of the first incomplete hook at an exact landed SHA
    Retry {
        project: String,
        sha: String,
        /// Acknowledge that an interrupted command may already have performed side effects
        #[arg(long)]
        acknowledge_uncertain: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: HooksCmd) -> Result<()> {
    match cmd {
        HooksCmd::List { project, json } => {
            let operations = corral_ops::post_land_operations(ctx, project.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&operations)?);
            } else {
                for op in operations {
                    println!(
                        "{} {} landed={} readiness={} policy={}",
                        op.project,
                        op.landed_sha,
                        op.landed,
                        if op.ready() {
                            "succeeded".into()
                        } else {
                            op.blocker().unwrap_or_else(|| "not landed".into())
                        },
                        op.policy_digest
                    );
                    for hook in op.hooks {
                        println!(
                            "  {} {:?} argv={:?}",
                            hook.hook.name, hook.state, hook.hook.argv
                        );
                        for attempt in hook.attempts {
                            println!(
                                "    attempt {} exit={:?} timeout={} log={}",
                                attempt.number,
                                attempt.exit_code,
                                attempt.timed_out,
                                attempt.log.display()
                            );
                        }
                    }
                }
            }
        }
        HooksCmd::Reconcile => corral_ops::reconcile_post_land(ctx)?,
        HooksCmd::Retry {
            project,
            sha,
            acknowledge_uncertain,
        } => {
            let op = corral_ops::retry_post_land(ctx, &project, &sha, acknowledge_uncertain)?;
            println!("{}", serde_json::to_string_pretty(&op)?);
            if let Some(reason) = op.blocker() {
                bail!("{reason}");
            }
        }
    }
    Ok(())
}
