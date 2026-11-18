//! What an operation's form offers beyond the task itself, read from the served rows and
//! configuration: the worker profiles with the one the engine would pick, the projects, the
//! base branch a task starts from and the project's local branches to land onto. The engine's
//! `resolved_profile` names the default where corrald serves it; an older one is followed in
//! the engine's order, and where the app cannot tell (a routing rule that matches titles by
//! pattern), it says so and leaves the choice to the engine.
use serde_json::Value;

use crate::model::daemon::Feed;

/// A form's served choices; empty when no configuration has arrived.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormContext {
    /// Configured worker profiles, by name.
    pub profiles: Vec<String>,
    /// The profile a dispatch without one runs: the row's `resolved_profile`, else as the app
    /// resolves it; `None` when routing decides by title pattern or nothing names one.
    pub profile: Option<String>,
    /// Configured projects, by id.
    pub projects: Vec<String>,
    /// The branch the task starts from and lands onto; `None` for the repository's default.
    pub base: Option<String>,
    /// The branch the task's latest worker run started from: where approving lands unless
    /// told otherwise.
    pub landing: Option<String>,
    /// The task project's local branches, newest tip first, offered as landing targets.
    pub branches: Vec<String>,
}

fn config(feed: &Feed) -> Option<&serde_json::Map<String, Value>> {
    feed.settings.as_ref().map(|settings| &settings.config)
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

/// One `[[routing]]` rule's verdict for the task: it picks a profile, it does not apply, or
/// only its title pattern can tell.
enum Route {
    Picks(String),
    Skips,
    Unknown,
}

fn route(rule: &Value, task: &corral_types::Task) -> Route {
    let listed = |key: &str, value: Option<&str>| {
        rule.get(key)
            .and_then(Value::as_array)
            .is_none_or(|values| {
                values.is_empty()
                    || value
                        .is_some_and(|value| values.iter().any(|item| item.as_str() == Some(value)))
            })
    };
    let project = text(rule.get("project"));
    if !listed("kind", task.kind.as_deref())
        || !listed("size", task.size.as_deref())
        || project.is_some_and(|project| task.project.as_deref() != Some(project.as_str()))
    {
        return Route::Skips;
    }
    if rule
        .get("title_regex")
        .is_some_and(|pattern| !pattern.is_null())
    {
        return Route::Unknown;
    }
    text(rule.get("profile")).map_or(Route::Skips, Route::Picks)
}

/// The task's project, as the projects topic resolves it after the workspace defaults.
fn effective<'a>(
    feed: &'a Feed,
    project: Option<&str>,
) -> Option<&'a corral_protocol::rows::EffectiveProjectSettings> {
    feed.projects.get(project?)?.effective.as_ref()
}

fn default_of(feed: &Feed, key: &str) -> Option<String> {
    text(config(feed)?.get("defaults")?.get(key))
}

/// The worker profile the engine resolves for `task` without a dispatch flag: the task's own,
/// the first matching routing rule, the project's, then the workspace default.
fn resolved_profile(feed: &Feed, task: &corral_types::Task) -> Option<String> {
    if let Some(profile) = &task.profile {
        return Some(profile.clone());
    }
    let rules = config(feed)
        .and_then(|config| config.get("routing"))
        .and_then(Value::as_array);
    for rule in rules.into_iter().flatten() {
        match route(rule, task) {
            Route::Picks(profile) => return Some(profile),
            Route::Unknown => return None,
            Route::Skips => {}
        }
    }
    effective(feed, task.project.as_deref())
        .and_then(|settings| settings.profile.clone())
        .or_else(|| default_of(feed, "profile"))
}

impl FormContext {
    /// The choices for `task`, or for a new task when there is none.
    pub fn new(feed: &Feed, task: Option<&corral_types::Task>) -> Self {
        let names = |key: &str| -> Vec<String> {
            config(feed)
                .and_then(|config| config.get(key))
                .and_then(Value::as_object)
                .map(|table| table.keys().cloned().collect())
                .unwrap_or_default()
        };
        let project = task.and_then(|task| task.project.as_deref());
        let row = task.and_then(|task| feed.board.task(&task.id));
        Self {
            profiles: names("profiles"),
            profile: row
                .and_then(|row| row.row.resolved_profile.clone())
                .or_else(|| task.and_then(|task| resolved_profile(feed, task))),
            projects: feed.projects.keys().map(ToString::to_string).collect(),
            base: task
                .and_then(|task| task.base.clone())
                .or_else(|| effective(feed, project).and_then(|settings| settings.base.clone()))
                .or_else(|| default_of(feed, "base")),
            landing: row
                .and_then(|row| row.row.run.as_ref())
                .map(|run| run.base.clone()),
            // Never the task's own branch: the engine refuses landing it onto itself.
            branches: crate::model::projects::branches(feed, project)
                .into_iter()
                .filter(|branch| {
                    row.and_then(|row| row.row.run.as_ref())
                        .is_none_or(|run| run.branch != *branch)
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::rows::{EffectiveProjectSettings, ProjectRow, SettingsSnapshot};
    use serde_json::json;

    fn feed(routing: Value) -> Feed {
        let config = json!({
            "defaults": { "profile": "omp-sol" },
            "profiles": { "omp-sol": {}, "codex-terra": {}, "omp-opus": {} },
            "routing": routing,
        });
        let mut feed = Feed {
            settings: Some(SettingsSnapshot {
                config: config.as_object().cloned().unwrap_or_default(),
                ..SettingsSnapshot::default()
            }),
            ..Feed::default()
        };
        feed.projects.insert(
            "site".into(),
            ProjectRow {
                id: "site".into(),
                effective: Some(EffectiveProjectSettings {
                    base: Some("release".into()),
                    profile: Some("codex-terra".into()),
                    ..EffectiveProjectSettings::default()
                }),
                ..ProjectRow::default()
            },
        );
        feed
    }

    /// The dispatch form marks the profile the engine would pick and starts from the project's
    /// effective base; a title-pattern rule leaves the profile to the engine unless the row
    /// names the engine's own resolution.
    #[test]
    fn the_dispatch_default_follows_the_engines_resolution_order() {
        let mut task = crate::model::daemon::bare_task(
            "T-1",
            "Frontend polish",
            corral_types::Status::Ready,
            jiff::Timestamp::UNIX_EPOCH,
        );
        task.project = Some("site".into());
        task.kind = Some("ui".into());
        let plain = FormContext::new(&feed(json!([])), Some(&task));
        assert_eq!(plain.profile.as_deref(), Some("codex-terra"));
        assert_eq!(plain.base.as_deref(), Some("release"));
        assert_eq!(plain.profiles.len(), 3);
        let routed = FormContext::new(
            &feed(json!([{ "kind": ["ui"], "profile": "omp-opus" }])),
            Some(&task),
        );
        assert_eq!(routed.profile.as_deref(), Some("omp-opus"));
        let mut pattern = feed(json!([{ "title_regex": "(?i)front", "profile": "omp-opus" }]));
        assert_eq!(FormContext::new(&pattern, Some(&task)).profile, None);
        let mut row = crate::model::daemon::task_row(task.clone());
        row.resolved_profile = Some("omp-opus".into());
        pattern.board_received(1, vec![crate::model::daemon::Task::new(row)], &[], true);
        let served = FormContext::new(&pattern, Some(&task));
        assert_eq!(served.profile.as_deref(), Some("omp-opus"));
    }
}
