//! Owner-only adapter to the shared C3 services. No native/model consent consumer.
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand, ValueEnum};
use corral_ops::{Ctx, assistant::workflow::handoff as h};
use serde::de::DeserializeOwned;
use std::path::PathBuf;

#[derive(Args)]
pub struct HandoffArgs {
    #[command(subcommand)]
    pub cmd: HandoffCmd,
}
#[derive(Subcommand)]
pub enum HandoffCmd {
    /// Register an ordinary source using its complete genuine current-generation input
    Init {
        session: String,
        #[arg(long)]
        input: String,
    },
    /// Freeze selected context/code; creates no task or execution authority
    Prepare {
        session: String,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        scratch: Option<String>,
    },
    /// Inspect an immutable package and the exact choice digest
    Preview {
        package: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        save: bool,
    },
    /// Commit exactly the choice the owner selected
    Commit {
        #[arg(long)]
        choice: PathBuf,
        #[arg(long)]
        digest: String,
    },
    /// Inspect current authoritative outcome and waits
    Show { package: String },
    /// Bind a new goal to current spec, plan, findings and code
    ContinuePreview {
        task: String,
        #[arg(long)]
        goal: GoalArg,
    },
    /// Start exactly a current-revision selected continuation
    Start {
        #[arg(long)]
        choice: PathBuf,
        #[arg(long)]
        digest: String,
    },
    /// Hold the current task scope; no old start/ship authority survives
    Save {
        task: String,
        #[arg(long)]
        transfer: String,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        contract: String,
    },
    /// Settle old writers and return latest work, prepare-only (not rollback)
    TakeBack {
        task: String,
        #[arg(long)]
        transfer: String,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        contract: String,
    },
}
#[derive(Clone, Copy, ValueEnum)]
pub enum GoalArg {
    PlanContinue,
    PlanReviewOnly,
    ImplementationPrepare,
    ImplementationVerify,
    ImplementationShip,
}
impl From<GoalArg> for h::Goal {
    fn from(value: GoalArg) -> Self {
        match value {
            GoalArg::PlanContinue => Self::PlanContinue,
            GoalArg::PlanReviewOnly => Self::PlanReviewOnly,
            GoalArg::ImplementationPrepare => Self::ImplementationPrepare,
            GoalArg::ImplementationVerify => Self::ImplementationVerify,
            GoalArg::ImplementationShip => Self::ImplementationShip,
        }
    }
}
fn load<T: DeserializeOwned>(path: &PathBuf) -> Result<T> {
    use std::io::Read as _;
    let file =
        corral_core::fs_guard::open_regular_nofollow(path).context("open selected handoff file")?;
    ensure!(
        file.metadata()?.len() <= 1024 * 1024,
        "handoff input exceeds 1 MiB"
    );
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .context("read bounded selected handoff file")?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "handoff input grew beyond 1 MiB"
    );
    serde_json::from_slice(&bytes).context("decode selected handoff file")
}
fn binding(ctx: &Ctx, task: &str, transfer: &str, revision: u64) -> Result<()> {
    let state = h::owner::read(ctx, transfer)?;
    ensure!(
        state["task"].as_str() == Some(corral_core::Store::normalize_id(task)?.as_str())
            && state["revision"].as_u64() == Some(revision),
        "handoff_stale_choice: current transfer/revision changed"
    );
    Ok(())
}
pub fn run(ctx: &Ctx, args: HandoffArgs) -> Result<()> {
    h::owner::require_entry(ctx)?;
    let value = match args.cmd {
        HandoffCmd::Init { session, input } => {
            serde_json::to_value(h::owner::initialize(ctx, &session, &input)?)?
        }
        HandoffCmd::Prepare {
            session,
            request,
            scratch,
        } => {
            let prepared = h::owner::prepare(ctx, &session, load(&request)?, scratch.as_deref())?;
            serde_json::json!({"package":prepared.id(),"preview":prepared.preview()})
        }
        HandoffCmd::Preview {
            package,
            title,
            into,
            save,
        } => {
            let choice = h::transfer::preview(ctx, &package, &title, into.as_deref(), save)?;
            serde_json::json!({"digest":choice.digest()?,"choice":choice,"source":h::preview_package(&ctx.store,&package)?})
        }
        HandoffCmd::Commit { choice, digest } => {
            serde_json::to_value(h::owner::commit(ctx, &load(&choice)?, &digest)?)?
        }
        HandoffCmd::Show { package } => h::owner::read(ctx, &package)?,
        HandoffCmd::ContinuePreview { task, goal } => {
            let choice = h::continuation::preview(ctx, &task, goal.into())?;
            serde_json::json!({"digest":choice.digest()?,"choice":choice})
        }
        HandoffCmd::Start { choice, digest } => {
            serde_json::to_value(h::owner::start(ctx, &load(&choice)?, &digest)?)?
        }
        HandoffCmd::Save {
            task,
            transfer,
            revision,
            contract,
        } => {
            binding(ctx, &task, &transfer, revision)?;
            serde_json::to_value(h::owner::save(ctx, &task, &contract)?)?
        }
        HandoffCmd::TakeBack {
            task,
            transfer,
            revision,
            contract,
        } => {
            binding(ctx, &task, &transfer, revision)?;
            serde_json::to_value(h::owner::take_back(ctx, &task, &contract)?)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
