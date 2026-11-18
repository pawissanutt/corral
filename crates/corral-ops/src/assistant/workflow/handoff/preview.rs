use super::{
    Content, Goal, Package,
    context::{self, Context, Preview},
};
use anyhow::{Context as _, Result, ensure};
use corral_core::Store;
use serde::{Deserialize, Serialize};

/// Read projection, not authority. An actual continuation/role is allocated by C3, never
/// cloned from a live process. Inspect the package for every archived evidence reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePreview {
    pub package: Content,
    pub source_assistant: String,
    pub source_workflow: String,
    pub source_revision: u64,
    pub goal: Goal,
    pub plan: Option<corral_core::assistant_workflow::PlanRevision>,
    pub local_reviews: Vec<corral_core::assistant_workflow::DesignArtifact>,
    /// None = unknown, never fresh/approved by implication. T-753's local-review lifecycle
    /// has not landed; opaque historical review artifacts have no trusted revision binding.
    pub local_review_freshness: Option<bool>,
    pub context: Preview,
    pub code_preserved: bool,
    pub excluded_work: Vec<String>,
}

/// The operational identity every continuation prompt starts with. One projection for the
/// committed package and the read-only native preview, so the two cannot drift.
pub(super) struct Facts<'a> {
    pub assistant: &'a str,
    pub expected: &'a super::super::Expected,
    pub project: &'a str,
    pub scope: &'a [String],
    pub goal: Goal,
    pub plan: Option<&'a corral_core::assistant_workflow::PlanRevision>,
    /// The preserved work line; see [`code_line`].
    pub code: String,
    pub reviews: usize,
    pub execution: Option<&'a corral_core::assistant_workflow::DesignArtifact>,
}

/// A code checkpoint's identity. `bundle` is `None` before capture (native preview).
pub(super) fn code_line(state: &super::git::WorkState, bundle: Option<&str>) -> String {
    format!(
        "base {} HEAD {} branch {} at {}; index {}; {} work entries; bundle {}",
        state.base,
        state.head,
        state.branch,
        state.branch_tip,
        state.index.blob.hash(),
        state.files.len(),
        bundle.unwrap_or("captured at selection")
    )
}

impl Facts<'_> {
    pub(super) fn render(&self) -> String {
        let plan = self.plan.map_or_else(
            || "absent / incomplete; not approved".into(),
            |p| {
                format!(
                    "r{} {} {:?}; review freshness must be checked, no approval imported",
                    p.revision,
                    p.artifact.blob.hash(),
                    p.stage
                )
            },
        );
        format!(
            "Source: {} / {} state {} epoch {}\nProject/scope: {} {:?}\nSelected goal/stop: {:?} (not execution consent)\nPlan: {plan}\nPreserved work: {}\nReview/findings artifacts: {}; execution/active-operation checkpoint: {:?}. These references are never replayed; reconcile their current state before proceeding.\n",
            self.assistant,
            self.expected.workflow,
            self.expected.state_revision,
            self.expected.authority_epoch,
            self.project,
            self.scope,
            self.goal,
            self.code,
            self.reviews,
            self.execution
        )
    }
}

/// Prefix the operational facts, reserving budget rather than dropping them.
pub(super) fn with_facts(mut preview: Preview, facts: &str) -> Result<Preview> {
    ensure!(
        facts.len() + preview.prompt.len() <= context::PROMPT_MAX + 8192,
        "context_oversized: operational identity exceeds continuation budget"
    );
    preview.prompt = format!("{facts}\n{}", preview.prompt);
    Ok(preview)
}

impl Package {
    pub(super) fn facts(&self) -> String {
        Facts {
            assistant: &self.assistant,
            expected: &self.expected,
            project: &self.project,
            scope: &self.scope,
            goal: self.goal,
            plan: self.plan.as_ref(),
            code: self.snapshot.as_ref().map_or_else(
                || "no code checkpoint".into(),
                |s| code_line(&s.state, Some(s.bundle.blob.hash().as_str())),
            ),
            reviews: self.reviews.len(),
            execution: self.execution.as_ref(),
        }
        .render()
    }

    pub(super) fn context_preview(&self, store: &Store) -> Result<Preview> {
        let context: Context =
            serde_json::from_slice(&self.context.load(store)?).context("decode handoff context")?;
        with_facts(context.preview()?, &self.facts())
    }
}

pub fn preview_package(store: &Store, id: &str) -> Result<PackagePreview> {
    let (publication, package, context) = super::read_package(store, id)?;
    Ok(PackagePreview {
        package: publication.package,
        source_assistant: package.assistant,
        source_workflow: package.expected.workflow,
        source_revision: package.expected.state_revision,
        goal: package.goal,
        plan: package.plan,
        local_reviews: package.reviews,
        local_review_freshness: None,
        context,
        code_preserved: package.snapshot.is_some(),
        excluded_work: package.snapshot.map_or_else(Vec::new, |s| s.excluded),
    })
}
