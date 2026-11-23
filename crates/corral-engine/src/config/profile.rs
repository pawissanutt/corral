use corral_types::ConfigProblem;
use serde::{Deserialize, Serialize};

use super::{Harness, problem};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Vec<ModelSelector>>,
}

/// An exact Pi model selector. Pi owns provider and catalog validation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ModelSelector(String);

impl ModelSelector {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ModelSelector {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value
            .split_once('/')
            .is_none_or(|(provider, model)| provider.is_empty() || model.is_empty())
            || value.contains(|ch: char| ch.is_whitespace() || ch == '\0')
        {
            return Err("expected an exact provider/model selector without whitespace or NUL");
        }
        Ok(Self(value))
    }
}

impl Profile {
    pub(crate) fn fallback_args(&self) -> Vec<&str> {
        match &self.fallback {
            Some(list) if list.is_empty() => vec!["--no-fallback"],
            Some(list) => list
                .iter()
                .flat_map(|selector| ["--fallback", selector.as_str()])
                .collect(),
            None => Vec::new(),
        }
    }

    pub(super) fn fallback_harness_error(&self) -> Option<&'static str> {
        (self.fallback.is_some() && self.harness != "pi")
            .then_some("fallback is only supported by the Pi harness")
    }

    pub(super) fn validate(
        &self,
        name: &str,
        harness: Option<&Harness>,
        problems: &mut Vec<ConfigProblem>,
    ) {
        if let Some(harness) = harness {
            if self.model.is_some() && harness.model_flag.is_none() {
                problems.push(problem(
                    format!("profiles.{name}.model"),
                    "harness has no model_flag",
                ));
            }
            if self.effort.is_some() && harness.effort_flag.is_none() {
                problems.push(problem(
                    format!("profiles.{name}.effort"),
                    "harness has no effort_flag",
                ));
            }
        } else {
            problems.push(problem(
                format!("profiles.{name}.harness"),
                format!("unknown harness {:?}", self.harness),
            ));
        }
        if let Some(error) = self.fallback_harness_error() {
            problems.push(problem(format!("profiles.{name}.fallback"), error));
        }
        for (key, invalid) in [
            ("args", self.args.iter().any(|arg| arg.contains('\0'))),
            (
                "model",
                self.model.as_ref().is_some_and(|arg| arg.contains('\0')),
            ),
            (
                "effort",
                self.effort.as_ref().is_some_and(|arg| arg.contains('\0')),
            ),
        ] {
            if invalid {
                problems.push(problem(format!("profiles.{name}.{key}"), "contains NUL"));
            }
        }
    }
}
