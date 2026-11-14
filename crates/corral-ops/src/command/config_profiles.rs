//! App profile mutations clone server-owned configuration and share removal truth with settings.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use corral_core::backend::{ApplyError, HeadQuery};
use corral_core::config_doc::toml_edit::Key;
use corral_core::{Config, ConfigDoc, ConfigLayer, ConfigLayers, ConfigSource, RunHead, Store};
use corral_protocol::rows::ProfileChoice;

fn check_hash(doc: &ConfigDoc, expected: &str) -> Result<()> {
    if super::config_hash(doc) != expected {
        return Err(ApplyError::Precondition(
            "workspace configuration changed; reload settings".into(),
        )
        .into());
    }
    Ok(())
}

pub(super) fn add(
    store: &Store,
    name: &str,
    from: &str,
    model: Option<String>,
    effort: Option<String>,
    expected_hash: &str,
) -> Result<()> {
    store.edit_config_layer_from(ConfigLayer::Workspace, ConfigSource::SettingsUi, |doc| {
        check_hash(doc, expected_hash)?;
        // Both the template and uniqueness are read under the committing dispatch/config locks.
        let base = store.config_layers()?.config()?;
        let name = name.trim();
        ensure!(!name.is_empty(), "type a profile name");
        ensure!(
            name.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            "{name:?}: use letters, digits, '-' and '_' only"
        );
        ensure!(
            !base.profiles.contains_key(name) && !base.agents.contains_key(name),
            "profile {name} already exists"
        );
        let mut profile = base
            .profiles
            .get(from)
            .with_context(|| format!("profile {from:?} is not a configured clone source"))?
            .clone();
        if let Some(model) = model {
            profile.model = Some(model);
        }
        if let Some(effort) = effort {
            profile.effort = Some(effort);
        }
        let mut draft = base.clone();
        draft.profiles.insert(name.to_owned(), profile);
        doc.apply_config_changes(&base, &draft)
    })?;
    Ok(())
}

pub(super) fn remove(store: &Store, name: &str, expected_hash: &str) -> Result<()> {
    store.edit_config_layer_from(ConfigLayer::Workspace, ConfigSource::SettingsUi, |doc| {
        check_hash(doc, expected_hash)?;
        let layers = store.config_layers()?;
        let base = layers.config()?;
        ensure!(
            base.profiles.contains_key(name),
            "profile {name:?} does not exist"
        );
        let personal = personal_doc(&layers)?;
        let (workspace, inherited) = layers_for(doc, &personal, name)?;
        // Config layers have no tombstones: removing an override must not claim to delete
        // the profile when the personal definition will immediately become effective again.
        if let Some(reason) = removal_refusal(workspace, inherited, &[]) {
            bail!("{reason}");
        }
        let mut references = references(store, &base)?;
        let refs = references.remove(name).unwrap_or_default();
        if let Some(reason) = removal_refusal(workspace, inherited, &refs) {
            bail!("{reason}");
        }
        let mut draft = base.clone();
        draft.profiles.remove(name);
        // The canonical writer validates the final merged configuration before publishing.
        doc.apply_config_changes(&base, &draft)
    })?;
    Ok(())
}

fn personal_doc(layers: &ConfigLayers) -> Result<ConfigDoc> {
    if layers.path(ConfigLayer::User).is_ok() {
        layers.doc(ConfigLayer::User)
    } else {
        Ok(ConfigDoc::default())
    }
}

fn layers_for(workspace: &ConfigDoc, personal: &ConfigDoc, name: &str) -> Result<(bool, bool)> {
    let key = format!("profiles.{}", Key::new(name));
    Ok((
        workspace.get(&key)?.is_some(),
        personal.get(&key)?.is_some(),
    ))
}

fn removal_refusal(workspace: bool, inherited: bool, refs: &[String]) -> Option<String> {
    if inherited || !workspace {
        Some("profile_inherited: profile is inherited from personal/default configuration; workspace removal cannot delete it".into())
    } else if !refs.is_empty() {
        Some(format!(
            "profile_in_use: referenced by {}; change those first",
            refs.join(", ")
        ))
    } else {
        None
    }
}

/// The same exact configured catalog and restrictions consumed by the committing operations.
/// This projection is descriptive, never authority: references can change before confirmation.
pub(crate) fn profile_choices(
    store: &Store,
    layers: &ConfigLayers,
    config: &Config,
) -> Result<Vec<ProfileChoice>> {
    let workspace = layers.doc(ConfigLayer::Workspace)?;
    let personal = personal_doc(layers)?;
    let mut refs = references(store, config)?;
    config
        .profiles
        .iter()
        .map(|(name, profile)| {
            let (workspace, inherited) = layers_for(&workspace, &personal, name)?;
            let references = refs.remove(name).unwrap_or_default();
            Ok(ProfileChoice {
                name: name.clone(),
                harness: profile.harness.clone(),
                model: profile.model.clone(),
                effort: profile.effort.clone(),
                workspace,
                inherited,
                remove_refusal: removal_refusal(workspace, inherited, &references),
                references,
            })
        })
        .collect()
}

/// All configured selectors plus live task/session bindings, without loading historical runs.
fn references(store: &Store, config: &Config) -> Result<BTreeMap<String, Vec<String>>> {
    let mut refs = BTreeMap::<String, Vec<String>>::new();
    let mut record = |profile: &str, at: String| {
        if config.profiles.contains_key(profile) {
            refs.entry(profile.to_owned()).or_default().push(at);
        }
    };
    record(
        config
            .defaults
            .profile
            .as_deref()
            .unwrap_or(corral_core::FALLBACK_PROFILE),
        "defaults.profile".into(),
    );
    record(&config.pm.agent, "pm.agent".into());
    if let Some(profile) = &config.assistant.agent {
        record(profile, "assistant.agent".into());
    }
    if let Some(profile) = &config.lessons.profile {
        record(profile, "lessons.profile".into());
    }
    if let Some(profile) = &config.review.reviewer {
        record(profile, "review.reviewer".into());
    }
    for (name, project) in &config.projects {
        if let Some(profile) = &project.default_profile {
            record(profile, format!("projects.{name}.default_profile"));
        }
        if let Some(profile) = &project.review.reviewer {
            record(profile, format!("projects.{name}.review.reviewer"));
        }
    }
    for (name, profile) in &config.profiles {
        if let Some(target) = &profile.escalate_to {
            record(target, format!("profiles.{name}.escalate_to"));
        }
    }
    for (role, profile) in &config.routing.role_profiles {
        record(profile, format!("routing.role_profiles.{role:?}"));
    }
    for (contract, profile) in &config.routing.review_profiles {
        record(profile, format!("routing.review_profiles.{contract:?}"));
    }
    for (vendor, profile) in &config.routing.reviewer_by_vendor {
        record(profile, format!("routing.reviewer_by_vendor.{vendor}"));
    }
    for (index, rule) in config.routing.rules.iter().enumerate() {
        record(&rule.profile, format!("routing.rules[{index}].profile"));
    }
    for (index, rule) in config.routing.reviewer_rules.iter().enumerate() {
        record(
            &rule.profile,
            format!("routing.reviewer_rules[{index}].profile"),
        );
    }
    for task in store.heads_checked(&HeadQuery::Open { project: None })? {
        if let Some(profile) = &task.profile {
            record(profile, format!("{}.profile", task.id));
        }
        if let Some(profile) = &task.reviewer {
            record(profile, format!("{}.reviewer", task.id));
        }
        if let Some(plan) = &task.dispatch_plan {
            record(
                &plan.choice.profile,
                format!("{}.dispatch_plan.choice", task.id),
            );
            record(
                &plan.route.profile,
                format!("{}.dispatch_plan.route", task.id),
            );
        }
        if let Some(run) = task.current_run.as_deref() {
            run_references(run, &task.id, &mut record);
        }
        if let Some(run) = task.last_plan_run.as_deref() {
            record(&run.profile, format!("{}.{}.profile", task.id, run.id));
            if let Some(execution) = &run.execution {
                record(
                    &execution.resolved.profile,
                    format!("{}.{}.execution", task.id, run.id),
                );
            }
        }
        if let Some(review) = task
            .last_plan
            .as_ref()
            .and_then(|plan| plan.current_review.as_ref())
        {
            record(
                &review.reviewer,
                format!("{}.{}.reviewer", task.id, review.id),
            );
        }
    }
    for session in store.list_assistants()? {
        if session.ended.is_none() {
            record(&session.profile, format!("{}.profile", session.id));
        }
    }
    for values in refs.values_mut() {
        values.sort();
        values.dedup();
    }
    Ok(refs)
}

fn run_references(run: &RunHead, task: &str, record: &mut impl FnMut(&str, String)) {
    record(
        run.profile.as_deref().unwrap_or(&run.agent),
        format!("{task}.{}.profile", run.id),
    );
    if let Some(execution) = &run.execution {
        record(
            &execution.resolved.profile,
            format!("{task}.{}.execution", run.id),
        );
    }
    if let Some(retry) = &run.retry {
        record(&retry.profile, format!("{task}.{}.retry", run.id));
    }
    if let Some(child) = run.current_child.as_deref() {
        run_references(child, task, record);
    }
}
