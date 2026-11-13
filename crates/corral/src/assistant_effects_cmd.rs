//! Thin typed adapter. Preview/lookup are read-only; apply has no noninteractive bypass.
use anyhow::{Result, ensure};
use clap::{Args, Subcommand};
use corral_ops::assistant::workflow::effect_recovery as recovery;

#[derive(Args)]
pub struct EffectsArgs {
    #[command(subcommand)]
    pub cmd: EffectsCmd,
}
#[derive(Args)]
pub struct Target {
    /// Exact assistant whose original executors are being inspected
    pub assistant: String,
    /// Original, fenced host generation (not the current host's generation)
    #[arg(long)]
    pub generation: u32,
    /// Explicit matching associated task; omission requires a complete no-associated-task proof
    #[arg(long)]
    pub task: Option<String>,
    /// Exact full admission ID; repeat to select a proved subset
    #[arg(long, required = true)]
    pub admission: Vec<String>,
    /// Stable reconciliation request ID; use receipt lookup after an uncertain acknowledgement
    #[arg(long)]
    pub request: String,
}
impl Target {
    fn request(self) -> recovery::Request {
        recovery::Request {
            assistant: self.assistant,
            task: self.task,
            generation: self.generation,
            admissions: self.admission,
            request_id: self.request,
        }
    }
}
#[derive(Subcommand)]
pub enum EffectsCmd {
    /// Read-only exact proposal, executor evidence and retained blockers (JSON); never drain
    Preview(Target),
    /// Reconcile only proved rows after an unmanaged owner types the exact terminal challenge
    Apply {
        #[command(flatten)]
        target: Target,
        /// Digest from the exact inspected preview; changes require a fresh preview
        #[arg(long)]
        digest: String,
        /// Nonblank owner explanation retained with the immutable audit
        #[arg(long)]
        reason: String,
    },
    /// Read a saved request outcome; replay is acknowledgement, not renewed authority
    Receipt { assistant: String, request: String },
    /// Read the immutable reconciliation/audit for one original admission
    Outcome {
        assistant: String,
        #[arg(long)]
        generation: u32,
        #[arg(long)]
        admission: String,
    },
}
pub fn run(inv: &crate::invocation::Invocation, args: EffectsArgs) -> Result<()> {
    let ctx = inv.ctx()?;
    let value = match args.cmd {
        EffectsCmd::Preview(target) => {
            serde_json::to_value(recovery::preview(&ctx.store, &target.request())?)?
        }
        EffectsCmd::Apply {
            target,
            digest,
            reason,
        } => {
            let proposal = recovery::preview(&ctx.store, &target.request())?;
            ensure!(
                proposal.digest == digest,
                "stale effect recovery digest; read a fresh preview"
            );
            let confirmation = recovery::confirm(&ctx.store, &proposal, &reason)?;
            let (receipt, replayed) = recovery::apply(&ctx.store, confirmation)?;
            serde_json::json!({"receipt":receipt,"replayed":replayed,"authority":"record-only; not handoff_drain"})
        }
        EffectsCmd::Receipt { assistant, request } => {
            serde_json::json!({"receipt":recovery::lookup(&ctx.store, &assistant, &request)?,
            "authority":"acknowledgement only; not current handoff authority"})
        }
        EffectsCmd::Outcome {
            assistant,
            generation,
            admission,
        } => recovery::outcome(&ctx.store, &assistant, generation, &admission)?,
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
