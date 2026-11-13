//! Read-only build launch settings and the project's owner-selected exit policy.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildCfg {
    pub max_parallel: u8,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, BuildProfileCfg>,
}

impl Default for BuildCfg {
    fn default() -> Self {
        Self {
            max_parallel: 3,
            profiles: BTreeMap::new(),
        }
    }
}

impl BuildCfg {
    pub fn is_default(&self) -> bool {
        self.max_parallel == 3 && self.profiles.is_empty()
    }

    /// Problems independent of workspace paths or the host's provider registry.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !(1..=8).contains(&self.max_parallel) {
            out.push("build.max_parallel must be within 1..=8".into());
        }
        if self.profiles.len() > 16 {
            out.push("build.profiles must contain at most 16 profiles".into());
        }
        for (name, profile) in &self.profiles {
            let bytes = name.as_bytes();
            if !(1..=32).contains(&bytes.len())
                || !bytes[0].is_ascii_lowercase()
                || !bytes
                    .iter()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
            {
                out.push(format!(
                    "build.profiles.{name}: name must match [a-z][a-z0-9-]{{0,31}}"
                ));
            }
            if !profile.model.split_once('/').is_some_and(|(provider, id)| {
                !provider.is_empty()
                    && !id.is_empty()
                    && provider.bytes().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.'
                    })
                    && !id.chars().any(char::is_whitespace)
            }) {
                out.push(format!(
                    "build.profiles.{name}.model must be <provider>/<id>"
                ));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildProfileCfg {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildExit {
    Review,
    Gate,
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use std::path::Path;

    #[test]
    fn build_and_project_exit_round_trip() {
        let cfg: Config = toml::from_str(
            r#"
[build]
max_parallel = 5
[build.profiles.fast]
model = "anthropic/claude-opus-4-8"
effort = "high"
description = "Review changes"
[projects.app]
path = "."
build_exit = "gate"
"#,
        )
        .unwrap();
        assert_eq!(cfg.build.max_parallel, 5);
        assert_eq!(cfg.projects["app"].build_exit, Some(BuildExit::Gate));
        assert_eq!(toml::from_str::<Config>(&cfg.to_toml()).unwrap(), cfg);
        for policy in [BuildExit::Review, BuildExit::Gate, BuildExit::None] {
            let encoded = toml::Value::try_from(policy).unwrap();
            assert_eq!(encoded.try_into::<BuildExit>().unwrap(), policy);
        }
    }

    #[test]
    fn every_build_problem_joins_config_validation() {
        let mut cfg = Config::default_for(Path::new("/nonexistent-build-fixture"));
        cfg.build.max_parallel = 0;
        for i in 0..17 {
            cfg.build.profiles.insert(
                format!("profile-{i}"),
                BuildProfileCfg {
                    model: "openai-codex/gpt-6.1-sol".into(),
                    effort: None,
                    description: None,
                },
            );
        }
        cfg.build.profiles.insert(
            "Bad".into(),
            BuildProfileCfg {
                model: "missing-provider".into(),
                effort: None,
                description: None,
            },
        );
        let errors = cfg.validate(Path::new(".")).unwrap_err().0;
        for key in [
            "build.max_parallel",
            "build.profiles must",
            "build.profiles.Bad:",
            "build.profiles.Bad.model",
        ] {
            assert!(errors.iter().any(|error| error.contains(key)), "{errors:?}");
        }
        cfg.build.max_parallel = 9;
        assert!(
            cfg.build
                .problems()
                .iter()
                .any(|error| error.contains("max_parallel"))
        );
        for name in ["", "a_b", "1bad", "abcdefghijklmnopqrstuvwxyzabcdefg"] {
            cfg.build.profiles.clear();
            cfg.build.profiles.insert(
                name.into(),
                BuildProfileCfg {
                    model: "provider/id".into(),
                    effort: None,
                    description: None,
                },
            );
            assert!(
                cfg.build
                    .problems()
                    .iter()
                    .any(|error| error.contains("name must"))
            );
        }
        cfg.build.max_parallel = 3;
        for model in ["id", "/id", "provider/", "provider/id with spaces"] {
            cfg.build.profiles.clear();
            cfg.build.profiles.insert(
                "valid".into(),
                BuildProfileCfg {
                    model: model.into(),
                    effort: None,
                    description: None,
                },
            );
            assert!(
                cfg.build
                    .problems()
                    .iter()
                    .any(|error| error.contains(".model"))
            );
        }
        cfg.build.profiles.clear();
        for max in [1, 8] {
            cfg.build.max_parallel = max;
            assert!(cfg.build.problems().is_empty());
        }
    }

    #[test]
    fn old_config_serialization_is_byte_identical() {
        let cfg: Config = toml::from_str("[projects.app]\npath = \".\"\n").unwrap();
        let old = cfg.to_toml();
        assert!(!old.contains("[build]"));
        assert!(!old.contains("build_exit"));
        let parsed: Config = toml::from_str(&old).unwrap();
        assert_eq!(parsed.build, BuildCfg::default());
        assert_eq!(parsed.to_toml().as_bytes(), old.as_bytes());
    }
}
