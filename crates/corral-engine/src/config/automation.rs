use std::{collections::BTreeMap, path::Path};

use anyhow::{Result, bail};
use corral_types::ConfigProblem;
use serde::{Deserialize, Serialize};

use super::{Config, problem};

mod cron;
pub use cron::matches_minute;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Automation {
    pub agents: BTreeMap<String, AgentSpec>,
    pub rules: BTreeMap<String, Rule>,
}

impl Automation {
    pub(super) fn is_empty(&self) -> bool {
        self.agents.is_empty() && self.rules.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec {
    pub profile: String,
    pub instructions: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub agent: String,
    #[serde(default)]
    pub enabled: bool,
    pub event: Option<String>,
    pub cron: Option<String>,
}

impl Config {
    pub(super) fn validate_loaded_automation(self, path: &Path) -> Result<Self> {
        let mut problems = Vec::new();
        validate(&self, &mut problems);
        if !problems.is_empty() {
            let details = problems.iter().map(ToString::to_string).collect::<Vec<_>>();
            bail!(
                "invalid configuration {}:\n{}",
                path.display(),
                details.join("\n")
            );
        }
        Ok(self)
    }
}

pub(super) fn validate(config: &Config, problems: &mut Vec<ConfigProblem>) {
    for (id, spec) in &config.automation.agents {
        let key = format!("automation.agents.{id}");
        check_id(id, &key, problems);
        let profile_error = match config.profiles.get(&spec.profile) {
            None => Some(format!("unknown profile {:?}", spec.profile)),
            Some(profile) if profile.harness != "pi" => Some(format!(
                "profile {:?} must use the pi harness",
                spec.profile
            )),
            Some(_) => None,
        };
        if let Some(error) = profile_error {
            problems.push(problem(format!("{key}.profile"), error));
        }
        if !(1..=8192).contains(&spec.instructions.trim().len()) {
            problems.push(problem(
                format!("{key}.instructions"),
                "must contain 1–8192 bytes after trimming whitespace",
            ));
        }
    }
    for (id, rule) in &config.automation.rules {
        let key = format!("automation.rules.{id}");
        check_id(id, &key, problems);
        if !config.automation.agents.contains_key(&rule.agent) {
            problems.push(problem(
                format!("{key}.agent"),
                format!("unknown agent {:?}", rule.agent),
            ));
        }
        match (rule.event.as_deref(), rule.cron.as_deref()) {
            (Some("blocked"), None) => {}
            (Some(_), None) => {
                problems.push(problem(format!("{key}.event"), "must be \"blocked\""))
            }
            (None, Some(expression)) => {
                if let Err(error) = matches_minute(expression, 0) {
                    problems.push(problem(format!("{key}.cron"), error.to_string()));
                }
            }
            _ => problems.push(problem(key, "set exactly one of event or cron")),
        }
    }
}

fn check_id(id: &str, key: &str, problems: &mut Vec<ConfigProblem>) {
    let mut bytes = id.bytes();
    if id.len() > 40
        || !bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        || !bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        problems.push(problem(key, "ID must match [a-z][a-z0-9_-]{0,39}"));
    }
}

#[cfg(test)]
#[path = "automation_tests.rs"]
mod tests;
