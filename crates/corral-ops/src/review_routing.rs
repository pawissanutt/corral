//! Shared code-review selection and its read-only explanation.
use anyhow::{Context, Result, bail};
use corral_core::{
    Config, ModelFamily, PolicySource, ProfileChoice, ResolvedProfile, Reviewer, Task,
};
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
    /// The `[[routing.reviewer_rules]]` entry that chose the reviewer, e.g. "reviewer rule 1 (small)".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
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
        Reviewer::Auto => "auto",
        Reviewer::Profile(name) => name,
    }
}

/// Under `auto`, the profile mapped to the task's floor before candidate paths are known.
pub(crate) fn auto_profile(config: &Config, task: &Task) -> Result<String> {
    let signals =
        corral_core::ReviewSignals::for_task(task, corral_core::ReviewSubjectKind::Implementation);
    let kind = corral_core::review_requirement(&signals).contract.kind;
    config
        .review_profile_for(kind)
        .map(|(_, profile)| profile.to_string())
        .with_context(|| format!("no routing.review_profiles entry for {}", kind.as_str()))
}

/// The same selector is used before dispatch and when starting a code reviewer.
pub(crate) fn resolve(
    config: &Config,
    reviewer: &Reviewer,
    implementer: Option<&ModelFamily>,
) -> Result<(String, ResolvedProfile)> {
    let name = match reviewer {
        Reviewer::None => bail!("no reviewer configured"),
        Reviewer::Auto => {
            bail!("automatic review routing selects its profile from the review contract")
        }
        Reviewer::Pm => "pm".into(),
        Reviewer::CrossVendor => config.cross_vendor_reviewer(
            implementer.context("cross-vendor review requires a known implementer vendor")?,
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
    let rule = config.reviewer_rule(task);
    let policy_source = if task.reviewer.is_some() {
        "task_override"
    } else if rule.is_some() {
        "reviewer_rule"
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
            Reviewer::Auto => "auto_mapping",
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
            Reviewer::None | Reviewer::CrossVendor | Reviewer::Auto => None,
        },
        harness: None,
        model: None,
        vendor: None,
        source: source.into(),
        policy_source: policy_source.into(),
        rule: rule.map(|(index, rule)| match &rule.name {
            Some(name) => format!("reviewer rule {index} ({name})"),
            None => format!("reviewer rule {index}"),
        }),
        warnings: Vec::new(),
    }
}

pub(crate) fn fill(
    mapping: &mut ReviewMapping,
    profile: ResolvedProfile,
    family: Option<&ModelFamily>,
    reference_role: &str,
) {
    if profile.family().is_some_and(|f| Some(&f) == family) {
        mapping.warnings.push(format!(
            "reviewer uses the same model family as the {reference_role}"
        ));
    }
    mapping.profile = Some(profile.profile);
    mapping.harness = Some(profile.harness);
    mapping.model = profile.model;
    mapping.vendor = profile.vendor;
}

pub(crate) fn explain(config: &Config, task: &Task, family: Option<&ModelFamily>) -> ReviewMapping {
    let reviewer = choice(config, task, None);
    let mut mapping = mapping(config, task, &reviewer);
    if reviewer != Reviewer::None {
        let effective = match &reviewer {
            Reviewer::Auto => auto_profile(config, task).map(Reviewer::Profile),
            other => Ok(other.clone()),
        };
        match effective.and_then(|effective| resolve(config, &effective, family)) {
            Ok((_, profile)) => {
                if let Err(error) = config
                    .task_review_policy(task)
                    .check_reviewer_family(family, profile.family().as_ref())
                {
                    mapping.warnings.push(format!("{error:#}"));
                }
                fill(&mut mapping, profile, family, "implementer");
            }
            Err(error) => mapping.warnings.push(format!("{error:#}")),
        }
    }
    mapping
}
