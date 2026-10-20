//! Task-aware review requirements (PRF-08): which versioned contract, checks and reviewer
//! capability a review needs, and the route record saved with each review attempt.
//!
//! The requirement is a deterministic floor derived from machine fields (review subject,
//! design flag, task kind, labels, touched risk paths). Task size is recorded but never
//! lowers a requirement. Profile resolution and launch live in corral-ops.
use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{Config, ProfileRole, ReviewContract, ReviewContractKind, Reviewer, Task, TaskKind};

pub const REVIEW_ROUTE_VERSION: u32 = 1;

/// Every shipped contract text carries this version; bump a kind when its text changes meaning.
pub fn review_contract_version(kind: ReviewContractKind) -> u32 {
    match kind {
        ReviewContractKind::Plan
        | ReviewContractKind::Design
        | ReviewContractKind::Implementation
        | ReviewContractKind::ComplexImplementation
        | ReviewContractKind::VisualImplementation
        | ReviewContractKind::Prose
        | ReviewContractKind::TestOnly => 1,
    }
}

impl ReviewContractKind {
    pub const ALL: [ReviewContractKind; 7] = [
        Self::Plan,
        Self::Design,
        Self::Implementation,
        Self::ComplexImplementation,
        Self::VisualImplementation,
        Self::Prose,
        Self::TestOnly,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Design => "design",
            Self::Implementation => "implementation",
            Self::ComplexImplementation => "complex_implementation",
            Self::VisualImplementation => "visual_implementation",
            Self::Prose => "prose",
            Self::TestOnly => "test_only",
        }
    }

    /// Plan and design judgments use the plan-review role; everything else reviews a candidate.
    pub const fn role(self) -> ProfileRole {
        match self {
            Self::Plan | Self::Design => ProfileRole::PlanReview,
            _ => ProfileRole::ImplementationReview,
        }
    }

    pub const fn capability(self) -> ReviewCapability {
        match self {
            Self::Plan | Self::Design | Self::ComplexImplementation => ReviewCapability::Strong,
            _ => ReviewCapability::Routine,
        }
    }

    /// Mapping lookups never fall from a strong contract to a routine mapping.
    pub const fn fallbacks(self) -> &'static [ReviewContractKind] {
        match self {
            Self::Plan => &[],
            Self::Design => &[Self::Plan],
            Self::ComplexImplementation => &[Self::Design, Self::Plan],
            Self::VisualImplementation | Self::Prose | Self::TestOnly => &[Self::Implementation],
            Self::Implementation => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCapability {
    Routine,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSubjectKind {
    Plan,
    Implementation,
}

/// Machine identities of the checks a contract requires; the prose lives in versioned assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCheck {
    CompleteObject,
    CriterionEvidence,
    CandidateEvidence,
    FullDiff,
    RelevantCode,
    RepositoryReferences,
    Feasibility,
    OwnerDecisions,
    Alternatives,
    Invariants,
    VisualEvidence,
    SourceProvenance,
    DeterministicFix,
    RegressionCanFail,
    BehaviorUnchanged,
    MinimalScope,
    ExhaustiveClassSweep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRisk {
    Concurrency,
    Permissions,
    DataIntegrity,
    Security,
    Authority,
    Migration,
    HighConsequence,
    RiskPath,
    OwnerPath,
}

impl ReviewRisk {
    /// Task labels are proposer/PM machine fields, not worker prose.
    pub fn from_label(label: &str) -> Option<Self> {
        Some(match label.trim().to_ascii_lowercase().as_str() {
            "concurrency" | "race" => Self::Concurrency,
            "permissions" | "permission" => Self::Permissions,
            "data-integrity" | "persistence" => Self::DataIntegrity,
            "security" => Self::Security,
            "authority" | "policy" => Self::Authority,
            "migration" => Self::Migration,
            "high-consequence" | "risky" => Self::HighConsequence,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReasonCode {
    PlanSubject,
    DesignSubject,
    RiskFlag,
    VisualWork,
    ProseDeliverable,
    ResearchConclusions,
    TestOnly,
    Refactor,
    Hotfix,
    RoutineImplementation,
    SizeNotDowngrading,
    StrongerRequested,
    RoundScoped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewReason {
    pub code: ReviewReasonCode,
    pub detail: String,
}

/// Inputs to the deterministic floor. `risks` includes path-derived risks supplied by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSignals {
    pub subject: ReviewSubjectKind,
    pub design: bool,
    pub kind: Option<String>,
    pub size: Option<crate::TaskSize>,
    pub hotfix: bool,
    pub flaky: bool,
    pub risks: BTreeSet<ReviewRisk>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_paths: Vec<String>,
    pub stronger_requested: bool,
}

impl ReviewSignals {
    pub fn for_task(task: &Task, subject: ReviewSubjectKind) -> Self {
        let has = |name: &str| task.labels.iter().any(|l| l.eq_ignore_ascii_case(name));
        Self {
            subject,
            design: task.design,
            kind: task.kind.clone(),
            size: task.size,
            hotfix: has("hotfix"),
            flaky: has("flaky") || has("flaky-test"),
            risks: task
                .labels
                .iter()
                .filter_map(|l| ReviewRisk::from_label(l))
                .collect(),
            risk_paths: Vec::new(),
            stronger_requested: false,
        }
    }

    fn task_kind(&self) -> Option<TaskKind> {
        self.kind.as_deref().and_then(|k| k.parse().ok())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequirement {
    pub contract: ReviewContract,
    pub capability: ReviewCapability,
    pub checks: BTreeSet<ReviewCheck>,
    pub reasons: Vec<ReviewReason>,
}

fn reason(code: ReviewReasonCode, detail: impl Into<String>) -> ReviewReason {
    ReviewReason {
        code,
        detail: detail.into(),
    }
}

/// The floor: plan > risk > visual > prose/research > test-only > routine implementation.
pub fn review_requirement(signals: &ReviewSignals) -> ReviewRequirement {
    use ReviewCheck as C;
    use ReviewContractKind as K;
    let mut reasons = Vec::new();
    let mut checks: BTreeSet<ReviewCheck> = [C::CompleteObject, C::ExhaustiveClassSweep].into();
    let kind = signals.task_kind();
    let contract = if signals.subject == ReviewSubjectKind::Plan {
        checks.extend([
            C::RepositoryReferences,
            C::Feasibility,
            C::OwnerDecisions,
            C::CriterionEvidence,
        ]);
        if signals.design {
            checks.insert(C::Alternatives);
            reasons.push(reason(ReviewReasonCode::DesignSubject, "task is a design"));
            K::Design
        } else {
            reasons.push(reason(
                ReviewReasonCode::PlanSubject,
                "a plan is under review",
            ));
            K::Plan
        }
    } else {
        checks.extend([C::CriterionEvidence, C::CandidateEvidence, C::FullDiff]);
        let contract = if !signals.risks.is_empty() {
            let names: Vec<_> = signals.risks.iter().map(|r| format!("{r:?}")).collect();
            reasons.push(reason(ReviewReasonCode::RiskFlag, names.join(", ")));
            checks.extend([C::RelevantCode, C::Invariants]);
            if kind == Some(TaskKind::Ui) {
                checks.insert(C::VisualEvidence);
                reasons.push(reason(ReviewReasonCode::VisualWork, "UI change"));
            }
            K::ComplexImplementation
        } else if kind == Some(TaskKind::Ui) {
            checks.extend([C::RelevantCode, C::VisualEvidence]);
            reasons.push(reason(ReviewReasonCode::VisualWork, "UI change"));
            K::VisualImplementation
        } else if matches!(kind, Some(TaskKind::Docs | TaskKind::Research)) {
            checks.insert(C::SourceProvenance);
            reasons.push(if kind == Some(TaskKind::Research) {
                reason(
                    ReviewReasonCode::ResearchConclusions,
                    "research conclusions need design judgment",
                )
            } else {
                reason(
                    ReviewReasonCode::ProseDeliverable,
                    "documentation deliverable",
                )
            });
            K::Prose
        } else if kind == Some(TaskKind::Test) || signals.flaky {
            checks.extend([C::RelevantCode, C::DeterministicFix, C::RegressionCanFail]);
            reasons.push(reason(
                ReviewReasonCode::TestOnly,
                "test-only or flaky-test fix",
            ));
            K::TestOnly
        } else {
            checks.insert(C::RelevantCode);
            reasons.push(reason(
                ReviewReasonCode::RoutineImplementation,
                "implementation against its spec",
            ));
            K::Implementation
        };
        if kind == Some(TaskKind::Refactor) {
            checks.insert(C::BehaviorUnchanged);
            reasons.push(reason(
                ReviewReasonCode::Refactor,
                "behavior must be unchanged",
            ));
        }
        if signals.hotfix {
            checks.insert(C::MinimalScope);
            reasons.push(reason(
                ReviewReasonCode::Hotfix,
                "fix must stay minimal and scoped",
            ));
        }
        contract
    };
    let mut capability = contract.capability();
    if kind == Some(TaskKind::Research) {
        capability = ReviewCapability::Strong;
    }
    if signals.stronger_requested && capability == ReviewCapability::Routine {
        capability = ReviewCapability::Strong;
        reasons.push(reason(
            ReviewReasonCode::StrongerRequested,
            "previous routine attempt requested stronger review",
        ));
    }
    if capability == ReviewCapability::Strong
        && let Some(size) = signals.size
    {
        reasons.push(reason(
            ReviewReasonCode::SizeNotDowngrading,
            format!("size {size} does not lower the requirement"),
        ));
    }
    ReviewRequirement {
        contract: ReviewContract {
            kind: contract,
            version: review_contract_version(contract),
        },
        capability,
        checks,
        reasons,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewScope {
    Full,
    /// From round 3: prior findings and the code their fixes touched only.
    PriorFindingsAndFixes,
}

pub const ROUND_SCOPED_FROM: u32 = 3;

pub fn review_scope(round: u32) -> ReviewScope {
    if round >= ROUND_SCOPED_FROM {
        ReviewScope::PriorFindingsAndFixes
    } else {
        ReviewScope::Full
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewIndependence {
    DifferentVendor,
    DifferentModel,
    /// Only one model is available: a fresh session, disclosed rather than presented as diversity.
    SameModelFreshSession,
    Unknown,
}

/// Saved with each review attempt, independently of the implementation profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRoute {
    pub version: u32,
    pub requirement: ReviewRequirement,
    pub contract_digest: String,
    pub signals: ReviewSignals,
    pub profile: String,
    pub profile_revision: String,
    /// "explicit_override", "task_override", the policy layer, or "auto_mapping".
    pub profile_source: String,
    /// The `routing.review_profiles` key actually used under `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping: Option<ReviewContractKind>,
    pub independence: ReviewIndependence,
    pub capability_mismatch: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub round: u32,
    pub scope: ReviewScope,
    /// The superseded attempt whose stronger-review request produced this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalated_from: Option<String>,
    /// Set on an attempt that asked for stronger review before it was superseded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stronger_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_digest: Option<String>,
}

impl ReviewRoute {
    /// The identity a verdict must still match: contract, checks and reviewer configuration.
    pub fn same_assessment(&self, other: &ReviewRoute) -> bool {
        self.requirement == other.requirement
            && self.contract_digest == other.contract_digest
            && self.profile == other.profile
            && self.profile_revision == other.profile_revision
            && self.packet_digest == other.packet_digest
    }
}

impl Config {
    /// Owner-approved capability: a strong review needs a profile eligible for plan review.
    /// Legacy profiles without metadata keep their unrestricted behavior.
    pub fn review_capability_ok(&self, profile: &str, capability: ReviewCapability) -> bool {
        let Some(metadata) = self.profiles.get(profile).and_then(|p| p.metadata.as_ref()) else {
            return true;
        };
        capability == ReviewCapability::Routine
            || metadata
                .eligibility
                .roles
                .contains(&ProfileRole::PlanReview)
    }

    /// Resolve `routing.review_profiles` with its recorded, strength-preserving fallback chain.
    pub fn review_profile_for(
        &self,
        kind: ReviewContractKind,
    ) -> Option<(ReviewContractKind, &str)> {
        std::iter::once(kind)
            .chain(kind.fallbacks().iter().copied())
            .find_map(|k| {
                self.routing
                    .review_profiles
                    .get(&k)
                    .map(|p| (k, p.as_str()))
            })
    }

    pub(crate) fn review_profile_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (kind, profile) in &self.routing.review_profiles {
            let key = format!("routing.review_profiles.{}", kind.as_str());
            if let Err(error) = self.check_review_mapping(*kind, profile) {
                problems.push(format!("{key}: {error:#}"));
            }
        }
        let auto = std::iter::once(&self.review)
            .chain(self.projects.values().map(|p| &p.review))
            .any(|layer| layer.reviewer.as_deref().map(Reviewer::parse) == Some(Reviewer::Auto));
        if auto {
            for kind in [ReviewContractKind::Plan, ReviewContractKind::Implementation] {
                if !self.routing.review_profiles.contains_key(&kind) {
                    problems.push(format!(
                        "reviewer = \"auto\" needs routing.review_profiles.{}",
                        kind.as_str()
                    ));
                }
            }
        }
        problems
    }

    fn check_review_mapping(&self, kind: ReviewContractKind, profile: &str) -> Result<()> {
        ensure!(
            self.profile_names().iter().any(|p| p == profile),
            "unknown profile '{profile}'"
        );
        if let Some(metadata) = self.profiles.get(profile).and_then(|p| p.metadata.as_ref()) {
            let e = &metadata.eligibility;
            ensure!(
                e.enabled && e.automatic && e.roles.contains(&kind.role()),
                "profile '{profile}' is not enabled for automatic {:?}",
                kind.role()
            );
        }
        ensure!(
            self.review_capability_ok(profile, kind.capability()),
            "profile '{profile}' lacks plan-review eligibility required for strong review"
        );
        Ok(())
    }
}

/// Count prior `changes` verdicts so round-scoped review is a machine decision.
pub fn implementation_review_round(task: &Task) -> u32 {
    let changes = task
        .runs
        .iter()
        .flat_map(|run| &run.review_runs)
        .filter(|child| child.outcome != Some(crate::RunOutcome::Superseded))
        .filter_map(|child| child.review.as_ref())
        .filter(|r| r.verdict == Some(crate::Verdict::Changes))
        .count();
    u32::try_from(changes).unwrap_or(u32::MAX).saturating_add(1)
}

pub fn plan_review_round(task: &Task) -> u32 {
    let changes = task
        .plans
        .iter()
        .flat_map(|p| &p.reviews)
        .filter(|r| r.verdict == Some(crate::Verdict::Changes))
        .count();
    u32::try_from(changes).unwrap_or(u32::MAX).saturating_add(1)
}
