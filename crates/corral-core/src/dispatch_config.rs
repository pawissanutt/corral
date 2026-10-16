use serde::{Deserialize, Serialize};

use crate::Config;

/// How dispatch handles eligible Ideas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromoteIdeas {
    Never,
    #[default]
    Suggest,
    Auto,
}

/// Effective dispatch selection policy after applying project overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DispatchPolicy {
    pub auto_backlog: bool,
    pub promote_ideas: PromoteIdeas,
    pub max_auto_promotions_per_day: usize,
}

impl Default for DispatchPolicy {
    fn default() -> Self {
        Self {
            auto_backlog: true,
            promote_ideas: PromoteIdeas::Suggest,
            max_auto_promotions_per_day: 3,
        }
    }
}

/// Ready admission restrictions plus global dispatch selection policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DispatchCfg {
    pub paused: bool,
    /// Empty (including omitted): all configured projects. Nonempty: only these projects.
    pub ready_projects: Vec<String>,
    pub auto_backlog: bool,
    pub promote_ideas: PromoteIdeas,
    pub max_auto_promotions_per_day: usize,
}

impl Default for DispatchCfg {
    fn default() -> Self {
        let policy = DispatchPolicy::default();
        Self {
            paused: false,
            ready_projects: Vec::new(),
            auto_backlog: policy.auto_backlog,
            promote_ideas: policy.promote_ideas,
            max_auto_promotions_per_day: policy.max_auto_promotions_per_day,
        }
    }
}

/// `[projects.<p>.dispatch]`: unspecified fields inherit the global dispatch policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_backlog: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promote_ideas: Option<PromoteIdeas>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_auto_promotions_per_day: Option<usize>,
}

impl DispatchOverride {
    pub fn is_empty(&self) -> bool {
        self.auto_backlog.is_none()
            && self.promote_ideas.is_none()
            && self.max_auto_promotions_per_day.is_none()
    }
}

impl Config {
    pub fn ready_project_allowed(&self, project: Option<&str>) -> bool {
        project.is_some_and(|project| {
            self.projects.contains_key(project)
                && (self.dispatch.ready_projects.is_empty()
                    || self.dispatch.ready_projects.iter().any(|p| p == project))
        })
    }

    /// Resolves project overrides; absent or unknown projects use global settings.
    /// Pause and Ready admission remain separate workspace-wide controls.
    pub fn dispatch_policy(&self, project: Option<&str>) -> DispatchPolicy {
        let mut policy = DispatchPolicy {
            auto_backlog: self.dispatch.auto_backlog,
            promote_ideas: self.dispatch.promote_ideas,
            max_auto_promotions_per_day: self.dispatch.max_auto_promotions_per_day,
        };
        if let Some(project) = project.and_then(|name| self.projects.get(name)) {
            policy.auto_backlog = project.dispatch.auto_backlog.unwrap_or(policy.auto_backlog);
            policy.promote_ideas = project
                .dispatch
                .promote_ideas
                .unwrap_or(policy.promote_ideas);
            policy.max_auto_promotions_per_day = project
                .dispatch
                .max_auto_promotions_per_day
                .unwrap_or(policy.max_auto_promotions_per_day);
        }
        policy
    }
}
