//! Shared code-review selection and its read-only explanation.
use anyhow::{Context, Result, bail};
use corral_core::{Config, PolicySource, ProfileChoice, ResolvedProfile, Reviewer, Task};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapping {
    pub selection: String,
    pub profile: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub source: String,
    pub policy_source: String,
    pub warnings: Vec<String>,
}

pub(crate) fn choice(config: &Config, task: &Task, explicit: Option<&str>) -> Reviewer {
    explicit.map_or_else(|| config.task_review_policy(task).reviewer, Reviewer::parse)
}

pub(crate) fn selection_name(reviewer: &Reviewer) -> &str {
    match reviewer {
        Reviewer::None => "none",
        Reviewer::Pm => "pm",
        Reviewer::CrossVendor => "cross-vendor",
        Reviewer::Profile(name) => name,
    }
}

/// The same selector is used before dispatch and when starting a code reviewer.
pub(crate) fn resolve(
    config: &Config,
    reviewer: &Reviewer,
    implementer_vendor: Option<&str>,
) -> Result<(String, ResolvedProfile)> {
    let name = match reviewer {
        Reviewer::None => bail!("no reviewer configured"),
        Reviewer::Pm => "pm".into(),
        Reviewer::CrossVendor => config.cross_vendor_reviewer(
            implementer_vendor
                .context("cross-vendor review requires a known implementer vendor")?,
        )?,
        Reviewer::Profile(name) => {
            if !config.profile_names().contains(name) {
                bail!("unknown reviewer profile '{name}'");
            }
            name.clone()
        }
    };
    let profile = config.resolve_profile(
        &ProfileChoice {
            profile: Some(if *reviewer == Reviewer::Pm {
                config.pm.agent.clone()
            } else {
                name.clone()
            }),
            ..Default::default()
        },
        None,
        None,
    )?;
    Ok((name, profile))
}

pub(crate) fn mapping(config: &Config, task: &Task, reviewer: &Reviewer) -> ReviewMapping {
    let policy_source = if task.reviewer.is_some() {
        "task_override"
    } else {
        match config
            .review_policy_sources(task.project.as_deref(), task.review)
            .reviewer
        {
            PolicySource::Default => "none",
            PolicySource::Global => "workspace_review_config",
            PolicySource::Project => "project_review_config",
            PolicySource::Task => "task_override",
        }
    };
    let source = if task.reviewer.is_some() {
        policy_source
    } else {
        match reviewer {
            Reviewer::CrossVendor => "cross_vendor_map",
            Reviewer::Pm => "pm",
            Reviewer::None => "none",
            Reviewer::Profile(_) => policy_source,
        }
    };
    ReviewMapping {
        selection: selection_name(reviewer).into(),
        profile: match reviewer {
            Reviewer::Profile(name) => Some(name.clone()),
            Reviewer::Pm => Some(config.pm.agent.clone()),
            Reviewer::None | Reviewer::CrossVendor => None,
        },
        harness: None,
        model: None,
        vendor: None,
        source: source.into(),
        policy_source: policy_source.into(),
        warnings: Vec::new(),
    }
}

pub(crate) fn fill(
    mapping: &mut ReviewMapping,
    profile: ResolvedProfile,
    vendor: Option<&str>,
    reference_role: &str,
) {
    if profile.vendor.as_deref().is_some_and(|v| Some(v) == vendor) {
        mapping.warnings.push(format!(
            "reviewer uses the same vendor as the {reference_role}"
        ));
    }
    mapping.profile = Some(profile.profile);
    mapping.harness = Some(profile.harness);
    mapping.model = profile.model;
    mapping.vendor = profile.vendor;
}

pub(crate) fn explain(config: &Config, task: &Task, vendor: Option<&str>) -> ReviewMapping {
    let reviewer = choice(config, task, None);
    let mut mapping = mapping(config, task, &reviewer);
    if reviewer != Reviewer::None {
        match resolve(config, &reviewer, vendor) {
            Ok((_, profile)) => fill(&mut mapping, profile, vendor, "implementer"),
            Err(error) => mapping.warnings.push(format!("{error:#}")),
        }
    }
    mapping
}
