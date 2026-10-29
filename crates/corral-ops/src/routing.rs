//! Task-aware profile selection shared by dispatch and its read-only explanation.

use std::{collections::BTreeMap, fmt};

use anyhow::{Result, bail};
use corral_core::{
    ProfileChoice, ProfileRole, ResolvedProfile, Reviewer, RouteDecision, RouteSource, Task,
};
use serde::Serialize;

use crate::review_routing::{self, ReviewMapping};
use crate::{Ctx, ModelSource, harness, harness_report};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteExplanation {
    #[serde(flatten)]
    pub implementer: RouteDecision,
    pub review: ReviewExplanation,
    pub require_cross_family: bool,
    pub review_policy_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewExplanation {
    pub reviewer: ReviewMapping,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planner: Option<PlannerMapping>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_reviewer: Option<ReviewMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannerMapping {
    pub profile: String,
    pub harness: String,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub source: PlannerSource,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum PlannerSource {
    Route(RouteSource),
    Run { run: String },
}

impl fmt::Display for PlannerSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Route(source) => source.fmt(f),
            Self::Run { run } => write!(f, "planner run {run}"),
        }
    }
}

/// Explains a task's next run without creating a worktree, pane, or run record.
pub fn route(ctx: &Ctx, id: &str, choice: &ProfileChoice) -> Result<RouteExplanation> {
    route_with_plan(ctx, id, choice, false)
}

/// Includes explicit `dispatch --plan` intent without starting a planning session.
pub fn route_with_plan(
    ctx: &Ctx,
    id: &str,
    choice: &ProfileChoice,
    plan: bool,
) -> Result<RouteExplanation> {
    let task = ctx.store.get(id)?;
    route_snapshot(ctx, &task, choice, plan)
}

/// Resolves an already-loaded task/config snapshot for background consumers.
pub fn route_snapshot(
    ctx: &Ctx,
    task: &Task,
    choice: &ProfileChoice,
    plan: bool,
) -> Result<RouteExplanation> {
    crate::tracker::check_action(task, "route tracker")?;
    let mut implementer = if choice.profile.is_none()
        && choice.model.is_none()
        && choice.effort.is_none()
        && let Some(queued) = &task.dispatch_plan
    {
        crate::queue::resolve_plan(
            ctx,
            task,
            &crate::DispatchOpts {
                plan: plan || queued.plan,
                ..Default::default()
            },
        )?
        .route
    } else {
        route_task_for_role(
            ctx,
            task,
            choice,
            if crate::plan_dispatch::needed(
                ctx,
                task,
                &crate::DispatchOpts {
                    plan,
                    ..Default::default()
                },
            ) {
                ProfileRole::Planning
            } else {
                ProfileRole::Implementation
            },
        )?
    };
    explain_route(ctx, task, &mut implementer, plan, None)
}

/// Explains an implementation or planning run using its launch identity, not today's routing rules.
/// Reviewer sessions, when present, retain their recorded identity too.
pub fn recorded_route(ctx: &Ctx, task: &Task) -> Result<RouteExplanation> {
    crate::tracker::check_action(task, "route tracker")?;
    if task.current_run().is_none()
        && let Some(run) = task.plan_runs.last()
    {
        let mut planner = RouteDecision {
            profile: run.profile.clone(),
            harness: run.harness.clone(),
            model: run.model.clone(),
            effort: run.effort.clone(),
            route: run.route.clone().unwrap_or(RouteSource::Override),
            warnings: Vec::new(),
            availability: None,
        };
        let mut explanation =
            explain_route(ctx, task, &mut planner, true, Some(run.vendor.as_deref()))?;
        if let Some(planner) = &mut explanation.review.planner {
            planner.source = PlannerSource::Run {
                run: run.id.clone(),
            };
        }
        if let Some(review) = task
            .plans
            .iter()
            .rev()
            .find(|plan| plan.run.as_deref() == Some(run.id.as_str()))
            .and_then(|plan| plan.reviews.last())
        {
            let config = &ctx.store.config;
            let selected = Reviewer::parse(&review.reviewer);
            let mut mapping = review_routing::mapping(config, task, &selected);
            mapping.source = "recorded_plan_review".into();
            mapping.policy_source = "recorded_plan_review".into();
            mapping.rule = None;
            if review.reviewer == "owner" {
                mapping.profile = None;
            } else {
                let family = corral_core::model_family(run.vendor.as_deref(), run.model.as_deref());
                match review_routing::resolve(config, &selected, family.as_ref()) {
                    Ok((_, profile)) => {
                        review_routing::fill(&mut mapping, profile, family.as_ref(), "planner");
                    }
                    Err(error) => mapping.warnings.push(format!("{error:#}")),
                }
            }
            explanation.review.reviewer = mapping.clone();
            explanation.review.plan_reviewer = Some(mapping);
        }
        return Ok(explanation);
    }
    let run = task
        .current_run()
        .ok_or_else(|| anyhow::anyhow!("task has no run"))?;
    let mut implementer = RouteDecision {
        profile: run.profile.clone().unwrap_or_else(|| run.agent.clone()),
        harness: run.harness.clone().unwrap_or_else(|| run.agent.clone()),
        model: run.model.clone(),
        effort: run.effort.clone(),
        route: run.route.clone().unwrap_or(RouteSource::Override),
        warnings: Vec::new(),
        availability: run.execution.as_ref().and_then(|e| e.candidate.clone()),
    };
    let mut explanation = explain_route(
        ctx,
        task,
        &mut implementer,
        false,
        Some(run.vendor.as_deref()),
    )?;
    if let Some(reviewer) = run.review_runs.last() {
        explanation.review.reviewer.profile = Some(reviewer.agent.clone());
        explanation.review.reviewer.harness = reviewer.harness.clone();
        explanation.review.reviewer.model = reviewer.model.clone();
        explanation.review.reviewer.vendor = reviewer.vendor.clone();
        explanation.review.reviewer.selection = reviewer.agent.clone();
        explanation.review.reviewer.source = "recorded_run".into();
        explanation.review.reviewer.rule = None;
        explanation.review.reviewer.warnings.clear();
        let family = reviewer.family(reviewer.vendor.as_deref());
        if family.is_some() && family == run.family(run.vendor.as_deref()) {
            explanation
                .review
                .reviewer
                .warnings
                .push("reviewer uses the same model family as the implementer".into());
        }
    }
    Ok(explanation)
}

fn explain_route(
    ctx: &Ctx,
    task: &Task,
    implementer: &mut RouteDecision,
    plan: bool,
    recorded_vendor: Option<Option<&str>>,
) -> Result<RouteExplanation> {
    if matches!(
        implementer.route,
        RouteSource::ProjectDefault | RouteSource::Defaults | RouteSource::Fallback
    ) {
        implementer
            .warnings
            .push("default profile; no owner rule or Auto selection supplied this choice".into());
    }
    let config = &ctx.store.config;
    let policy = config.task_review_policy(task);
    let sources = config.review_policy_sources(task.project.as_deref(), task.review);
    let review_policy_source =
        if policy.reviewer == Reviewer::CrossVendor && task.reviewer.is_some() {
            "task reviewer:".into()
        } else {
            if policy.reviewer == Reviewer::CrossVendor {
                sources.reviewer
            } else {
                sources.require_cross_family
            }
            .label(task.project.as_deref())
        };
    let resolved = if recorded_vendor.is_some() {
        None
    } else {
        Some(config.resolve_profile(
            &ProfileChoice {
                profile: Some(implementer.profile.clone()),
                model: implementer.model.clone(),
                effort: implementer.effort.clone(),
            },
            None,
            None,
        )?)
    };
    let vendor =
        recorded_vendor.unwrap_or_else(|| resolved.as_ref().and_then(|r| r.vendor.as_deref()));
    let model = resolved
        .as_ref()
        .and_then(|r| r.model.as_deref())
        .or(implementer.model.as_deref());
    let family = corral_core::model_family(vendor, model);
    let mut review = ReviewExplanation {
        reviewer: review_routing::explain(config, task, family.as_ref()),
        planner: None,
        plan_reviewer: None,
    };
    let future_plan = plan || task.dispatch_plan.as_ref().is_some_and(|p| p.plan);
    let submitted_plan = !future_plan && !task.plans.is_empty();
    if future_plan
        || submitted_plan
        || task.design
        || config.plan_required(task.project.as_deref(), task.plan_required)
    {
        let planner = if submitted_plan {
            crate::plan::planner(task)?.map(|run| PlannerMapping {
                profile: run.profile.clone(),
                harness: run.harness.clone(),
                model: run.model.clone(),
                vendor: run.vendor.clone(),
                source: PlannerSource::Run {
                    run: run.id.clone(),
                },
                warnings: Vec::new(),
            })
        } else {
            Some(PlannerMapping {
                profile: implementer.profile.clone(),
                harness: implementer.harness.clone(),
                model: implementer.model.clone(),
                vendor: vendor.map(str::to_string),
                source: PlannerSource::Route(implementer.route.clone()),
                warnings: implementer.warnings.clone(),
            })
        };
        let identity = planner.as_ref().map(|planner| crate::plan::PlannerProfile {
            profile: &planner.profile,
            model: planner.model.as_deref(),
            vendor: planner.vendor.as_deref(),
        });
        let reference_family = match identity {
            Some(planner) => planner.family(),
            None => family.clone(),
        };
        let reference_family = reference_family.as_ref();
        let reference_role = if identity.is_some() {
            "planner"
        } else {
            "implementer"
        };
        let selected = review_routing::choice(config, task, None);
        let mut mapping = review_routing::mapping(config, task, &selected);
        let expected = if submitted_plan {
            crate::plan::expected_reviewer(config, task)
        } else {
            crate::plan::expected_reviewer_for(config, task, identity, family.as_ref())
        };
        match expected {
            Ok(name) if name == "owner" => {
                mapping.selection = name;
                mapping.source = "owner".into();
            }
            Ok(name) => {
                if let Err(error) =
                    crate::plan::independent_reviewer_for(config, task, identity, &name)
                {
                    mapping.warnings.push(format!("{error:#}"));
                }
                let reviewer = if name == "pm" {
                    Reviewer::Pm
                } else {
                    Reviewer::Profile(name)
                };
                if selected == Reviewer::None {
                    mapping.source = "independent_profile".into();
                }
                match review_routing::resolve(config, &reviewer, reference_family) {
                    Ok((name, profile)) => {
                        mapping.selection = name;
                        review_routing::fill(
                            &mut mapping,
                            profile,
                            reference_family,
                            reference_role,
                        );
                    }
                    Err(error) => mapping.warnings.push(format!("{error:#}")),
                }
            }
            Err(error) => {
                if selected != Reviewer::None
                    && let Ok((_, profile)) =
                        review_routing::resolve(config, &selected, reference_family)
                {
                    review_routing::fill(&mut mapping, profile, reference_family, reference_role);
                }
                mapping.warnings.push(format!("{error:#}"));
            }
        }
        review.planner = planner;
        review.plan_reviewer = Some(mapping);
    }
    Ok(RouteExplanation {
        implementer: implementer.clone(),
        review,
        require_cross_family: policy.cross_family_required(),
        review_policy_source,
    })
}

pub(crate) fn route_task(ctx: &Ctx, task: &Task, choice: &ProfileChoice) -> Result<RouteDecision> {
    route_task_for_role(ctx, task, choice, ProfileRole::Implementation)
}

pub(crate) fn route_task_for_role(
    ctx: &Ctx,
    task: &Task,
    choice: &ProfileChoice,
    role: ProfileRole,
) -> Result<RouteDecision> {
    crate::tracker::check_action(task, "route tracker")?;
    let selections = ctx.store.auto_profile_selections()?;
    let selection = selections
        .iter()
        .rev()
        .find(|selection| selection.task == task.id && selection.role == role);
    let mut catalogs = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut check = |profile: &ResolvedProfile| {
        let model = profile.model.as_deref()?;
        let catalog = catalogs
            .entry(profile.harness.clone())
            .or_insert_with(|| model_catalog(ctx, &profile.harness));
        match catalog {
            ModelCatalog::Known(models) => {
                rejection(&profile.harness, profile.vendor.as_deref(), model, models)
            }
            ModelCatalog::Unknown(reason) => {
                if !warnings.contains(reason) {
                    warnings.push(reason.clone());
                }
                None
            }
        }
    };
    let config = &ctx.store.config;
    let mut decision =
        config.route_task_with_selection(task, choice, role, selection, |profile| {
            let reason = check(profile)?;
            // Availability changes the candidate within the profile, never the profile.
            (!crate::availability::has_alternative(config, &profile.profile, choice, &mut check))
                .then_some(reason)
        })?;
    crate::availability::bind_route(ctx, task, role, choice, &mut decision, &mut check)?;
    decision.warnings.extend(warnings);
    Ok(decision)
}

/// Rechecks the final model before any dispatch worktree or pane is created.
/// Unknown catalogs are warnings, never evidence that an entered ID is invalid.
pub(crate) fn validate_model(ctx: &Ctx, decision: &RouteDecision) -> Result<Option<String>> {
    let Some(model) = decision.model.as_deref() else {
        return Ok(None);
    };
    match discovered_catalog(ctx, &decision.harness) {
        ModelCatalog::Known(models) => {
            let vendor = ctx
                .store
                .config
                .resolve_route(decision)
                .ok()
                .and_then(|resolved| resolved.vendor);
            if let Some(reason) = rejection(&decision.harness, vendor.as_deref(), model, &models) {
                bail!("profile '{}': {reason}", decision.profile);
            }
            Ok(None)
        }
        ModelCatalog::Unknown(reason) => Ok(Some(reason)),
    }
}

enum ModelCatalog {
    Known(Vec<String>),
    Unknown(String),
}

/// Existing local authentication/settings and a usable cache can exclude a model.
/// Missing observations are not proof of access, and this path never runs discovery.
pub(crate) fn known_model_unavailability(
    ctx: &Ctx,
    profile: &corral_core::ResolvedProfile,
) -> Option<String> {
    let model = profile.model.as_deref()?;
    let is_omo = ctx
        .store
        .config
        .harnesses
        .get(&profile.harness)
        .and_then(|harness| harness.flags.as_deref())
        .unwrap_or(&profile.harness)
        == "omo";
    if is_omo && let Some(models) = harness::preferred_omo_models(ctx) {
        return rejection(&profile.harness, profile.vendor.as_deref(), model, &models);
    }
    let report = harness::cached_harness_report(ctx)?;
    let info = report.get(&profile.harness)?;
    match info.model_source {
        ModelSource::Listed | ModelSource::Builtin => rejection(
            &profile.harness,
            profile.vendor.as_deref(),
            model,
            &info.models,
        ),
        ModelSource::FreeText | ModelSource::None => None,
    }
}

fn model_catalog(ctx: &Ctx, key: &str) -> ModelCatalog {
    let is_omo = ctx
        .store
        .config
        .harnesses
        .get(key)
        .and_then(|h| h.flags.as_deref())
        .unwrap_or(key)
        == "omo";
    if is_omo && let Some(models) = harness::preferred_omo_models(ctx) {
        return ModelCatalog::Known(models);
    }
    discovered_catalog(ctx, key)
}

fn discovered_catalog(ctx: &Ctx, key: &str) -> ModelCatalog {
    let report = match harness_report(ctx, false) {
        Ok(report) => report,
        Err(error) => {
            return ModelCatalog::Unknown(format!(
                "harness '{key}': model discovery failed: {error:#}; allowing unverified model"
            ));
        }
    };
    let Some(info) = report.get(key) else {
        return ModelCatalog::Unknown(format!(
            "harness '{key}': no model discovery result; allowing unverified model"
        ));
    };
    match info.model_source {
        ModelSource::Listed | ModelSource::Builtin => ModelCatalog::Known(info.models.clone()),
        ModelSource::FreeText | ModelSource::None => ModelCatalog::Unknown(format!(
            "harness '{key}': model discovery unavailable{}; allowing unverified model",
            if info.notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", info.notes.join("; "))
            }
        )),
    }
}

/// Whether `model` is outside the discovered catalog of a profile on `harness` bound to `vendor`.
/// A pi-native profile is checked against its own provider's models only, with a bare id
/// qualified by that provider. A selector naming another provider never runs on its host; when
/// discovery listed none of the profile provider's models (it is not logged in), the catalog is
/// no evidence about any other id.
fn rejection(
    harness: &str,
    vendor: Option<&str>,
    model: &str,
    models: &[String],
) -> Option<String> {
    let scoped = harness::models_for_profile(harness, vendor, Some(model), models);
    if scoped.is_empty() && !models.is_empty() && !harness::foreign_selector(harness, vendor, model)
    {
        return None;
    }
    let models = &scoped[..];
    let selector = harness::catalog_selector(harness, vendor, model);
    if models.contains(&selector) {
        return None;
    }
    let mut nearest: Vec<_> = models
        .iter()
        .map(|id| (edit_distance(model, id), id))
        .collect();
    nearest.sort_unstable();
    let close: Vec<_> = nearest.iter().take(3).map(|(_, id)| id.as_str()).collect();
    Some(format!(
        "model '{model}' is not an exact discovered model ID for harness '{harness}'; close matches: {}",
        if close.is_empty() {
            "(none)".into()
        } else {
            close.join(", ")
        }
    ))
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<_> = right.chars().collect();
    let mut row: Vec<_> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(a != *b));
            diagonal = above;
        }
    }
    row[right.len()]
}

/// Sets or clears a task's reviewer override without changing its implementer.
pub fn reviewer_set(ctx: &Ctx, id: &str, reviewer: Option<&str>) -> Result<Task> {
    let (task, old) = ctx.store.dispatch_update(id, |store, task| {
        if let Some(name) = reviewer
            && !matches!(name, "pm" | "cross-vendor" | "none")
        {
            store.config.resolve_profile(
                &ProfileChoice {
                    profile: Some(name.to_string()),
                    ..Default::default()
                },
                None,
                None,
            )?;
        }
        let old = task.reviewer.clone();
        task.reviewer = reviewer.map(str::to_string);
        Ok(old)
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "reviewer_override",
        serde_json::json!({"old": old, "new": task.reviewer}),
    )?;
    Ok(task)
}

/// Pins a task's implementer profile for its next dispatch through the same atomic update as reviewer overrides.
pub fn implementer_set(ctx: &Ctx, id: &str, profile: &str) -> Result<Task> {
    let (task, old) = ctx.store.dispatch_update(id, |store, task| {
        store.config.resolve_profile(
            &ProfileChoice {
                profile: Some(profile.into()),
                ..Default::default()
            },
            None,
            None,
        )?;
        let old = task.profile.replace(profile.into());
        if let Some(plan) = &task.dispatch_plan
            && task.launch.is_none()
        {
            let role = if plan.plan {
                ProfileRole::Planning
            } else {
                ProfileRole::Implementation
            };
            let route = store.config.route_task_with_selection(
                task,
                &ProfileChoice::default(),
                role,
                None,
                |_| None,
            )?;
            if let Some(plan) = &mut task.dispatch_plan {
                plan.choice.profile = profile.into();
                plan.choice.model = None;
                plan.choice.effort = None;
                plan.choice.source = Some(route.route.clone());
                plan.choice.switch = None;
                plan.overrides = Some(ProfileChoice::default());
                plan.route = route;
            }
        }
        Ok(old)
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "implementer_override",
        serde_json::json!({"old": old, "new": task.profile}),
    )?;
    Ok(task)
}
