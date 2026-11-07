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
    pub context: Preview,
    pub code_preserved: bool,
    pub excluded_work: Vec<String>,
}

impl Package {
    pub(super) fn context_preview(&self, store: &Store) -> Result<Preview> {
        let context: Context =
            serde_json::from_slice(&self.context.load(store)?).context("decode handoff context")?;
        let mut preview = context.preview()?;
        let plan = self.plan.as_ref().map_or_else(
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
        let code = self.snapshot.as_ref().map_or_else(
            || "no code checkpoint".into(),
            |s| {
                format!(
                    "base {} HEAD {} branch {} at {}; index {}; {} work entries; bundle {}",
                    s.state.base,
                    s.state.head,
                    s.state.branch,
                    s.state.branch_tip,
                    s.state.index.blob.hash(),
                    s.state.files.len(),
                    s.bundle.blob.hash()
                )
            },
        );
        let facts = format!(
            "Source: {} / {} state {} epoch {}\nProject/scope: {} {:?}\nSelected goal/stop: {:?} (not execution consent)\nPlan: {plan}\nPreserved work: {code}\nReview/findings artifacts: {}; execution/active-operation checkpoint: {:?}. These references are never replayed; reconcile their current state before proceeding.\n",
            self.assistant,
            self.expected.workflow,
            self.expected.state_revision,
            self.expected.authority_epoch,
            self.project,
            self.scope,
            self.goal,
            self.reviews.len(),
            self.execution
        );
        // Reserve space rather than dropping operational facts or claiming all history loaded.
        ensure!(
            facts.len() + preview.prompt.len() <= context::PROMPT_MAX + 8192,
            "context_oversized: operational identity exceeds continuation budget"
        );
        preview.prompt = format!("{facts}\n{}", preview.prompt);
        Ok(preview)
    }
}

pub fn preview_package(store: &Store, id: &str) -> Result<PackagePreview> {
    let (journal, package, context) = super::read_package(store, id)?;
    Ok(PackagePreview {
        package: journal.package,
        source_assistant: package.assistant,
        source_workflow: package.expected.workflow,
        source_revision: package.expected.state_revision,
        goal: package.goal,
        context,
        code_preserved: package.snapshot.is_some(),
        excluded_work: package.snapshot.map_or_else(Vec::new, |s| s.excluded),
    })
}
