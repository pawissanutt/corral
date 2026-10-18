//! Ordered task routing. Matching fields are ANDed; alternatives within a field are ORed.

use std::{collections::BTreeMap, fmt};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{Config, FALLBACK_PROFILE, ProfileChoice, ResolvedProfile, Task, TaskKind, TaskSize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routing {
    /// Owner-approved role choices; launch semantics beyond existing roles are separate.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub role_profiles: BTreeMap<crate::ProfileRole, String>,
    /// Opt-in staged repair policy; absent preserves the existing escalation/review limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<crate::RepairPolicy>,
    #[serde(default)]
    pub rules: Vec<RoutingRule>,
    /// Failed attempts before automatic escalation; zero disables it.
    #[serde(
        default = "default_escalate_after",
        skip_serializing_if = "is_default_escalate_after"
    )]
    pub escalate_after: u32,
    /// Reviewer vendor -> profile (not implementer vendor -> profile).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reviewer_by_vendor: BTreeMap<String, String>,
}

fn default_escalate_after() -> u32 {
    1
}

fn is_default_escalate_after(value: &u32) -> bool {
    *value == default_escalate_after()
}

impl Default for Routing {
    fn default() -> Self {
        Self {
            role_profiles: BTreeMap::new(),
            repair: None,
            rules: Vec::new(),
            escalate_after: default_escalate_after(),
            reviewer_by_vendor: BTreeMap::new(),
        }
    }
}

impl Routing {
    pub fn is_empty(&self) -> bool {
        self.role_profiles.is_empty()
            && self.repair.is_none()
            && self.rules.is_empty()
            && is_default_escalate_after(&self.escalate_after)
            && self.reviewer_by_vendor.is_empty()
    }
}

fn alternatives<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        One(T),
        Many(Vec<T>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(value) => vec![value],
        OneOrMany::Many(values) => values,
    })
}

/// Empty match fields impose no restriction. A rule with no fields is a catch-all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "alternatives",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub project: Vec<String>,
    #[serde(
        default,
        deserialize_with = "alternatives",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub kind: Vec<TaskKind>,
    #[serde(
        default,
        deserialize_with = "alternatives",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub size: Vec<TaskSize>,
    #[serde(default, alias = "title", skip_serializing_if = "Option::is_none")]
    pub title_regex: Option<String>,
    #[serde(default, alias = "body", skip_serializing_if = "Option::is_none")]
    pub body_regex: Option<String>,
    pub profile: String,
}

impl RoutingRule {
    pub fn matches(&self, task: &Task) -> Result<bool> {
        let title = self
            .title_regex
            .as_deref()
            .map(Regex::new)
            .transpose()
            .context("invalid routing title regex")?;
        let body = self
            .body_regex
            .as_deref()
            .map(Regex::new)
            .transpose()
            .context("invalid routing body regex")?;
        Ok((self.project.is_empty()
            || task
                .project
                .as_ref()
                .is_some_and(|p| self.project.contains(p)))
            && (self.kind.is_empty()
                || task
                    .kind
                    .as_deref()
                    .is_some_and(|value| self.kind.iter().any(|kind| kind.as_str() == value)))
            && (self.size.is_empty() || task.size.is_some_and(|s| self.size.contains(&s)))
            && title.is_none_or(|r| r.is_match(&task.title))
            && body.is_none_or(|r| r.is_match(&task.body)))
    }
}

/// Stable provenance persisted with a run. Rule indexes are one-based config order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RouteSource {
    Override,
    Task,
    Rule { index: usize, name: Option<String> },
    ProjectDefault,
    Defaults,
    Fallback,
}

impl fmt::Display for RouteSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Override => f.write_str("dispatch override"),
            Self::Task => f.write_str("task profile"),
            Self::Rule { index, name } => {
                write!(f, "routing rule {index}")?;
                if let Some(name) = name {
                    write!(f, " ({name})")?;
                }
                Ok(())
            }
            Self::ProjectDefault => f.write_str("project default_profile"),
            Self::Defaults => f.write_str("[defaults] profile"),
            Self::Fallback => f.write_str("fallback omo"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteDecision {
    pub profile: String,
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub route: RouteSource,
    pub warnings: Vec<String>,
}

impl Config {
    /// Resolves task routing using caller-supplied availability (None means available).
    /// Explicit overrides are retained for explanation; dispatch validates the final model separately.
    /// Automatic unavailable choices fall through.
    pub fn route_task(
        &self,
        task: &Task,
        choice: &ProfileChoice,
        mut unavailable: impl FnMut(&ResolvedProfile) -> Option<String>,
    ) -> Result<RouteDecision> {
        let mut candidates = Vec::new();
        if let Some(profile) = &choice.profile {
            candidates.push((profile.clone(), RouteSource::Override, None));
        } else {
            if let Some(profile) = &task.profile {
                candidates.push((profile.clone(), RouteSource::Task, None));
            }
            for (index, rule) in self.routing.rules.iter().enumerate() {
                candidates.push((
                    rule.profile.clone(),
                    RouteSource::Rule {
                        index: index + 1,
                        name: rule.name.clone(),
                    },
                    Some(rule),
                ));
            }
            if let Some(profile) = task
                .project
                .as_deref()
                .and_then(|p| self.projects.get(p))
                .and_then(|p| p.default_profile.as_ref())
            {
                candidates.push((profile.clone(), RouteSource::ProjectDefault, None));
            }
            if let Some(profile) = &self.defaults.profile {
                candidates.push((profile.clone(), RouteSource::Defaults, None));
            }
            candidates.push((FALLBACK_PROFILE.to_string(), RouteSource::Fallback, None));
        }
        let explicit = choice.profile.is_some() || choice.model.is_some();
        let mut warnings = Vec::new();
        for (profile, source, rule) in candidates {
            if let Some(rule) = rule
                && !rule.matches(task).with_context(|| source.to_string())?
            {
                continue;
            }
            let resolved = self.resolve_profile(
                &ProfileChoice {
                    profile: Some(profile),
                    model: choice.model.clone(),
                    effort: choice.effort.clone(),
                },
                None,
                None,
            )?;
            if let Some(reason) = unavailable(&resolved) {
                warnings.push(format!(
                    "{}: profile '{}': {reason}{}",
                    source,
                    resolved.profile,
                    if explicit {
                        "; keeping explicit override"
                    } else {
                        "; skipped"
                    }
                ));
                if !explicit {
                    continue;
                }
            }
            return Ok(RouteDecision {
                profile: resolved.profile,
                harness: resolved.harness,
                model: resolved.model,
                effort: resolved.effort,
                route: if choice.model.is_some() || choice.effort.is_some() {
                    RouteSource::Override
                } else {
                    source
                },
                warnings,
            });
        }
        bail!("no available routing profile: {}", warnings.join("; "))
    }
}
