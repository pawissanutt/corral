//! Semantic choices belong to proposers/PMs; these operations only validate and persist them.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use corral_core::{
    AutoProfileSelection, PROFILE_RECORD_VERSION, Profile, ProfileChoice, ProfileRole, Store, Task,
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SelectionCatalog {
    pub version: u32,
    pub task: Task,
    pub role: ProfileRole,
    pub spec_revision: String,
    pub catalog_revision: String,
    pub config_revision: String,
    pub eligible: BTreeMap<String, SelectionProfile>,
    pub excluded: BTreeMap<String, Vec<String>>,
    /// No provider, account or execution probe is implied by configuration eligibility.
    pub access_status: &'static str,
    /// Implementation-review selection: the exact subject, contract floor and policy to copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<crate::review_receipts::ReviewSelectionView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectionProfile {
    pub profile_revision: String,
    #[serde(flatten)]
    pub profile: Profile,
}

/// Read fresh task/configuration identities and complete custom guidance without model calls.
pub fn selection_catalog(store: &Store, id: &str, role: ProfileRole) -> Result<SelectionCatalog> {
    supported_role(role)?;
    let fresh = store.reopen_unobserved()?;
    let task = fresh.get(id)?;
    let config = &fresh.config;
    let review = review_view(&fresh, &task, role)?;
    let mut eligible = BTreeMap::new();
    let mut excluded = BTreeMap::new();
    for key in config.profile_names() {
        let capable = review.as_ref().is_none_or(|view| {
            config.review_capability_ok(&key, view.requirement.capability)
                && (role != ProfileRole::PlanReview
                    || corral_core::plan_policy::independent_reviewer(config, &task, &key).is_ok())
        });
        let candidate = selectable(&fresh, &task, role, &key).and_then(|profile| {
            ensure!(
                capable,
                "profile '{key}' lacks the required review capability or independence"
            );
            Ok(profile)
        });
        match candidate {
            Ok(profile) => {
                let revision = profile
                    .metadata
                    .as_ref()
                    .map_or("legacy", |metadata| metadata.revision.as_str());
                eligible.insert(
                    key,
                    SelectionProfile {
                        profile_revision: revision.into(),
                        profile,
                    },
                );
            }
            Err(error) => {
                excluded.insert(key, vec![format!("{error:#}")]);
            }
        }
    }
    Ok(SelectionCatalog {
        version: PROFILE_RECORD_VERSION,
        spec_revision: corral_core::plan_contract_revision(&task)?,
        catalog_revision: config.profile_catalog_revision()?,
        config_revision: config.profile_setup_revision()?,
        task,
        role,
        eligible,
        excluded,
        access_status: "not_checked",
        review,
    })
}

fn review_view(
    store: &Store,
    task: &Task,
    role: ProfileRole,
) -> Result<Option<crate::review_receipts::ReviewSelectionView>> {
    let ctx = crate::Ctx::from_store(store.clone());
    match role {
        ProfileRole::ImplementationReview => {
            let run = crate::review::current_run(task)?;
            crate::review_receipts::implementation_view(&ctx, task, &run).map(Some)
        }
        ProfileRole::PlanReview => {
            let attempt = crate::plan_review::next_plan_attempt(&ctx, task)?;
            crate::review_receipts::plan_view(&ctx, task, &attempt).map(Some)
        }
        ProfileRole::Implementation | ProfileRole::Planning | ProfileRole::Pm => Ok(None),
    }
}

/// Save an Auto receipt, preserving its actor and all existing pins/authority.
///
/// Revalidate even an idempotent redelivery: a historical receipt is not a current choice.
pub fn selection_save(
    store: &Store,
    receipt: &AutoProfileSelection,
) -> Result<AutoProfileSelection> {
    supported_role(receipt.role)?;
    let fresh = store.reopen_unobserved()?;
    let task = fresh.get(&receipt.task)?;
    receipt.validate(&fresh.config, &task)?;
    selectable(&fresh, &task, receipt.role, &receipt.profile)?;
    let view = review_view(&fresh, &task, receipt.role)?;
    let run = crate::review::current_run(&task).ok();
    let family = run.as_ref().and_then(|r| {
        let vendor = r.vendor.clone().or_else(|| fresh.config.run_vendor(r).ok());
        r.family(vendor.as_deref())
    });
    if let Some(view) = &view {
        let ctx = crate::Ctx::from_store(fresh.clone());
        if receipt.role == ProfileRole::PlanReview {
            crate::review_receipts::check_plan(&ctx, &task, receipt, view)?;
        } else {
            crate::review_receipts::check(
                &ctx,
                &task,
                run.as_ref()
                    .context("implementation review needs a current run")?,
                receipt,
                &view.requirement,
                Some(&view.subject),
            )?;
        }
    }
    let context = view
        .as_ref()
        .map(|view| corral_core::ReviewSelectionContext {
            task: &task,
            subject: &view.subject,
            author_family: family.as_ref(),
        });
    if let Some(alternative) = &receipt.closest_alternative {
        selectable(&fresh, &task, receipt.role, alternative)?;
    }
    fresh.save_auto_profile_selection(receipt, context.as_ref())?;
    Ok(receipt.clone())
}

fn supported_role(role: ProfileRole) -> Result<()> {
    ensure!(
        matches!(
            role,
            ProfileRole::Implementation
                | ProfileRole::Planning
                | ProfileRole::ImplementationReview
                | ProfileRole::PlanReview
        ),
        "selection supports implementation, planning, implementation review and plan review"
    );
    Ok(())
}

fn selectable(store: &Store, task: &Task, role: ProfileRole, key: &str) -> Result<Profile> {
    let config = &store.config;
    if let Some(metadata) = config
        .profiles
        .get(key)
        .and_then(|profile| profile.metadata.as_ref())
    {
        let eligibility = &metadata.eligibility;
        ensure!(eligibility.enabled, "profile '{key}' is disabled");
        ensure!(
            eligibility.automatic,
            "profile '{key}' is not enabled for Auto"
        );
        ensure!(
            eligibility.roles.contains(&role),
            "profile '{key}' is not eligible for {role:?}"
        );
        ensure!(
            eligibility.projects.is_empty()
                || task
                    .project
                    .as_ref()
                    .is_some_and(|project| eligibility.projects.contains(project)),
            "profile '{key}' is outside the task's project scope"
        );
    }
    config.check_profile_eligibility(key, role, task.project.as_deref(), true)?;
    let resolved = config.resolve_profile(
        &ProfileChoice {
            profile: Some(key.into()),
            ..Default::default()
        },
        None,
        None,
    )?;
    if let Some(reason) = crate::routing::known_model_unavailability(
        &crate::Ctx::from_store(store.clone()),
        &resolved,
    ) {
        anyhow::bail!("{reason}");
    }
    if let Some(profile) = config.profiles.get(key) {
        if let Some(metadata) = &profile.metadata {
            metadata.validate(config, profile)?;
        }
        return Ok(profile.clone());
    }
    // Legacy agents remain usable and explicitly lack semantic guidance/evidence.
    Ok(Profile {
        harness: resolved.harness,
        model: resolved.model,
        vendor: resolved.vendor,
        effort: resolved.effort,
        args: Vec::new(),
        escalate_to: None,
        metadata: None,
    })
}
