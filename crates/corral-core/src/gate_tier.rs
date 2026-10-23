//! Persisted shapes of named gate tiers: configuration, Plan scope gates, owner member-gate
//! records, receipt bindings, and gate, verdict and evidence carries. Every new field that uses
//! them is optional and omitted when absent, so untiered records serialize exactly as before.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::config_validation::SemanticInputs;
use crate::{CandidateIdentity, Config, ReviewOverride};

/// The reserved tier name of the existing `[review] gate`.
pub const FULL_TIER: &str = "full";

/// `[review] tier_policy`: default tiers by role, merged field by field across policy layers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_member: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_rounds: Option<String>,
}

impl TierPolicy {
    pub fn is_empty(&self) -> bool {
        self.plan_member.is_none() && self.review_rounds.is_none()
    }

    pub(crate) fn merge(&mut self, layer: &TierPolicy) {
        if let Some(tier) = &layer.plan_member {
            self.plan_member = Some(tier.clone());
        }
        if let Some(tier) = &layer.review_rounds {
            self.review_rounds = Some(tier.clone());
        }
    }
}

/// A gate a planner declares for one Plan scope, or an owner sets by member-gate amendment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeGate {
    pub tier: String,
    /// Test selections that replace the touched-crate default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_failures: Vec<ExpectedFailure>,
}

/// A test the tier command may see fail, named `<binary-id> <test-name>` as nextest reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedFailure {
    pub test: String,
    /// Why it fails and which child restores it.
    pub reason: String,
}

impl ScopeGate {
    /// Checks the gate's own shape. Whether a project defines the tier is checked where its
    /// configuration is known.
    ///
    /// # Errors
    /// Rejects a malformed tier name, tests or expected failures on the full tier, blank or
    /// control-character tests, expected failures without a reason, and malformed test IDs.
    pub fn validate(&self) -> Result<()> {
        check_tier_name(&self.tier)?;
        ensure!(
            self.tier != FULL_TIER || self.tests.is_empty() && self.expected_failures.is_empty(),
            "tier full takes no tests or expected failures"
        );
        for test in &self.tests {
            ensure!(
                !test.trim().is_empty(),
                "tests must not contain blank entries"
            );
            ensure!(
                !test.chars().any(char::is_control),
                "test {test:?} contains a control character"
            );
        }
        for failure in &self.expected_failures {
            check_test_id(&failure.test)?;
            ensure!(
                !failure.reason.trim().is_empty(),
                "expected failure {:?} needs a reason",
                failure.test
            );
        }
        Ok(())
    }
}

fn check_tier_name(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    ensure!(
        bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "tier name {name:?} must match [a-z][a-z0-9-]*"
    );
    Ok(())
}

fn check_test_id(id: &str) -> Result<()> {
    ensure!(
        !id.chars().any(char::is_control),
        "expected failure {id:?} contains a control character"
    );
    id.split_once(' ')
        .filter(|(binary, name)| !binary.is_empty() && !name.is_empty() && name.trim() == *name)
        .with_context(|| {
            format!("expected failure {id:?} must have the form '<binary-id> <test-name>'")
        })?;
    Ok(())
}

impl Config {
    /// Tier key problems of every review layer, each naming its layer. A project layer may name
    /// tiers it inherits from `[review]`; references are not checked when a rejected `gate_tiers`
    /// left the inventory unknown.
    pub(crate) fn tier_problems(&self, inputs: &SemanticInputs) -> Vec<String> {
        let key = |parts: &[&str]| parts.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
        let global = self.review.gate_tiers.as_ref();
        let global_known = inputs.available(&key(&["review", "gate_tiers"]));
        let visible = global_known.then(|| visible_tiers(global));
        let mut problems = layer_problems("[review]", &self.review, visible.as_ref());
        for (name, project) in &self.projects {
            let known =
                global_known && inputs.available(&key(&["projects", name, "review", "gate_tiers"]));
            let visible = known.then(|| {
                visible_tiers(global.into_iter().chain(project.review.gate_tiers.as_ref()))
            });
            problems.extend(layer_problems(
                &format!("[projects.{name}.review]"),
                &project.review,
                visible.as_ref(),
            ));
        }
        problems
    }
}

fn layer_problems(
    name: &str,
    layer: &ReviewOverride,
    visible: Option<&BTreeSet<&str>>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (tier, commands) in layer.gate_tiers.iter().flatten() {
        if tier == FULL_TIER {
            problems.push(format!("{name}: gate_tiers.full is reserved for gate"));
        } else if let Err(error) = check_tier_name(tier) {
            problems.push(format!("{name}: gate_tiers: {error}"));
        }
        if commands.is_empty() {
            problems.push(format!("{name}: gate_tiers.{tier} has no commands"));
        } else if commands.iter().any(|command| command.trim().is_empty()) {
            problems.push(format!("{name}: gate_tiers.{tier} has an empty command"));
        }
    }
    let policy = layer.tier_policy.as_ref();
    for (field, tier) in [
        ("plan_member", policy.and_then(|p| p.plan_member.as_deref())),
        (
            "review_rounds",
            policy.and_then(|p| p.review_rounds.as_deref()),
        ),
    ] {
        let (Some(tier), Some(visible)) = (tier, visible) else {
            continue;
        };
        if tier != FULL_TIER && !visible.contains(tier) {
            let defined = std::iter::once(FULL_TIER)
                .chain(visible.iter().copied().filter(|t| *t != FULL_TIER))
                .collect::<Vec<_>>()
                .join(", ");
            problems.push(format!(
                "{name}: tier_policy.{field} names unknown tier {tier:?} (defined: {defined})"
            ));
        }
    }
    if layer.plan_checkpoint_every == Some(0) {
        problems.push(format!("{name}: plan_checkpoint_every must be at least 1"));
    }
    problems
}

/// Binds a receipt identity to its selected tier. `None` on an identity means full.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateTierBinding {
    pub tier: String,
    /// SHA-256 of the canonical selection: tier, commands, tests, expected failures and source.
    pub selection_sha256: String,
    /// Where the selection came from, such as a Plan scope digest or a member-gate revision.
    pub source: String,
}

/// An owner's revision-guarded member gate on a tracker; outside the operating contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberGateRecord {
    pub revision: u64,
    pub gate: ScopeGate,
    pub reason: String,
    pub by: String,
    pub at: Timestamp,
    /// Superseded revisions, oldest first; their own history is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<MemberGateRecord>,
}

/// Gate rules fixed when a run is dispatched. Legacy runs keep full-gate rules for life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateRules {
    #[default]
    Legacy,
    Tiered,
}

impl GateRules {
    pub fn is_legacy(&self) -> bool {
        *self == GateRules::Legacy
    }
}

/// The tier a gate record ran, its context file digest, and how it was carried or promoted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateTierReceipt {
    pub tier: String,
    pub context_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carried: Option<GateCarry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_from: Option<GatePromotion>,
}

/// A receipt rebound after a recorded clean, equal-patch replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCarry {
    pub from_head: String,
    pub from_tree: String,
    pub from_integration_base: String,
    pub old_patch_sha256: String,
    pub new_patch_sha256: String,
    pub at: Timestamp,
}

/// The approved reduced-tier identity a landing gate may promote, and that receipt's time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatePromotion {
    pub identity: CandidateIdentity,
    pub gate_at: Timestamp,
}

/// Records bound to `from` stand for `to` while `to` is the current identity and they still
/// hash to `records_sha256`. The evidence ledger itself is never rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceCarry {
    pub version: u32,
    pub method: CarryMethod,
    pub from: CandidateIdentity,
    /// Start of the capture and artifact window the carried records were admitted under.
    pub window_start: Timestamp,
    pub to: CandidateIdentity,
    pub records_sha256: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CarryMethod {
    #[serde(rename = "tier_promotion")]
    TierPromotion,
    #[serde(rename = "clean_replay+tier_promotion")]
    CleanReplayTierPromotion,
    #[serde(rename = "clean_replay")]
    CleanReplay,
}

fn visible_tiers<'a>(
    layers: impl IntoIterator<Item = &'a BTreeMap<String, Vec<String>>>,
) -> BTreeSet<&'a str> {
    layers
        .into_iter()
        .flat_map(|tiers| tiers.keys().map(String::as_str))
        .collect()
}

#[cfg(test)]
#[path = "gate_tier_tests.rs"]
mod tests;
