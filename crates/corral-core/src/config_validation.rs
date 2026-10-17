//! Semantic checks retain the availability of their schema and inventory dependencies.

use std::collections::{BTreeMap, BTreeSet};

use crate::profile::{HarnessCapabilities, ProfileCheck, ProfileProblem};
use crate::{Config, Harness};

pub(crate) struct SemanticInputs {
    pub tables_complete: bool,
    pub independent_checked: bool,
    pub rejected_profiles: Vec<String>,
    pub rejected_fields: Vec<Vec<String>>,
}

impl Default for SemanticInputs {
    fn default() -> Self {
        Self {
            tables_complete: true,
            independent_checked: false,
            rejected_profiles: Vec::new(),
            rejected_fields: Vec::new(),
        }
    }
}

impl SemanticInputs {
    pub fn available(&self, key: &[String]) -> bool {
        !self
            .rejected_fields
            .iter()
            .any(|bad| key.starts_with(bad) || bad.starts_with(key))
    }

    fn known(&self, supplied: &toml::Value, key: &[String]) -> bool {
        self.available(key)
            && (self.tables_complete
                || key
                    .iter()
                    .try_fold(supplied, |value, part| value.get(part))
                    .is_some())
    }

    fn harness_section(&self, config: &Config, name: &str) -> Option<&'static str> {
        if config.harnesses.contains_key(name) {
            Some("harnesses")
        } else if self.tables_complete && self.available(&["harnesses".into(), name.into()]) {
            // A fallback is selected only when absence of the preferred harness is established.
            Some("agents")
        } else {
            None
        }
    }

    pub fn profiles_complete(&self) -> bool {
        self.tables_complete
            && !self
                .rejected_fields
                .iter()
                .any(|key| key.len() == 1 && matches!(key[0].as_str(), "profiles" | "agents"))
    }

    pub fn profile_problem_available(
        &self,
        config: &Config,
        supplied: &toml::Value,
        problem: &ProfileProblem,
    ) -> bool {
        if !self.known(supplied, &problem.key) {
            return false;
        }
        let Some(name) = &problem.profile else {
            return true;
        };
        let harness = &config.profiles[name].harness;
        if !self.known(
            supplied,
            &["profiles".into(), name.clone(), "harness".into()],
        ) {
            return false;
        }
        if matches!(problem.check, ProfileCheck::Harness) {
            return self.tables_complete;
        }
        let Some(section) = self.harness_section(config, harness) else {
            return false;
        };
        let fields: &[&str] = match problem.check {
            ProfileCheck::Model | ProfileCheck::Effort if section == "harnesses" => {
                problem.check.capability_fields()
            }
            ProfileCheck::Model | ProfileCheck::Effort | ProfileCheck::Command => &["command"],
            ProfileCheck::Independent | ProfileCheck::Harness => &[],
        };
        fields.iter().all(|field| {
            self.known(
                supplied,
                &[section.into(), harness.clone(), (*field).into()],
            )
        })
    }

    pub fn reviewer_available(&self, config: &Config, name: &str) -> bool {
        if !self.tables_complete
            || self
                .rejected_profiles
                .iter()
                .any(|rejected| rejected == name)
            || !self.available(&["profiles".into(), name.into()])
        {
            return false;
        }
        match config.profiles.get(name) {
            Some(profile) => self
                .harness_section(config, &profile.harness)
                .is_some_and(|section| self.available(&[section.into(), profile.harness.clone()])),
            None => self.available(&["agents".into(), name.into()]),
        }
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
                check: ProfileCheck::Independent,
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
                    check: ProfileCheck::Independent,
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
    for section in ["harnesses", "agents"] {
        if let Some(entries) = raw.get(section).and_then(toml::Value::as_table) {
            for (name, entry) in entries {
                if entry
                    .get("command")
                    .and_then(toml::Value::as_str)
                    .is_some_and(|command| command.trim().is_empty())
                {
                    out.push(ProfileProblem {
                        key: vec![section.into(), name.clone(), "command".into()],
                        message: "command is empty".into(),
                        profile: None,
                        check: ProfileCheck::Independent,
                    });
                }
            }
        }
    }
    if let Some(harnesses) = raw.get("harnesses").and_then(toml::Value::as_table) {
        for (name, harness) in harnesses {
            if let Some(flags) = harness.get("flags").and_then(toml::Value::as_str)
                && let Err(error) = Harness::base_flag_map(name, Some(flags))
            {
                out.push(ProfileProblem {
                    key: vec!["harnesses".into(), name.clone(), "flags".into()],
                    message: format!("{error:#}"),
                    profile: None,
                    check: ProfileCheck::Independent,
                });
            }
        }
    }
    for (name, profile) in raw
        .get("profiles")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
    {
        let Some(harness) = profile.get("harness").and_then(toml::Value::as_str) else {
            continue;
        };
        for (field, check) in [
            ("model", ProfileCheck::Model),
            ("effort", ProfileCheck::Effort),
        ] {
            let Some(value) = profile.get(field).and_then(toml::Value::as_str) else {
                continue;
            };
            let selected = raw
                .get("harnesses")
                .and_then(toml::Value::as_table)
                .and_then(|entries| entries.get(harness));
            let flags = if let Some(selected) = selected {
                let Some(table) = selected.as_table() else {
                    continue;
                };
                let fields = check.capability_fields();
                if !complete && !fields.iter().all(|field| table.contains_key(*field)) {
                    continue;
                }
                // Decode only the capability fields this operation consumes. Schema
                // errors in unrelated fields must not erase independently valid inputs.
                let fragment = toml::Value::Table(
                    fields
                        .iter()
                        .filter_map(|field| Some(((*field).to_owned(), table.get(*field)?.clone())))
                        .collect(),
                );
                let Ok(capabilities) = fragment.try_into::<HarnessCapabilities>() else {
                    // The schema pass owns diagnostics for these unavailable inputs.
                    continue;
                };
                capabilities.flag_map(harness)
            } else {
                if !complete
                    || !raw.get("harnesses").is_none_or(toml::Value::is_table)
                    || raw
                        .get("agents")
                        .and_then(|agents| agents.get(harness))
                        .and_then(toml::Value::as_table)
                        .is_none()
                {
                    continue;
                }
                Harness::base_flag_map(harness, None)
            };
            let Ok(flags) = flags else { continue };
            let rendered = if field == "model" {
                flags.render(harness, Some(value), None)
            } else {
                flags.render(harness, None, Some(value))
            };
            if let Err(error) = rendered {
                out.push(ProfileProblem {
                    key: vec!["profiles".into(), name.clone(), field.into()],
                    message: error.to_string(),
                    profile: Some(name.clone()),
                    check,
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
