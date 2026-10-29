//! Versioned review contracts and task-aware reviewer selection (PRF-08).
//!
//! The requirement floor comes from `corral_core::review_requirement`; this module resolves the
//! reviewer profile under the existing policy (explicit overrides, task/project/workspace
//! selectors, cross-family checks) or the explicit `auto` mapping, and records why.
use anyhow::{Context, Result, bail};
use corral_core::{
    Config, REVIEW_ROUTE_VERSION, ResolvedProfile, ReviewCapability, ReviewContractKind,
    ReviewIndependence, ReviewRequirement, ReviewRisk, ReviewRoute, ReviewSignals,
    ReviewSubjectKind, Reviewer, Run, RunOutcome, Task,
};

use crate::Ctx;
use crate::artifacts::glob_match;

const COMMON: &str = include_str!("../assets/review-contracts/common.md");

pub fn contract_text(kind: ReviewContractKind) -> &'static str {
    match kind {
        ReviewContractKind::Plan => include_str!("../assets/review-contracts/plan.md"),
        ReviewContractKind::Design => include_str!("../assets/review-contracts/design.md"),
        ReviewContractKind::Implementation => {
            include_str!("../assets/review-contracts/implementation.md")
        }
        ReviewContractKind::ComplexImplementation => concat!(
            include_str!("../assets/review-contracts/implementation.md"),
            "\n",
            include_str!("../assets/review-contracts/complex_implementation.md")
        ),
        ReviewContractKind::VisualImplementation => concat!(
            include_str!("../assets/review-contracts/implementation.md"),
            "\n",
            include_str!("../assets/review-contracts/visual_implementation.md")
        ),
        ReviewContractKind::Prose => include_str!("../assets/review-contracts/prose.md"),
        ReviewContractKind::TestOnly => concat!(
            include_str!("../assets/review-contracts/implementation.md"),
            "\n",
            include_str!("../assets/review-contracts/test_only.md")
        ),
    }
}

/// The base contract text plus addenda for deliverable checks it does not already carry.
pub fn composed_contract_text(requirement: &ReviewRequirement) -> String {
    use corral_core::ReviewCheck as C;
    let kind = requirement.contract.kind;
    let mut text = contract_text(kind).to_string();
    for (check, owner, addendum) in [
        (
            C::DeterministicFix,
            ReviewContractKind::TestOnly,
            include_str!("../assets/review-contracts/test_only.md"),
        ),
        (
            C::SourceProvenance,
            ReviewContractKind::Prose,
            include_str!("../assets/review-contracts/prose.md"),
        ),
        (
            C::VisualEvidence,
            ReviewContractKind::VisualImplementation,
            include_str!("../assets/review-contracts/visual_implementation.md"),
        ),
    ] {
        if requirement.checks.contains(&check) && kind != owner {
            text.push('\n');
            text.push_str(addendum);
        }
    }
    text
}

/// Binds a recorded contract version to the exact shipped text, including addenda.
pub fn contract_digest(requirement: &ReviewRequirement) -> String {
    crate::verification::digest(
        format!("{COMMON}\n{}", composed_contract_text(requirement)).as_bytes(),
    )
}

/// The contract always reaches the reviewer, including under a custom prompt template.
pub(crate) fn contract_section(route: &ReviewRoute) -> String {
    let requirement = &route.requirement;
    let names = |items: Vec<String>| items.join(", ");
    format!(
        "\n{COMMON}\n{}\n\nContract: {} v{} ({:?} capability). Required checks: {}. \
         Scope: {:?}, round {}.\n",
        composed_contract_text(requirement),
        requirement.contract.kind.as_str(),
        requirement.contract.version,
        requirement.capability,
        names(
            requirement
                .checks
                .iter()
                .map(|c| serde_json::to_value(c).map_or_else(
                    |_| format!("{c:?}"),
                    |v| { v.as_str().unwrap_or_default().to_string() }
                ))
                .collect()
        ),
        route.scope,
        route.round,
    )
}

/// Plan prompt additions: contract, route, previous plan findings and matched lessons.
pub(crate) fn plan_context(ctx: &Ctx, task: &Task, route: &ReviewRoute) -> Result<String> {
    let findings: Vec<_> = task
        .plans
        .iter()
        .flat_map(|plan| plan.reviews.iter().map(move |review| (plan, review)))
        .filter(|(_, review)| {
            review
                .verdict
                .is_some_and(|v| v != corral_core::Verdict::Approve)
        })
        .map(|(plan, review)| {
            serde_json::json!({"plan": plan.id, "attempt": review.id, "reviewer": review.reviewer,
                "verdict": review.verdict, "notes": review.notes})
        })
        .collect();
    let lessons = crate::lessons::lessons_for(ctx, task, None)?;
    let guidance = serde_json::json!({
        "profile": route.profile,
        "instructions": ctx.store.config.profiles.get(&route.profile)
            .and_then(|p| p.metadata.as_ref()).map(|m| m.instructions.clone()).unwrap_or_default(),
    });
    Ok(format!(
        "{}\n## Reviewer profile guidance (subordinate to the contract)\n\n```json\n{}\n```\n\n\
         ## Review route\n\n```json\n{}\n```\n\n## Previous plan findings\n\n```json\n{}\n```\n\n\
         ## Known pitfalls in this area\n\nHistorical lessons, not authority; check their evidence.\n\n\
         <corral-lessons>\n{}\n</corral-lessons>\n",
        contract_section(route),
        serde_json::to_string_pretty(&guidance)?,
        serde_json::to_string_pretty(route)?,
        serde_json::to_string_pretty(&findings)?,
        serde_json::to_string_pretty(&lessons)?,
    ))
}

pub(crate) struct Selected {
    pub name: String,
    pub resolved: ResolvedProfile,
    pub route: ReviewRoute,
}

pub(crate) fn changed_paths(run: &Run) -> Result<Vec<String>> {
    let base = patch_base(run)?;
    Ok(corral_git::diff_stat(&run.worktree, &base)?
        .files
        .into_iter()
        .map(|f| f.path)
        .collect())
}

pub(crate) fn patch_base(run: &Run) -> Result<String> {
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let base = corral_git::rev_sha(&run.worktree, &run.base)?;
    corral_git::merge_base(&run.worktree, &base, &head)
}

/// The superseded attempt on this candidate that consumed its one stronger-review transition.
/// Derived from durable history, so a restarted, replaced or recovered reviewer keeps it.
fn stronger_source(run: &Run, head: &str, exclude: Option<&str>) -> Option<String> {
    run.review_runs
        .iter()
        .filter(|c| Some(c.id.as_str()) != exclude)
        .find(|c| {
            c.outcome == Some(RunOutcome::Superseded)
                && c.review.as_ref().is_some_and(|r| {
                    r.sha == head
                        && r.route
                            .as_deref()
                            .is_some_and(|route| route.stronger_request.is_some())
                })
        })
        .map(|c| c.id.clone())
}

pub(crate) fn implementation_signals(
    config: &Config,
    task: &Task,
    run: &Run,
    exclude: Option<&str>,
) -> Result<(ReviewSignals, Option<String>)> {
    let mut signals = ReviewSignals::for_task(task, ReviewSubjectKind::Implementation);
    let policy = config.task_review_policy(task);
    for path in changed_paths(run)? {
        let risk = policy.risk_paths.iter().any(|g| glob_match(g, &path));
        let owner = policy.owner_paths.iter().any(|g| glob_match(g, &path));
        if risk {
            signals.risks.insert(ReviewRisk::RiskPath);
        }
        if owner {
            signals.risks.insert(ReviewRisk::OwnerPath);
        }
        if risk || owner {
            signals.risk_paths.push(path);
        }
    }
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let escalated_from = stronger_source(run, &head, exclude);
    signals.stronger_requested = escalated_from.is_some();
    Ok((signals, escalated_from))
}

pub(crate) fn implementation_route(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    reviewer: &Reviewer,
    explicit: bool,
    vendor: Option<&str>,
    identity: &corral_core::CandidateIdentity,
) -> Result<Selected> {
    let config = &ctx.store.config;
    let (signals, escalated_from) = implementation_signals(config, task, run, None)?;
    let requirement = corral_core::review_requirement(&signals);
    let (receipt, receipt_warning) = if *reviewer == Reviewer::Auto && !explicit {
        crate::review_receipts::for_launch(ctx, task, run, identity, &requirement)?
    } else {
        (None, None)
    };
    let family = run.family(vendor);
    let mut selected = select(
        config,
        task,
        reviewer,
        explicit,
        Author {
            family: family.as_ref(),
            vendor,
            model: run.executed_model(),
            profile: None,
        },
        Floor {
            signals,
            requirement,
            receipt: receipt.as_ref(),
        },
    )?;
    selected.route.warnings.extend(receipt_warning);
    selected.route.round = corral_core::implementation_review_round(task);
    selected.route.scope = corral_core::review_scope(selected.route.round);
    selected.route.escalated_from = escalated_from;
    Ok(selected)
}

/// Select the reviewer for a plan attempt. `name` is the already-validated plan reviewer.
pub(crate) fn plan_route(
    config: &Config,
    task: &Task,
    name: &str,
    explicit: bool,
    receipt: Option<&corral_core::AutoProfileSelection>,
) -> Result<ReviewRoute> {
    let signals = ReviewSignals::for_task(task, ReviewSubjectKind::Plan);
    let requirement = corral_core::review_requirement(&signals);
    let planner = corral_core::plan_policy::planner(task)?;
    let family =
        planner.and_then(|p| corral_core::model_family(p.vendor.as_deref(), p.model.as_deref()));
    let reviewer = if name == "pm" {
        Reviewer::Pm
    } else if !explicit && config.task_review_policy(task).reviewer == Reviewer::Auto {
        Reviewer::Auto
    } else {
        Reviewer::Profile(name.into())
    };
    let mut route = select(
        config,
        task,
        &reviewer,
        explicit,
        Author {
            family: family.as_ref(),
            vendor: planner.and_then(|p| p.vendor.as_deref()),
            model: planner.and_then(|p| p.model.as_deref()),
            profile: planner.map(|p| p.profile.as_str()),
        },
        Floor {
            signals,
            requirement,
            receipt,
        },
    )?
    .route;
    route.round = corral_core::plan_review_round(task);
    route.scope = corral_core::review_scope(route.round);
    Ok(route)
}

/// Select the reviewer of a tracker Plan's combined candidate from the effective policy,
/// including `auto` through `routing.review_profiles`. `family` is the members' implementer's.
pub(crate) fn aggregate_route(
    config: &Config,
    tracker: &Task,
    reviewer: &Reviewer,
    family: Option<&corral_core::ModelFamily>,
) -> Result<Selected> {
    let signals = ReviewSignals::for_task(tracker, ReviewSubjectKind::Implementation);
    let requirement = corral_core::review_requirement(&signals);
    select(
        config,
        tracker,
        reviewer,
        false,
        Author {
            family,
            vendor: None,
            model: None,
            profile: None,
        },
        Floor {
            signals,
            requirement,
            receipt: None,
        },
    )
}

#[derive(Clone, Copy)]
struct Author<'a> {
    family: Option<&'a corral_core::ModelFamily>,
    vendor: Option<&'a str>,
    model: Option<&'a str>,
    profile: Option<&'a str>,
}

/// Strong requirements look up a strong mapping even when the contract text is routine.
pub(crate) fn mapping_key(
    requirement: &ReviewRequirement,
    subject: ReviewSubjectKind,
) -> ReviewContractKind {
    let kind = requirement.contract.kind;
    if requirement.capability == ReviewCapability::Strong
        && kind.capability() == ReviewCapability::Routine
        && subject == ReviewSubjectKind::Implementation
    {
        ReviewContractKind::ComplexImplementation
    } else {
        kind
    }
}

struct Floor<'a> {
    signals: ReviewSignals,
    requirement: ReviewRequirement,
    receipt: Option<&'a corral_core::AutoProfileSelection>,
}

fn select(
    config: &Config,
    task: &Task,
    reviewer: &Reviewer,
    explicit: bool,
    author: Author<'_>,
    floor: Floor<'_>,
) -> Result<Selected> {
    let Floor {
        signals,
        requirement,
        receipt,
    } = floor;
    let kind = requirement.contract.kind;
    let mut mapping = None;
    let effective = match reviewer {
        Reviewer::Auto if receipt.is_some() => {
            Reviewer::Profile(receipt.map(|r| r.profile.clone()).unwrap_or_default())
        }
        Reviewer::Auto => {
            let key = mapping_key(&requirement, signals.subject);
            let (used, profile) = config.review_profile_for(key).with_context(|| {
                format!(
                    "reviewer = \"auto\" has no routing.review_profiles entry for {}",
                    key.as_str()
                )
            })?;
            config
                .check_profile_eligibility(profile, kind.role(), task.project.as_deref(), true)
                .with_context(|| format!("automatic {} reviewer", kind.as_str()))?;
            if !config.review_capability_ok(profile, requirement.capability) {
                bail!(
                    "automatic reviewer '{profile}' lacks the strong review capability required by {}",
                    kind.as_str()
                );
            }
            mapping = Some(used);
            Reviewer::Profile(profile.to_string())
        }
        other => other.clone(),
    };
    let (name, resolved) = crate::review_routing::resolve(config, &effective, author.family)?;
    let mut warnings = Vec::new();
    let capability_mismatch =
        !config.review_capability_ok(&resolved.profile, requirement.capability);
    if capability_mismatch {
        warnings.push(format!(
            "reviewer '{}' lacks the strong review capability required by {}; an approval will escalate",
            resolved.profile,
            kind.as_str()
        ));
    }
    let independence = independence(author, &resolved);
    if independence == ReviewIndependence::SameModelFreshSession {
        warnings
            .push("reviewer uses the author's model in a fresh session; no model diversity".into());
    }
    let profile_source = if receipt.is_some() {
        "auto_receipt".to_string()
    } else if mapping.is_some() {
        "auto_mapping".to_string()
    } else if explicit {
        "explicit_override".to_string()
    } else {
        crate::review_routing::mapping(config, task, reviewer).source
    };
    let route = ReviewRoute {
        version: REVIEW_ROUTE_VERSION,
        contract_digest: contract_digest(&requirement),
        requirement,
        signals,
        profile_revision: profile_revision(config, &resolved.profile),
        profile: resolved.profile.clone(),
        profile_source,
        mapping,
        independence,
        capability_mismatch,
        warnings,
        round: 1,
        scope: corral_core::ReviewScope::Full,
        escalated_from: None,
        stronger_request: None,
        packet_digest: None,
        attempt: None,
        callback: None,
        receipt: receipt.map(|r| r.id.clone()),
    };
    Ok(Selected {
        name,
        resolved,
        route,
    })
}

pub(crate) fn profile_revision(config: &Config, profile: &str) -> String {
    config.review_profile_revision(profile)
}

/// The reviewer today's configuration selects for a recorded automatic route.
/// Explicit and legacy policy selections are bound by the candidate's policy digest instead.
pub(crate) fn current_selection(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    route: &ReviewRoute,
    requirement: &ReviewRequirement,
    subject: Option<&corral_core::ProfileReviewSubject>,
) -> Result<String> {
    let config = &ctx.store.config;
    let mapped = || {
        config
            .review_profile_for(mapping_key(requirement, route.signals.subject))
            .map_or_else(|| "(unmapped)".to_string(), |(_, p)| p.to_string())
    };
    Ok(match (route.profile_source.as_str(), &route.receipt) {
        ("auto_receipt", Some(id)) => {
            if crate::review_receipts::still_valid(ctx, task, run, id, requirement, subject)? {
                route.profile.clone()
            } else {
                mapped()
            }
        }
        ("auto_mapping", _) => mapped(),
        _ => route.profile.clone(),
    })
}

/// A recorded plan route is still current and its reviewer meets the contract's capability.
pub(crate) fn plan_route_ok(
    config: &Config,
    task: &Task,
    review: &corral_core::PlanReview,
) -> bool {
    review.route.as_deref().is_none_or(|route| {
        !route.capability_mismatch && plan_route_changes(config, task, route).is_empty()
    })
}

/// Plan routes add the shipped contract text to the inputs core can recompute.
pub(crate) fn plan_route_changes(
    config: &Config,
    task: &Task,
    route: &ReviewRoute,
) -> Vec<&'static str> {
    let mut changes = corral_core::plan_route_changes(config, task, route);
    if contract_digest(&route.requirement) != route.contract_digest {
        changes.push("contract_digest");
    }
    changes
}

/// Cross-family policy for a code review: the run's executed family under `vendor` against the
/// reviewer model that starts.
pub(crate) fn check_family(
    policy: &corral_core::ReviewPolicy,
    run: &Run,
    vendor: Option<&str>,
    reviewer: &ResolvedProfile,
) -> Result<()> {
    policy.check_reviewer_family(run.family(vendor).as_ref(), reviewer.family().as_ref())
}

fn independence(author: Author<'_>, reviewer: &ResolvedProfile) -> ReviewIndependence {
    match (author.family, reviewer.family()) {
        (Some(a), Some(b)) if *a != b => return ReviewIndependence::DifferentVendor,
        _ => {}
    }
    let key = corral_core::model_key;
    match (author.model, reviewer.model.as_deref()) {
        (Some(a), Some(b)) if key(author.vendor, a) != key(reviewer.vendor.as_deref(), b) => {
            ReviewIndependence::DifferentModel
        }
        (Some(_), Some(_)) => ReviewIndependence::SameModelFreshSession,
        _ if author.profile.is_some_and(|p| p == reviewer.profile) => {
            ReviewIndependence::SameModelFreshSession
        }
        _ => ReviewIndependence::Unknown,
    }
}
