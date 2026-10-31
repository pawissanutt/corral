//! Finite policy selection and explicit terminal-owner confirmation.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, IsTerminal, Write};

use anyhow::ensure;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_core::{Config, OperatingAuthority, OperatingPreview, OperatingSelection};
use corral_ops::Ctx;

#[derive(Args)]
pub struct AutomationArgs {
    #[command(subcommand)]
    pub(crate) cmd: AutomationCmd,
}

#[derive(Subcommand)]
pub(crate) enum AutomationCmd {
    /// Show the exact finite policy, constraints and confirmation digest without granting authority
    Preview(SelectionArgs),
    /// Recheck the same selection and require the owner to type the full digest at the terminal
    Confirm {
        #[command(flatten)]
        selection: SelectionArgs,
        #[arg(long)]
        digest: String,
    },
    /// Explain effective automation and why it may be inert
    Status,
}

#[derive(Args)]
pub(crate) struct SelectionArgs {
    /// Snapshot current Ready/Backlog tasks in these projects (with --include-new: all unfinished and future tasks)
    #[arg(long = "project", value_delimiter = ',')]
    projects: Vec<String>,
    /// Also cover tasks filed later in the --project projects; the grant then expires
    #[arg(long)]
    include_new: bool,
    /// Days after confirmation when a grant covering new tasks ends (default 7, max 365)
    #[arg(long, requires = "include_new")]
    expires_after_days: Option<u32>,
    /// Most tasks filed after confirmation that the grant covers
    #[arg(long, requires = "include_new")]
    max_new_tasks: Option<u32>,
    #[arg(long = "task", value_delimiter = ',')]
    tasks: Vec<String>,
    #[arg(long = "exclude", value_delimiter = ',')]
    exclusions: Vec<String>,
    /// Exact reviewed plan scope: TASK:LABEL (repeatable)
    #[arg(long = "plan-scope")]
    plan_scopes: Vec<String>,
    /// Delegate dispatch (also required for automatic gate/review/repair)
    #[arg(long)]
    dispatch: bool,
    /// Delegate landing subject to existing owner exceptions and verification
    #[arg(long)]
    accept: bool,
    #[arg(long)]
    plan_accept: bool,
    /// Delegate final tracker Plan acceptance; still requires an independent aggregate review
    #[arg(long)]
    aggregate_accept: bool,
}

impl SelectionArgs {
    pub(crate) fn resolve(self) -> Result<OperatingSelection> {
        let mut plan_scopes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for scope in self.plan_scopes {
            let (task, label) = scope
                .split_once(':')
                .context("--plan-scope needs TASK:LABEL")?;
            ensure!(!label.is_empty(), "plan scope label must not be empty");
            plan_scopes
                .entry(corral_core::Store::normalize_id(task)?)
                .or_default()
                .insert(label.to_owned());
        }
        Ok(OperatingSelection {
            projects: self.projects.into_iter().collect(),
            tasks: self.tasks.into_iter().collect(),
            exclusions: self.exclusions.into_iter().collect(),
            plan_scopes,
            authority: OperatingAuthority {
                dispatch: self.dispatch,
                accept: self.accept,
                plan_accept: self.plan_accept,
                aggregate_accept: self.aggregate_accept,
            },
            include_new: self.include_new,
            expires_after_days: self.expires_after_days,
            max_new_tasks: self.max_new_tasks,
        })
    }
}

pub fn run(ctx: &Ctx, args: AutomationArgs) -> Result<()> {
    match args.cmd {
        AutomationCmd::Status => {
            crate::pm_cmd::evaluate(ctx, |snapshot| crate::mode_cmd::status(snapshot, false))?;
        }
        AutomationCmd::Preview(selection) => {
            let preview = corral_ops::automation_preview(&ctx.store, &selection.resolve()?)?;
            println!("{}", serde_json::to_string_pretty(&preview)?);
            println!("Exact confirmation digest: {}", preview.id);
            println!("Stored grant digest: {}", preview.grant_digest);
        }
        AutomationCmd::Confirm { selection, digest } => {
            corral_ops::require_automation_owner(&ctx.store)?;
            let preview = corral_ops::automation_preview(&ctx.store, &selection.resolve()?)?;
            ensure!(
                digest == preview.id,
                "preview changed; run automation preview again"
            );
            crate::mode_cmd::print_preview(&preview)?;
            confirm_terminal(ctx, &preview)?;
        }
    }
    Ok(())
}

pub(crate) fn confirm_terminal(ctx: &Ctx, preview: &OperatingPreview) -> Result<Option<Config>> {
    corral_ops::require_automation_owner(&ctx.store)?;
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "owner confirmation requires an interactive terminal; use mode autopilot --confirm <preview-id> for explicit nonterminal confirmation"
    );
    print!(
        "Type the full digest {} to grant this exact policy (Enter cancels): ",
        preview.id
    );
    std::io::stdout()
        .flush()
        .context("displaying automation confirmation")?;
    let mut input = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut input)
        .context("reading owner confirmation")?;
    let digest = input.trim_end_matches(['\r', '\n']);
    if digest.is_empty() {
        println!("Cancelled; no grant written.");
        return Ok(None);
    }
    let config = corral_ops::automation_confirm(&ctx.store, preview, digest)?;
    println!("Confirmed {}", preview.id);
    Ok(Some(config))
}
