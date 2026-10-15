//! Task-aware profile selection shared by dispatch and its read-only explanation.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use corral_core::{ProfileChoice, RouteDecision, Task};

use crate::{Ctx, ModelSource, harness, harness_report};

/// Explains a task's next run without creating a worktree, pane, or run record.
pub fn route(ctx: &Ctx, id: &str, choice: &ProfileChoice) -> Result<RouteDecision> {
    route_task(ctx, &ctx.store.get(id)?, choice)
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
