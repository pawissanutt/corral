//! One workspace mode control; previews never confer authority.
use std::io::IsTerminal;

use anyhow::Result;
use clap::{Args, Subcommand};
use corral_core::OperatingPreview;
use corral_ops::{Ctx, command::Output};
use corral_protocol::cmd::{Command, mode::ModeCommand};

use crate::automation_cmd::SelectionArgs;
use crate::invocation::Invocation;

#[derive(Args)]
pub struct ModeArgs {
    #[command(subcommand)]
    pub(crate) cmd: ModeCmd,
}

#[derive(Subcommand)]
pub(crate) enum ModeCmd {
    /// Read effective mode, finite scope, authority, blockers and shared progress
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Withdraw automatic authority without stopping running work
    Assist,
    /// Preview a finite scope; grant only after exact owner confirmation
    Autopilot {
        #[command(flatten)]
        selection: SelectionArgs,
        /// Confirm the exact previously displayed preview identity (also noninteractively)
        #[arg(long)]
        confirm: Option<String>,
        /// Print the preview as JSON; without --confirm this never grants authority
        #[arg(long)]
        json: bool,
    },
    /// Stop new automatic actions without interrupting agents or receipt collection
    Pause,
    /// Resume the existing matching grant; never expands or refreshes its scope
    Resume,
    /// Land, accept plans and repair without owner stops or round limits until `--off`
    Turbo {
        #[arg(long)]
        off: bool,
    },
}

pub(crate) fn status(ctx: &Ctx, json: bool) -> Result<()> {
    let mut out = Output::default();
    let result = corral_ops::command::mode::status(ctx, json, &mut out);
    print!("{}", out.stdout);
    result
}

pub(crate) fn print_preview(preview: &OperatingPreview) -> Result<()> {
    let mut out = Output::default();
    let result = corral_ops::command::mode::print_preview(preview, &mut out);
    print!("{}", out.stdout);
    result
}

pub fn run(inv: &Invocation, args: ModeArgs) -> Result<()> {
    let (selection, json) = match args.cmd {
        ModeCmd::Status { json } => return status(inv.ctx()?, json),
        ModeCmd::Assist => return inv.submit(Command::Mode(ModeCommand::Assist {})),
        ModeCmd::Pause => return inv.submit(Command::Mode(ModeCommand::Pause {})),
        ModeCmd::Resume => return inv.submit(Command::Mode(ModeCommand::Resume {})),
        ModeCmd::Turbo { off } => return inv.submit(Command::Mode(ModeCommand::Turbo { off })),
        ModeCmd::Autopilot {
            selection,
            confirm: Some(confirm),
            json,
        } => {
            return inv.submit(Command::Mode(selection.into_command(confirm, json)));
        }
        ModeCmd::Autopilot {
            selection,
            confirm: None,
            json,
        } => (selection, json),
    };
    // Terminal confirmation replaces this handle's config, so keep an owned clone.
    let mut ctx = inv.ctx()?.clone();
    let invoker = inv.entry().invoker();
    let preview = corral_ops::automation_preview(&ctx.store, &selection.resolve()?)?;
    if json {
        let mut out = Output::default();
        out.json_pretty(&preview)?;
        print!("{}", out.stdout);
    } else {
        print_preview(&preview)?;
    }
    if json || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        if !json {
            println!(
                "No grant written. Review this preview, then repeat with --confirm {}",
                preview.id
            );
        }
        return Ok(());
    }
    let Some(config) = crate::automation_cmd::confirm_terminal(&ctx, invoker, &preview)? else {
        return Ok(());
    };
    ctx.store.config = config;
    status(&ctx, false)
}

pub(crate) fn legacy(inv: &Invocation, action: &str, json: bool) -> Result<()> {
    if action == "off" {
        return inv.submit(Command::Mode(ModeCommand::Off { json }));
    }
    let ctx = inv.ctx()?;
    if action == "on" && !json {
        println!(
            "No grant written. Use `corral mode autopilot --project <project> --dispatch` \
             to preview an exact finite scope and confirm it; review-only automation is no longer a control."
        );
    }
    status(ctx, json)
}
