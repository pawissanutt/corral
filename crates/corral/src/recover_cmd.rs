//! Read-only recovery proposals; only the shared local-owner ceremony grants authority.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::{Args, ValueEnum};
use corral_core::{
    ProfileChoice, RecoveryAction, RecoveryAdmissionStage, RecoveryCheck, RecoveryIdentity,
    RecoveryMerge, RecoveryPreview, RecoveryReviewStage, RecoveryReviewer, RecoveryState, Store,
};
use corral_ops::{Ctx, recovery};

#[derive(Clone, Copy, ValueEnum)]
enum Action {
    UseReviewer,
    AcceptPlan,
    AcceptCode,
    ForceAdmission,
}

#[derive(Clone, Copy, ValueEnum)]
enum Stage {
    Dispatch,
    PlanDispatch,
    Gate,
    CodeReview,
    PlanReview,
}

impl Stage {
    fn admission(self) -> RecoveryAdmissionStage {
        match self {
            Self::Dispatch => RecoveryAdmissionStage::Dispatch,
            Self::PlanDispatch => RecoveryAdmissionStage::PlanDispatch,
            Self::Gate => RecoveryAdmissionStage::Gate,
            Self::CodeReview => RecoveryAdmissionStage::CodeReview,
            Self::PlanReview => RecoveryAdmissionStage::PlanReview,
        }
    }
}

fn check(value: &str) -> std::result::Result<RecoveryCheck, String> {
    serde_json::from_value(serde_json::Value::String(value.into())).map_err(|_| {
        format!("unknown recovery check '{value}'; no wildcard exceptions are allowed")
    })
}

#[derive(Args)]
pub struct RecoverArgs {
    id: String,
    /// Exact proposed operation; without --apply this only previews
    #[arg(long, value_enum, required_unless_present = "history")]
    action: Option<Action>,
    /// One named launch stage (use-reviewer or force-admission only)
    #[arg(long, value_enum)]
    stage: Option<Stage>,
    /// Refuse unless this is still the current plan revision
    #[arg(long)]
    plan: Option<String>,
    /// Refuse unless this is still the current code run
    #[arg(long)]
    run: Option<String>,
    /// One-attempt reviewer; prefer ordinary route --set-reviewer when sufficient
    #[arg(long)]
    reviewer: Option<String>,
    /// Code acceptance strategy (accept-code requires an explicit choice)
    #[arg(long, value_parser = ["keep", "rebase", "ff"])]
    merge: Option<String>,
    /// Name every exact overridden/waived check; repeatable, no wildcard
    #[arg(long = "waive", value_parser = check, value_delimiter = ',')]
    exceptions: Vec<RecoveryCheck>,
    /// Plan acceptance resolutions; binds the actual file bytes into the preview
    #[arg(long)]
    notes_file: Option<PathBuf>,
    /// Read durable operations, not preview proposals
    #[arg(long, conflicts_with_all = ["action", "stage", "plan", "run", "reviewer", "merge", "exceptions", "notes_file", "apply", "reason"])]
    history: bool,
    /// Exact current preview digest; not permission and never a yes/force flag
    #[arg(long, requires = "reason")]
    apply: Option<String>,
    /// Nonblank owner-written reason; application still requires local terminal confirmation
    #[arg(long, requires = "apply")]
    reason: Option<String>,
    #[arg(long)]
    json: bool,
}

impl RecoverArgs {
    fn input(&self, ctx: &Ctx) -> Result<recovery::RecoveryInput> {
        let action = self.action.context("choose --action or --history")?;
        ensure!(
            matches!(action, Action::UseReviewer | Action::ForceAdmission) == self.stage.is_some(),
            "--stage is required only for use-reviewer and force-admission"
        );
        ensure!(
            matches!(action, Action::UseReviewer) == self.reviewer.is_some(),
            "--reviewer is required only for use-reviewer"
        );
        ensure!(
            matches!(action, Action::AcceptCode) == self.merge.is_some(),
            "--merge is required only for accept-code"
        );
        ensure!(
            matches!(action, Action::AcceptPlan) == self.notes_file.is_some(),
            "--notes-file is required only for accept-plan"
        );
        let notes = self
            .notes_file
            .as_ref()
            .map(|path| {
                std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
            })
            .transpose()?
            .unwrap_or_default();
        let exceptions: BTreeSet<_> = self.exceptions.iter().copied().collect();
        let action = match action {
            Action::UseReviewer => {
                let stage = match self.stage {
                    Some(Stage::PlanReview) => RecoveryReviewStage::PlanReview,
                    Some(Stage::CodeReview) => RecoveryReviewStage::CodeReview,
                    _ => anyhow::bail!("use-reviewer needs --stage plan-review or code-review"),
                };
                let resolved = ctx.store.config.resolve_profile(
                    &ProfileChoice {
                        profile: self.reviewer.clone(),
                        ..Default::default()
                    },
                    None,
                    None,
                )?;
                RecoveryAction::UseReviewer {
                    stage,
                    reviewer: RecoveryReviewer {
                        profile: resolved.profile,
                        model: resolved
                            .model
                            .context("reviewer effective model is unknown")?,
                        vendor: resolved.vendor.context("reviewer vendor is unknown")?,
                    },
                }
            }
            Action::AcceptPlan => RecoveryAction::AcceptPlan {
                waive_independent_review: exceptions
                    .contains(&RecoveryCheck::IndependentPlanReview),
                resolution_notes_sha256: corral_core::agent::paths::sha256_hex(notes.as_bytes()),
            },
            Action::AcceptCode => RecoveryAction::AcceptCode {
                merge: match self.merge.as_deref() {
                    Some("keep") => RecoveryMerge::Keep,
                    Some("rebase") => RecoveryMerge::Rebase,
                    Some("ff") => RecoveryMerge::Ff,
                    _ => anyhow::bail!("accept-code needs --merge keep, rebase or ff"),
                },
            },
            Action::ForceAdmission => {
                // The helper supplies current pressure identity, never caller-selected episode data.
                return Ok(recovery::RecoveryInput {
                    exceptions,
                    ..recovery::recover_resource_input(
                        ctx,
                        self.stage.context("missing stage")?.admission(),
                    )?
                });
            }
        };
        Ok(recovery::RecoveryInput {
            action,
            exceptions,
            notes,
        })
    }

    fn target(&self, preview: &RecoveryPreview) -> Result<()> {
        let (plan, run) = match &preview.request.target.identity {
            RecoveryIdentity::Plan { plan, .. } => (Some(plan.as_str()), None),
            RecoveryIdentity::Code { candidate, .. } => (None, Some(candidate.run.as_str())),
            RecoveryIdentity::Admission { plan, run, .. } => (plan.as_deref(), run.as_deref()),
        };
        ensure!(
            self.plan.as_deref().is_none_or(|p| Some(p) == plan),
            "recovery_stale: --plan does not match the current target"
        );
        ensure!(
            self.run.as_deref().is_none_or(|r| Some(r) == run),
            "recovery_stale: --run does not match the current target"
        );
        Ok(())
    }
}

pub fn run(start: &Path, args: RecoverArgs) -> Result<()> {
    // Do not use Entry::open: even an ordinary observed open can append configuration events.
    let ctx = Ctx::from_store(Store::open_unobserved(&Store::find_root(start)?)?);
    if args.history {
        let history = recovery::recover_history(&ctx, &args.id)?;
        if !args.json {
            println!(
                "{}: owner-recovery history (original results retained)",
                args.id
            );
        }
        println!("{}", serde_json::to_string_pretty(&history)?);
        return Ok(());
    }
    let input = args.input(&ctx)?;
    let preview = recovery::recover_preview(&ctx, &args.id, &input)?;
    args.target(&preview)?;
    if let Some(digest) = &args.apply {
        ensure!(
            *digest == preview.digest,
            "recovery_stale: preview changed; inspect a fresh preview and confirm again"
        );
        let confirmation = recovery::recover_confirm(
            &ctx,
            &preview,
            &input,
            args.reason.as_deref().unwrap_or_default(),
        )?;
        let record = recovery::recover_apply(&ctx, &preview, &input, Some(confirmation))?;
        if !args.json {
            println!("Owner recovery {} (not ordinary policy success)", record.id);
        }
        println!("{}", serde_json::to_string_pretty(&record)?);
        ensure!(
            record
                .outcomes
                .last()
                .is_some_and(|o| o.state == RecoveryState::Applied),
            "recovery did not complete; inspect --history and reconcile the existing operation; do not blindly replay"
        );
    } else {
        if !args.json {
            println!("Read-only recovery preview; no authorization or state change.");
            println!(
                "Prefer ordinary route --set-reviewer when sufficient. Remaining blockers prevent application."
            );
        }
        println!("{}", serde_json::to_string_pretty(&preview)?);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/recovery_cli/input.rs"]
mod tests;
