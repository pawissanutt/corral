//! The sample workspace's `corral.toml`: `config.get` returns its text, comments included, and
//! its revision, and `config.put` checks a candidate as corrald's engine does for the sample's
//! keys, refusing with one problem per key (or a syntax error at its line and column) and
//! leaving the text unchanged, or replaces it and re-projects the settings and project rows
//! from it.

use corral_protocol::rows::SettingsSnapshot;
use corral_types::{ConfigProblem, Refusal};
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

/// The engine's built-in configuration, as corrald serializes `Config::default()` for the
/// sample's tables.
fn built_in() -> Map<String, Value> {
    let defaults = json!({
        "limits": { "running": 4 },
        "defaults": {
            "base": null, "branch_prefix": "corral/", "max_rounds": 2, "profile": "omp-sol",
            "auto_dispatch": false, "auto_land": false
        },
        "harnesses": {
            "pi": { "command": "corral-pi run {model_args} --prompt-file {prompt_file} --corral {corral}", "model_flag": "--model", "effort_flag": "--effort" },
            "claude": { "command": "claude {model_args} {prompt_ref}", "model_flag": "--model", "effort_flag": "--effort" },
            "omo": { "command": "omo -n {name} {model_args} {prompt_ref}", "model_flag": "--model", "effort_flag": "--effort" },
            "omp": { "command": "omp {model_args} {prompt_ref}", "model_flag": "--model", "effort_flag": "--effort" },
            "generic": { "command": "cat {prompt_file}", "model_flag": null, "effort_flag": null }
        },
        "profiles": {
            "omo": { "harness": "omo" },
            "omp": { "harness": "omp" },
            "claude": { "harness": "claude" },
            "pi": { "harness": "pi" },
            "generic": { "harness": "generic" },
            "omp-sol": { "harness": "omp", "model": "gpt-6.2-sol", "effort": "medium" }
        },
        "runner": { "mode": "herdr" },
        "sync": { "quiet_after_secs": 120, "watch_secs": 5 },
        "artifacts": { "globs": ["docs/**/records/*", "**/*report*.html"], "max_bytes": 20_000_000 }
    });
    match defaults {
        Value::Object(defaults) => defaults,
        _ => Map::new(),
    }
}

/// An unconfigured project resolved against the built-in workspace defaults.
fn project_defaults() -> Map<String, Value> {
    match json!({
        "path": "", "base": null, "gate": [], "reviewer": null, "profile": "omp-sol",
        "running": null, "env": {}, "auto_dispatch": false, "auto_land": false,
        "max_rounds": 2, "branch_prefix": "corral/"
    }) {
        Value::Object(project) => project,
        _ => Map::new(),
    }
}

/// The file's content fingerprint, as corrald states it beside the text.
fn revision(text: &str) -> String {
    super::sample::sha(text, 16)
}

/// The settings snapshot for `text`: the parsed configuration, its revision and the engine's
/// defaults, around the listener and devices `base` carries.
pub(super) fn snapshot(text: &str, base: SettingsSnapshot) -> SettingsSnapshot {
    SettingsSnapshot {
        config: parsed(text),
        revision: Some(revision(text)),
        built_in_defaults: Some(built_in()),
        project_defaults: Some(project_defaults()),
        problem: None,
        ..base
    }
}

/// The parsed configuration the settings topic carries: the file's keys over the engine's
/// defaults, as corrald serializes its parsed `Config`.
fn parsed(text: &str) -> Map<String, Value> {
    let mut config = document::parse(text)
        .map(|doc| document::json(&doc))
        .unwrap_or_default();
    for (table, keys) in built_in() {
        let Value::Object(keys) = keys else {
            continue;
        };
        let entry = config
            .entry(table)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(entry) = entry {
            for (key, value) in keys.into_iter().filter(|(_, value)| !value.is_null()) {
                entry.entry(key).or_insert(value);
            }
        }
    }
    config
}

/// Answers `config.get` (no `put`) or `config.put` under `key`, as corrald records a reply.
pub fn config(feed: &mut Feed, key: &Id, put: Option<&str>) {
    let (value, problems) = match put {
        None => (feed.sample_config.clone(), None),
        Some(text) => match check(text) {
            Ok(()) => {
                feed.sample_config = Some(text.to_owned());
                let base = feed.settings.take().unwrap_or_default();
                feed.settings = Some(snapshot(text, base));
                super::rollup::rebuild(feed);
                (Some(text.to_owned()), None)
            }
            Err(problems) => (None, Some(problems)),
        },
    };
    let refusal = problems.map(|problems| {
        let mut reason = String::from("configuration problems:");
        for problem in &problems {
            reason.push_str(&format!("\n{problem}"));
        }
        Refusal {
            reason,
            next: Vec::new(),
            problems: Some(problems),
        }
    });
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
            value: value.map(|toml| json!({ "revision": revision(&toml), "toml": toml })),
            error: refusal
                .as_ref()
                .map(|refusal| crate::model::daemon::ClientError {
                    code: "refused".into(),
                    message: refusal.reason.clone(),
                }),
            refusal,
        },
    );
}

const BUILT_IN_HARNESSES: [&str; 5] = ["omo", "omp", "claude", "pi", "generic"];

/// A problem the engine names by key.
fn keyed(key: impl Into<String>, message: impl Into<String>) -> ConfigProblem {
    ConfigProblem {
        key: Some(key.into()),
        message: message.into(),
        line: None,
        column: None,
    }
}

/// A syntax error at its 1-based line and column, as the engine reports one.
fn syntax(text: &str, error: &toml_edit::TomlError) -> ConfigProblem {
    let start = error.span().map_or(0, |span| span.start);
    let before = &text[..text.floor_char_boundary(start)];
    ConfigProblem {
        key: None,
        message: error.message().to_owned(),
        line: Some(before.matches('\n').count() + 1),
        column: Some(
            before
                .rsplit('\n')
                .next()
                .map_or(1, |s| s.chars().count() + 1),
        ),
    }
}

/// The engine's check of the sample's keys: every problem, each naming its key, or the one
/// syntax error.
fn check(text: &str) -> Result<(), Vec<ConfigProblem>> {
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| vec![syntax(text, &error)])?;
    let config = parsed(text);
    // Indexing a `Value` reads a missing key as null; a `Map` index would panic.
    let root = Value::Object(config.clone());
    let table = |key: &str| config.get(key).and_then(Value::as_object);
    let names = |key: &str| table(key).map(|t| t.keys().cloned().collect::<Vec<_>>());
    let profiles = names("profiles").unwrap_or_default();
    let projects = names("projects").unwrap_or_default();
    let harnesses = names("harnesses").unwrap_or_default();
    let mut problems = Vec::new();
    let profile = |name: &Value, field: String, problems: &mut Vec<ConfigProblem>| {
        let name = name.as_str().unwrap_or_default();
        if !profiles.iter().any(|known| known == name) {
            problems.push(keyed(field, format!("unknown profile {name:?}")));
        }
    };
    if root["limits"]["running"].as_u64() == Some(0) {
        problems.push(keyed("limits.running", "must be greater than zero"));
    }
    if !matches!(root["runner"]["mode"].as_str(), Some("process" | "herdr")) {
        problems.push(keyed("runner.mode", "must be process or herdr"));
    }
    if root["defaults"]["base"]
        .as_str()
        .is_some_and(|base| base.trim().is_empty())
    {
        problems.push(keyed("defaults.base", "must not be empty"));
    }
    if let Some(name) = root["defaults"].get("profile") {
        profile(name, "defaults.profile".into(), &mut problems);
    }
    for (name, harness) in table("harnesses").into_iter().flatten() {
        if harness["command"]
            .as_str()
            .is_none_or(|command| command.trim().is_empty())
        {
            problems.push(keyed(
                format!("harnesses.{name}.command"),
                "must not be empty",
            ));
        }
    }
    for (name, entry) in table("profiles").into_iter().flatten() {
        let harness = entry["harness"].as_str().unwrap_or_default();
        if !harnesses.iter().any(|known| known == harness) && !BUILT_IN_HARNESSES.contains(&harness)
        {
            problems.push(keyed(
                format!("profiles.{name}.harness"),
                format!("unknown harness {harness:?}"),
            ));
        }
    }
    for (name, project) in table("projects").into_iter().flatten() {
        if project["path"].as_str().is_none_or(str::is_empty) {
            problems.push(keyed(format!("projects.{name}.path"), "must not be empty"));
        }
        if project["base"]
            .as_str()
            .is_some_and(|base| base.trim().is_empty())
        {
            problems.push(keyed(format!("projects.{name}.base"), "must not be empty"));
        }
        if project["running"].as_u64() == Some(0) {
            problems.push(keyed(
                format!("projects.{name}.running"),
                "must be greater than zero",
            ));
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
            problems.push(keyed(
                format!("projects.{name}.gate"),
                "contains an empty line or NUL",
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
            problems.push(keyed(
                format!("routing[{index}].project"),
                format!("unknown project {project:?}"),
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_routing_target_is_reported_and_accepted_without_a_profile_table() {
        let text = "[[routing]]\nprofile = 'omp'\n";
        let settings = snapshot(text, SettingsSnapshot::default());
        assert!(
            settings.built_in_defaults.unwrap()["profiles"]
                .get("omp")
                .is_some()
        );
        assert_eq!(check(text), Ok(()));
    }

    #[test]
    fn the_sample_file_passes_its_own_check_and_an_unknown_profile_is_refused_by_name() {
        assert!(check(&sample(true)).is_ok(), "{:?}", check(&sample(true)));
        let text = sample(true).replace("profile = \"codex-terra\"", "profile = \"nope\"");
        let refused = check(&text).unwrap_err();
        assert_eq!(
            refused,
            [keyed("projects.site.profile", "unknown profile \"nope\"")]
        );
    }
}
