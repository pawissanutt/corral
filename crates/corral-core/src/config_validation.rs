//! Semantic checks retain the availability of their schema and inventory dependencies.

use std::collections::{BTreeMap, BTreeSet};

use crate::profile::ProfileProblem;
use crate::{Config, Harness};

pub(crate) struct SemanticInputs {
    pub tables_complete: bool,
    pub rejected_profiles: Vec<String>,
    pub rejected_fields: Vec<Vec<String>>,
}

impl Default for SemanticInputs {
    fn default() -> Self {
        Self {
            tables_complete: true,
            rejected_profiles: Vec::new(),
            rejected_fields: Vec::new(),
        }
    }
}

impl SemanticInputs {
    pub fn available(&self, key: &[String]) -> bool {
        self.tables_complete
            && !self
                .rejected_fields
                .iter()
                .any(|bad| key.starts_with(bad) || bad.starts_with(key))
    }

    pub fn profiles_complete(&self) -> bool {
        self.tables_complete
            && !self
                .rejected_fields
                .iter()
                .any(|key| key.len() == 1 && matches!(key[0].as_str(), "profiles" | "agents"))
    }

    pub fn profile_problem_available(&self, config: &Config, problem: &ProfileProblem) -> bool {
        if !self.available(&problem.key) {
            return false;
        }
        let Some(name) = &problem.profile else {
            return true;
        };
        let harness = &config.profiles[name].harness;
        if !self.available(&["profiles".into(), name.clone(), "harness".into()])
            || !self.available(&["harnesses".into(), harness.clone(), "command".into()])
            || !self.available(&["agents".into(), harness.clone(), "command".into()])
        {
            return false;
        }
        let fields: &[&str] = match problem.key.last().map(String::as_str) {
            Some("model") => &["flags", "model_flag"],
            Some("effort") => &["flags", "effort_flag", "efforts"],
            _ => &[],
        };
        fields.iter().all(|field| {
            self.available(&["harnesses".into(), harness.clone(), (*field).into()])
                && self.available(&["agents".into(), harness.clone(), "command".into()])
        })
    }

    pub fn reviewer_available(&self, config: &Config, name: &str) -> bool {
        if self
            .rejected_profiles
            .iter()
            .any(|rejected| rejected == name)
            || !self.available(&["profiles".into(), name.into()])
            || !self.available(&["agents".into(), name.into()])
        {
            return false;
        }
        config.profiles.get(name).is_none_or(|profile| {
            self.available(&["harnesses".into(), profile.harness.clone()])
                && self.available(&["agents".into(), profile.harness.clone()])
        })
    }
}

pub(crate) fn escalation_problems(
    names: &BTreeSet<String>,
    targets: &BTreeMap<String, String>,
    complete: bool,
) -> Vec<ProfileProblem> {
    let mut out = Vec::new();
    for (name, target) in targets {
        let key = vec!["profiles".into(), name.clone(), "escalate_to".into()];
        if complete && !names.contains(target) {
            out.push(ProfileProblem {
                key: key.clone(),
                message: format!(
                    "unknown profile '{target}' (configured: {})",
                    names
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                profile: None,
            });
        }
        let mut seen = BTreeSet::new();
        let mut current = name;
        loop {
            if !seen.insert(current) {
                out.push(ProfileProblem {
                    key: key.clone(),
                    message: format!("escalation cycle at '{current}'"),
                    profile: None,
                });
                break;
            }
            match targets.get(current) {
                Some(next) => current = next,
                None => break,
            }
        }
    }
    out
}

/// These supplied fields do not depend on decoding their required-field parent.
pub(crate) fn independent_problems(raw: &toml::Value, complete: bool) -> Vec<ProfileProblem> {
    let mut out = Vec::new();
    if let Some(harnesses) = raw.get("harnesses").and_then(toml::Value::as_table) {
        for (name, harness) in harnesses {
            if let Some(flags) = harness.get("flags").and_then(toml::Value::as_str)
                && let Err(error) = Harness::base_flag_map(name, Some(flags))
            {
                out.push(ProfileProblem {
                    key: vec!["harnesses".into(), name.clone(), "flags".into()],
                    message: format!("{error:#}"),
                    profile: None,
                });
            }
        }
    }
    let names = ["profiles", "agents"]
        .iter()
        .filter_map(|section| raw.get(section)?.as_table())
        .flat_map(|table| table.keys().cloned())
        .collect();
    let targets = raw
        .get("profiles")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter_map(|(name, profile)| {
            Some((
                name.clone(),
                profile.get("escalate_to")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    let complete = complete
        && ["profiles", "agents"]
            .iter()
            .all(|section| raw.get(section).is_none_or(toml::Value::is_table));
    out.extend(escalation_problems(&names, &targets, complete));
    out
}
