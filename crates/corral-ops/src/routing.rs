//! Task-aware profile selection shared by dispatch and its read-only explanation.

use std::{collections::BTreeMap, fmt};

use anyhow::{Result, bail};
use corral_core::{ProfileChoice, Reviewer, RouteDecision, RouteSource, Task};
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
    let mut implementer = if choice.profile.is_none()
        && choice.model.is_none()
        && choice.effort.is_none()
        && let Some(plan) = &task.dispatch_plan
    {
        plan.route.clone()
    } else {
        route_task(ctx, task, choice)?
    };
    explain_route(ctx, task, &mut implementer, plan, None)
}

/// Explains an implementation or planning run using its launch identity, not today's routing rules.
/// Reviewer sessions, when present, retain their recorded identity too.
pub fn recorded_route(ctx: &Ctx, task: &Task) -> Result<RouteExplanation> {
    if task.current_run().is_none()
        && let Some(run) = task.plan_runs.last()
    {
        let mut planner = RouteDecision {
            profile: run.profile.clone(),
            harness: run.harness.clone(),
            model: run.model.clone(),
            effort: run.effort.clone(),
            route: RouteSource::Override,
            warnings: Vec::new(),
        };
        let mut explanation =
            explain_route(ctx, task, &mut planner, true, Some(run.vendor.as_deref()))?;
        if let Some(planner) = &mut explanation.review.planner {
            planner.source = PlannerSource::Run {
                run: run.id.clone(),
            };
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
        explanation.review.reviewer.warnings.clear();
        if reviewer.vendor.is_some() && reviewer.vendor == run.vendor {
            explanation
                .review
                .reviewer
                .warnings
                .push("reviewer uses the same vendor as the implementer".into());
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
            .push("implementer fell back to the default profile; no routing rule matched".into());
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
    let mut review = ReviewExplanation {
        reviewer: review_routing::explain(config, task, vendor),
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
        let reference_vendor = match identity {
            Some(planner) => planner.vendor,
            None => vendor,
        };
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
            crate::plan::expected_reviewer_for(config, task, identity, vendor)
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
                match review_routing::resolve(config, &reviewer, reference_vendor) {
                    Ok((name, profile)) => {
                        mapping.selection = name;
                        review_routing::fill(
                            &mut mapping,
                            profile,
                            reference_vendor,
                            reference_role,
                        );
                    }
                    Err(error) => mapping.warnings.push(format!("{error:#}")),
                }
            }
            Err(error) => {
                if selected != Reviewer::None
                    && let Ok((_, profile)) =
                        review_routing::resolve(config, &selected, reference_vendor)
                {
                    review_routing::fill(&mut mapping, profile, reference_vendor, reference_role);
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
    let mut catalogs = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut decision = ctx.store.config.route_task(task, choice, |profile| {
        let model = profile.model.as_deref()?;
        let catalog = catalogs
            .entry(profile.harness.clone())
            .or_insert_with(|| model_catalog(ctx, &profile.harness));
        match catalog {
            ModelCatalog::Known(models) => rejection(&profile.harness, model, models),
            ModelCatalog::Unknown(reason) => {
                if !warnings.contains(reason) {
                    warnings.push(reason.clone());
                }
                None
            }
        }
    })?;
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
            if let Some(reason) = rejection(&decision.harness, model, &models) {
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

fn rejection(harness: &str, model: &str, models: &[String]) -> Option<String> {
    if models.iter().any(|listed| listed == model) {
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
        if task.dispatch_plan.is_some() {
            let route = store.config.route_task(
                task,
                &ProfileChoice {
                    profile: Some(profile.into()),
                    ..Default::default()
                },
                |_| None,
            )?;
            if let Some(plan) = &mut task.dispatch_plan {
                plan.choice.profile = profile.into();
                plan.choice.model = None;
                plan.choice.effort = None;
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
