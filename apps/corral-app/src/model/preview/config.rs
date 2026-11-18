//! The sample workspace's `corral.toml`: `config.get` returns its text, comments included, and
//! `config.put` checks a candidate as corrald's engine does for the sample's keys, refusing with
//! one reason per line in the engine's words and leaving the text unchanged, or replaces it and
//! re-projects the settings and project rows from it.

use corral_protocol::rows::SettingsSnapshot;
use corral_types::Refusal;
use serde_json::{Map, Value, json};

use crate::model::Id;
use crate::model::daemon::{Feed, Operation};
use crate::model::settings::document;

/// The sample's file. Every project, profile and device it names is sample data.
pub(super) fn sample(projects: bool) -> String {
    let mut text = String::from(WORKSPACE);
    if projects {
        text.push_str(PROJECTS);
    }
    text.push_str(DAEMON);
    text
}

const WORKSPACE: &str = r#"# corral.toml of the sample workspace: every project, task and device here is sample data.

[limits]
running = 5                  # tasks in Running or Blocked at once; Review holds no slot

[defaults]
profile = "omp-sol"          # worker profile when nothing else names one
max_rounds = 2
auto_dispatch = false        # start Ready tasks when a slot is free
auto_land = false            # land after gate and review without the owner
branch_prefix = "corral/"

[runner]
mode = "herdr"
herdr_session = "corral"

[sync]
quiet_after_secs = 120
watch_secs = 5

[artifacts]
globs = ["docs/**/records/*", "**/*report*.html"]
max_bytes = 20000000

[harnesses.omp]
command = "omp {prompt_ref}"
model_flag = "--model"
effort_flag = "--thinking"

[harnesses.codex]
command = "codex exec {prompt_ref}"
model_flag = "-m"

[harnesses.pi]
command = "corral pi {prompt_ref}"
model_flag = "--model"
effort_flag = "--thinking"

[profiles.omp-sol]
harness = "omp"
model = "sol"
effort = "high"

[profiles.omp-sol-review]
harness = "omp"
model = "sol"
effort = "xhigh"

[profiles.codex-terra]
harness = "codex"
model = "terra"

[profiles.pi-luna]
harness = "pi"
model = "luna"
effort = "medium"
"#;

const PROJECTS: &str = r#"
[projects.corral]
path = "."
base = "main"
gate = ["just check"]        # each line runs with sh -c in the worktree
reviewer = "omp-sol-review"

[projects.site]
path = "site"
base = "release"
gate = ["bun test"]
reviewer = "omp-sol-review"
profile = "codex-terra"
running = 2
"#;

const DAEMON: &str = r#"
[daemon]                     # read when corrald starts; changing it needs a restart
listen = "127.0.0.1:7778"
web_origin = ["http://127.0.0.1:8080"]
"#;

/// The parsed configuration the settings topic carries: the file's keys over the engine's
/// defaults, as corrald serializes its parsed `Config`.
pub(super) fn parsed(text: &str) -> Map<String, Value> {
    let mut config = document::parse(text)
        .map(|doc| document::json(&doc))
        .unwrap_or_default();
    let defaults = json!({
        "limits": { "running": 4 },
        "defaults": {
            "branch_prefix": "corral/", "max_rounds": 2, "profile": "omp-sol",
            "auto_dispatch": false, "auto_land": false
        },
        "runner": { "mode": "herdr" },
        "sync": { "quiet_after_secs": 120, "watch_secs": 5 },
        "artifacts": { "globs": ["docs/**/records/*", "**/*report*.html"], "max_bytes": 20_000_000 }
    });
    if let Value::Object(defaults) = defaults {
        for (table, keys) in defaults {
            let Value::Object(keys) = keys else {
                continue;
            };
            let entry = config
                .entry(table)
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(entry) = entry {
                for (key, value) in keys {
                    entry.entry(key).or_insert(value);
                }
            }
        }
    }
    config
}

/// Answers `config.get` (no `put`) or `config.put` under `key`, as corrald records a reply.
pub fn config(feed: &mut Feed, key: &Id, put: Option<&str>) {
    let (value, refusal) = match put {
        None => (feed.sample_config.clone(), None),
        Some(text) => match check(text) {
            Ok(config) => {
                feed.sample_config = Some(text.to_owned());
                if let Some(settings) = &mut feed.settings {
                    settings.config = config;
                } else {
                    feed.settings = Some(SettingsSnapshot {
                        config,
                        ..SettingsSnapshot::default()
                    });
                }
                super::rollup::rebuild(feed);
                (Some(text.to_owned()), None)
            }
            Err(reason) => (None, Some(reason)),
        },
    };
    feed.operations.insert(
        key.clone(),
        Operation {
            method: if put.is_some() {
                "config.put"
            } else {
                "config.get"
            }
            .into(),
            loading: false,
            value: value.map(|toml| json!({ "toml": toml })),
            error: refusal
                .as_ref()
                .map(|reason| crate::model::daemon::ClientError {
                    code: "refused".into(),
                    message: reason.clone(),
                }),
            refusal: refusal.map(|reason| Refusal {
                reason,
                next: Vec::new(),
            }),
        },
    );
}

const BUILT_IN_HARNESSES: [&str; 4] = ["omo", "omp", "claude", "generic"];

/// The engine's check of the sample's keys: the parsed configuration, or every problem, one
/// per line.
fn check(text: &str) -> Result<Map<String, Value>, String> {
    let doc = document::parse(text)?;
    let config = parsed(text);
    // Indexing a `Value` reads a missing key as null; a `Map` index would panic.
    let root = Value::Object(config.clone());
    let table = |key: &str| config.get(key).and_then(Value::as_object);
    let names = |key: &str| table(key).map(|t| t.keys().cloned().collect::<Vec<_>>());
    let profiles = names("profiles").unwrap_or_default();
    let projects = names("projects").unwrap_or_default();
    let harnesses = names("harnesses").unwrap_or_default();
    let mut problems = Vec::new();
    let profile = |name: &Value, field: String, problems: &mut Vec<String>| {
        let name = name.as_str().unwrap_or_default();
        if !profiles.iter().any(|known| known == name) {
            problems.push(format!("{field}: unknown profile {name:?}"));
        }
    };
    if root["limits"]["running"].as_u64() == Some(0) {
        problems.push("limits.running must be greater than zero".into());
    }
    if !matches!(root["runner"]["mode"].as_str(), Some("process" | "herdr")) {
        problems.push("runner.mode must be process or herdr".into());
    }
    if root["defaults"]["base"]
        .as_str()
        .is_some_and(|base| base.trim().is_empty())
    {
        problems.push("defaults.base must not be empty".into());
    }
    if let Some(name) = root["defaults"].get("profile") {
        profile(name, "defaults.profile".into(), &mut problems);
    }
    for (name, harness) in table("harnesses").into_iter().flatten() {
        if harness["command"]
            .as_str()
            .is_none_or(|command| command.trim().is_empty())
        {
            problems.push(format!(
                "harnesses.{name}: name and command must not be empty"
            ));
        }
    }
    for (name, entry) in table("profiles").into_iter().flatten() {
        let harness = entry["harness"].as_str().unwrap_or_default();
        if !harnesses.iter().any(|known| known == harness) && !BUILT_IN_HARNESSES.contains(&harness)
        {
            problems.push(format!("profiles.{name}: unknown harness {harness:?}"));
        }
    }
    for (name, project) in table("projects").into_iter().flatten() {
        if project["path"].as_str().is_none_or(str::is_empty) {
            problems.push(format!("projects.{name}.path must not be empty"));
        }
        if project["base"]
            .as_str()
            .is_some_and(|base| base.trim().is_empty())
        {
            problems.push(format!("projects.{name}.base must not be empty"));
        }
        if project["running"].as_u64() == Some(0) {
            problems.push(format!("projects.{name}.running must be greater than zero"));
        }
        for key in ["profile", "reviewer"] {
            if let Some(named) = project.get(key) {
                profile(named, format!("projects.{name}.{key}"), &mut problems);
            }
        }
        let gate = document::lines(document::get(
            &doc,
            &document::Path::entry("projects", name, "gate"),
        ));
        if gate.iter().any(|line| line.trim().is_empty()) {
            problems.push(format!(
                "projects.{name}.gate contains an empty line or NUL"
            ));
        }
    }
    for (index, rule) in config
        .get("routing")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        profile(
            rule.get("profile").unwrap_or(&Value::Null),
            format!("routing[{index}].profile"),
            &mut problems,
        );
        if let Some(project) = rule["project"].as_str()
            && !projects.iter().any(|known| known == project)
        {
            problems.push(format!(
                "routing[{index}].project: unknown project {project:?}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(config)
    } else {
        Err(problems.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_file_passes_its_own_check_and_an_unknown_profile_is_refused_by_name() {
        assert!(check(&sample(true)).is_ok(), "{:?}", check(&sample(true)));
        let text = sample(true).replace("profile = \"codex-terra\"", "profile = \"nope\"");
        let refused = check(&text).unwrap_err();
        assert_eq!(refused, "projects.site.profile: unknown profile \"nope\"");
    }
}
