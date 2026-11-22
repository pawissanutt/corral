use std::collections::BTreeMap;

use corral_types::ConfigProblem;
use serde::{Deserialize, Serialize};

use super::{Config, problem};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Extension {
    /// None means every project; an explicit empty list matches none.
    pub projects: Option<Vec<String>>,
    pub enabled: bool,
    pub facts: Option<String>,
    pub on: Option<Vec<String>>,
    pub run: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, ExtensionTool>,
}

impl Default for Extension {
    fn default() -> Self {
        Self {
            projects: None,
            enabled: true,
            facts: None,
            on: None,
            run: None,
            tools: BTreeMap::new(),
        }
    }
}

impl Extension {
    /// Project scope only, independent of whether the extension is enabled.
    pub fn includes_project(&self, project: &str) -> bool {
        self.projects
            .as_ref()
            .is_none_or(|names| names.iter().any(|name| name == project))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionTool {
    pub run: String,
    pub about: Option<String>,
    #[serde(default)]
    pub record: bool,
}

pub(super) fn validate(config: &Config, problems: &mut Vec<ConfigProblem>) {
    for (name, extension) in &config.extensions {
        let key = format!("extensions.{name}");
        if name.trim().is_empty() {
            problems.push(problem("extensions", "names must not be empty"));
        }
        for project in extension.projects.iter().flatten() {
            if !config.projects.contains_key(project) {
                problems.push(problem(
                    format!("{key}.projects"),
                    format!("unknown project {project:?}"),
                ));
            }
        }
        if let Some(line) = &extension.facts {
            check_line(line, format!("{key}.facts"), problems);
        }
        if let Some(line) = &extension.run {
            check_line(line, format!("{key}.run"), problems);
        } else if extension.on.is_some() {
            problems.push(problem(format!("{key}.on"), "requires run"));
        }
        for (tool_name, tool) in &extension.tools {
            if tool_name.trim().is_empty() {
                problems.push(problem(format!("{key}.tools"), "names must not be empty"));
            }
            check_line(&tool.run, format!("{key}.tools.{tool_name}.run"), problems);
        }
    }
    for project in config.projects.keys() {
        let mut tools: BTreeMap<&str, &str> = BTreeMap::new();
        for (name, extension) in &config.extensions {
            if extension.enabled && extension.includes_project(project) {
                for tool in extension.tools.keys() {
                    if let Some(previous) = tools.insert(tool, name) {
                        problems.push(problem(
                            "extensions",
                            format!("duplicate tool {tool:?} for project {project:?}: {previous:?}, {name:?}"),
                        ));
                    }
                }
            }
        }
        let providers: Vec<_> = config
            .extensions
            .iter()
            .filter(|(_, extension)| {
                extension.enabled
                    && extension.facts.is_some()
                    && extension.includes_project(project)
            })
            .map(|(name, _)| format!("{name:?}"))
            .collect();
        if providers.len() > 1 {
            problems.push(problem(
                "extensions",
                format!(
                    "multiple enabled extensions fill the facts slot for project {project:?}: {}",
                    providers.join(", ")
                ),
            ));
        }
    }
}

fn check_line(line: &str, key: String, problems: &mut Vec<ConfigProblem>) {
    if line.trim().is_empty() || line.contains('\0') {
        problems.push(problem(key, "command must be nonempty and contain no NUL"));
    }
}
