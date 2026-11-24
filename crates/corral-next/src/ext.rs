use std::{collections::BTreeMap, env, io::Write, path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use corral_engine::{
    Config, Store,
    extension::{CommandContext, run},
    react,
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

/// One pass without a daemon, for CLI or cron setups. Retries a failed event at once.
pub(crate) fn deliver(root: &Path, config: &Config, out: &mut Output<'_>) -> Result<u8> {
    let state_dir = root.join(".corral-next");
    let store = Store::open(&state_dir.join("corral.db"))?;
    let reports = react::deliver(&react::Pass {
        root,
        state_dir: &state_dir,
        config,
        store: &store,
        retry_after: Duration::ZERO,
        stop: &|| false,
    });
    out.value(reports.as_slice(), |out, reports| {
        for report in reports {
            let name = &report.extension;
            if let Some(error) = &report.error {
                writeln!(out, "{name}: {error}")?;
            } else if report.busy {
                writeln!(out, "{name}: another process is delivering")?;
            } else {
                write!(
                    out,
                    "{name}: delivered {}, skipped {}",
                    report.delivered, report.skipped
                )?;
                if report.failed {
                    let cursor = react::status(&state_dir, name).ok().flatten();
                    let error = cursor.and_then(|cursor| cursor.last_error);
                    write!(out, "; failed: {}", error.unwrap_or_default())?;
                }
                writeln!(out)?;
            }
        }
        Ok(())
    })?;
    Ok(u8::from(
        reports
            .iter()
            .any(|report| report.failed || report.error.is_some()),
    ))
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
    let project = project.or_else(|| {
        // With no eligible project, run a workspace-level sample even if the
        // caller's shell inherited a project context.
        let first_eligible = config
            .projects
            .keys()
            .find(|p| extension.includes_project(p))?;
        Some(
            env::var("CORRAL_PROJECT")
                .ok()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| first_eligible.clone()),
        )
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
        (root.to_path_buf(), &empty_env)
    };
    let context = CommandContext {
        root,
        cwd: &cwd,
        project: project.as_deref(),
        extension: name,
        env,
        hold: None,
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
                (
                    "want".into(),
                    json!(["files", "definitions", "references", "entries", "types"]),
                ),
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
