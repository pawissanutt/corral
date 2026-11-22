use std::{collections::BTreeMap, env, io::Write, path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use corral_engine::{
    Config,
    extension::{CommandContext, run},
};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::output::Output;

const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const CHECK_MAX_STDOUT: usize = 1024 * 1024;

#[derive(Serialize)]
struct LineCheck {
    line: String,
    #[serde(flatten)]
    result: CheckResult,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckResult {
    Stdout(String),
    Error(String),
}

pub(crate) fn check(
    root: &Path,
    config: &Config,
    name: &str,
    project: Option<String>,
    out: &mut Output<'_>,
) -> Result<u8> {
    let extension = config
        .extensions
        .get(name)
        .with_context(|| format!("unknown extension {name:?}"))?;
    let project = project
        .or_else(|| env::var("CORRAL_PROJECT").ok().filter(|p| !p.is_empty()))
        .or_else(|| {
            config
                .projects
                .keys()
                .find(|p| extension.includes_project(p))
                .cloned()
        });
    let empty_env = BTreeMap::new();
    let (cwd, env) = if let Some(project) = project.as_deref() {
        let settings = config
            .projects
            .get(project)
            .with_context(|| format!("unknown project {project:?}"))?;
        ensure!(
            extension.includes_project(project),
            "extension {name:?} does not include project {project:?}"
        );
        (root.join(&settings.path), &settings.env)
    } else {
        ensure!(
            extension.projects.is_none(),
            "extension {name:?} does not include any project"
        );
        (root.to_path_buf(), &empty_env)
    };
    let context = CommandContext {
        root,
        cwd: &cwd,
        project: project.as_deref(),
        extension: name,
        env,
    };
    let mut checks = Vec::new();
    let mut check = |label: String, line, mut request: Map<String, Value>| {
        request.insert("sample".into(), json!(true));
        let result = match run(line, request, &context, CHECK_TIMEOUT, CHECK_MAX_STDOUT) {
            Ok(bytes) => CheckResult::Stdout(match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
            }),
            Err(error) => CheckResult::Error(format!("{error:#}")),
        };
        checks.push(LineCheck {
            line: label,
            result,
        });
    };
    if let Some(line) = &extension.facts {
        check(
            "facts".into(),
            line,
            Map::from_iter([
                ("query".into(), json!("snapshot")),
                ("repo".into(), json!(cwd.to_string_lossy())),
                ("rev".into(), json!("HEAD")),
                ("want".into(), json!(["files", "definitions", "references"])),
            ]),
        );
    }
    if let Some(line) = &extension.run {
        let kind = extension.on.as_ref().and_then(|kinds| kinds.first());
        check(
            "run".into(),
            line,
            Map::from_iter([
                ("kind".into(), json!(kind.map_or("created", String::as_str))),
                ("project".into(), json!(project)),
                ("seq".into(), json!(0)),
                ("ts".into(), json!("1970-01-01T00:00:00Z")),
                ("task".into(), Value::Null),
                ("run".into(), Value::Null),
                ("actor".into(), json!("owner")),
                ("detail".into(), json!({})),
            ]),
        );
    }
    for (tool_name, tool) in &extension.tools {
        check(
            format!("tools.{tool_name}"),
            &tool.run,
            Map::from_iter([("args".into(), json!([]))]),
        );
    }
    out.value(checks.as_slice(), |out, checks| {
        if checks.is_empty() {
            return writeln!(out, "Extension {name:?} has no command lines");
        }
        for check in checks {
            match &check.result {
                CheckResult::Stdout(text) => {
                    writeln!(out, "{}.{}:", name, check.line)?;
                    out.write_all(text.as_bytes())?;
                    if !text.is_empty() && !text.ends_with('\n') {
                        writeln!(out)?;
                    }
                }
                CheckResult::Error(error) => writeln!(out, "{}.{}: {error}", name, check.line)?,
            }
        }
        Ok(())
    })?;
    Ok(u8::from(checks.iter().any(|check| {
        matches!(check.result, CheckResult::Error(_))
    })))
}
