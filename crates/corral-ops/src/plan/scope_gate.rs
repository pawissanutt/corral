//! A Plan target member's gate: owner member-gate record, then the accepted plan scope's gate
//! found through the checked publication binding, then `tier_policy.plan_member`, then full.
//! Mutable child fields (title, labels, body) never choose the scope.
use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context, Result, ensure};
use corral_core::{Config, FULL_TIER, PlanDeliveryTarget, PlanDocument, ScopeGate, Store, Task};
use serde::Deserialize;

#[cfg(test)]
#[path = "scope_gate_tests.rs"]
mod tests;

/// The resolved member gate and the source its binding names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberGate {
    pub(crate) tracker: String,
    pub(crate) gate: ScopeGate,
    /// `member-gate:<tracker>@<revision>`, `plan:<design>@<Pn>#<label>@<scope_digest>`,
    /// `policy:plan_member` or `default:full`.
    pub(crate) source: String,
}

/// A member whose approved scope cannot be proven gets no gate rather than a fallback, which
/// could silently drop a planner's stricter tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScopeGateError {
    MissingBinding {
        member: String,
        design: String,
    },
    ScopeDigestMismatch {
        member: String,
        design: String,
        label: String,
    },
}

impl fmt::Display for ScopeGateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBinding { member, design } => write!(
                f,
                "member gate unresolved: {member} has no publication binding in {design}; no gate starts"
            ),
            Self::ScopeDigestMismatch {
                member,
                design,
                label,
            } => write!(
                f,
                "member gate unresolved: scope {label:?} of {design} no longer matches the digest {member} was published with; no gate starts"
            ),
        }
    }
}

impl std::error::Error for ScopeGateError {}

/// Only the fields this resolver reads from `state/plan-decompose-<design>.json`.
#[derive(Deserialize)]
struct Publication {
    backlink: String,
    #[serde(default)]
    scopes: BTreeMap<String, Binding>,
}

#[derive(Deserialize)]
struct Binding {
    task: String,
    scope_digest: String,
}

/// Resolves the gate of a member of an owned integration Plan. `None` for tasks without an
/// execution Plan and for members of main-delivery Plans, whose gates are not member gates.
///
/// # Errors
/// A [`ScopeGateError`] when a designed member's publication binding is missing or its scope
/// digest no longer matches the accepted revision; store errors otherwise.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "consumed by gate_tier::select (T-251 tier-selection-identity)"
    )
)]
pub(crate) fn resolve(store: &Store, task: &Task) -> Result<Option<MemberGate>> {
    let Some(tracker) = store.execution_plan(task)? else {
        return Ok(None);
    };
    let scope = tracker
        .approved_scope
        .as_ref()
        .context("Plan scope missing")?;
    if tracker.id == task.id || scope.delivery_target == PlanDeliveryTarget::Main {
        return Ok(None);
    }
    let member = |gate, source| {
        Ok(Some(MemberGate {
            tracker: tracker.id.clone(),
            gate,
            source,
        }))
    };
    if let Some(record) = tracker.member_gates.get(&task.id) {
        let source = format!("member-gate:{}@{}", tracker.id, record.revision);
        return member(record.gate.clone(), source);
    }
    // Imported trackers have no approved design: declared tests and expected failures come
    // only from an owner record.
    if let Some(design) = &scope.approved_design
        && let Some((gate, source)) = plan_scope_gate(store, task, design)?
    {
        return member(gate, source);
    }
    let policy = store.config.review_policy(task.project.as_deref(), None);
    let (tier, source) = match policy.tier_policy.plan_member {
        Some(tier) => (tier, "policy:plan_member"),
        None => (FULL_TIER.to_owned(), "default:full"),
    };
    let gate = ScopeGate {
        tier,
        tests: Vec::new(),
        expected_failures: Vec::new(),
    };
    member(gate, source.to_owned())
}

/// The accepted scope's gate through the exact binding `<design>@<Pn>` -> member -> label,
/// checked against the digest of the accepted revision's scope.
fn plan_scope_gate(
    store: &Store,
    task: &Task,
    design: &str,
) -> Result<Option<(ScopeGate, String)>> {
    let missing = || ScopeGateError::MissingBinding {
        member: task.id.clone(),
        design: design.to_owned(),
    };
    let (parent, revision) = design
        .split_once('@')
        .with_context(|| format!("malformed approved design {design:?}"))?;
    if task.plan.as_deref() != Some(design) {
        return Err(missing().into());
    }
    let parent = store.get(parent)?;
    let Some(plan) = parent
        .plans
        .iter()
        .find(|plan| plan.id == revision && plan.approval.is_some())
    else {
        return Err(missing().into());
    };
    let Some((label, digest)) = published_binding(store, &parent.id, design, &task.id)? else {
        return Err(missing().into());
    };
    let scope = plan
        .document
        .dependencies
        .iter()
        .find(|scope| scope.task == label);
    let current = scope
        .map(|scope| serde_json::to_vec(scope).map(|bytes| crate::verification::digest(&bytes)))
        .transpose()?;
    let Some(scope) = scope.filter(|_| current.as_ref() == Some(&digest)) else {
        return Err(ScopeGateError::ScopeDigestMismatch {
            member: task.id.clone(),
            design: design.to_owned(),
            label,
        }
        .into());
    };
    Ok(scope
        .gate
        .clone()
        .map(|gate| (gate, format!("plan:{design}#{label}@{digest}"))))
}

/// `(label, scope_digest)` of the journal scope that published `member` from `backlink`.
fn published_binding(
    store: &Store,
    design: &str,
    backlink: &str,
    member: &str,
) -> Result<Option<(String, String)>> {
    let path = store
        .state_dir()
        .join(format!("plan-decompose-{design}.json"));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let journal: Publication = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing publication journal {}", path.display()))?;
    if journal.backlink != backlink {
        return Ok(None);
    }
    Ok(journal
        .scopes
        .into_iter()
        .find(|(_, binding)| binding.task == member)
        .map(|(label, binding)| (label, binding.scope_digest)))
}

/// Every scope gate must name `full` or a tier the task's project defines.
///
/// # Errors
/// Names the scope label and the undefined tier.
pub(crate) fn check_declared_tiers(
    config: &Config,
    project: Option<&str>,
    document: &PlanDocument,
) -> Result<()> {
    let tiers = config.review_policy(project, None).gate_tiers;
    for scope in &document.dependencies {
        let Some(gate) = &scope.gate else { continue };
        ensure!(
            gate.tier == FULL_TIER || tiers.contains_key(&gate.tier),
            "scope {:?}: gate tier {:?} is not defined for project {} (defined: {})",
            scope.task,
            gate.tier,
            project.unwrap_or("(none)"),
            std::iter::once(FULL_TIER)
                .chain(tiers.keys().map(String::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}
