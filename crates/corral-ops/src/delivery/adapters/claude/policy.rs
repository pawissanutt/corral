//! Fail-closed inbound policy. CLI accept cannot loosen a repository or managed
//! restriction. Unknown/dynamic policy sources are blocked, never bypassed or executed.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::{Session, process};
use crate::delivery::{BlockReason, Readiness};
use crate::watchdog::{ProcessControl, SystemProcesses};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Policy {
    Accept,
    Hold,
    Refuse,
}

fn value(settings: &Value) -> Result<Option<Policy>> {
    ensure!(settings.is_object(), "Claude settings must be an object");
    match settings.get("crossSessionInbound") {
        None => Ok(None),
        Some(Value::String(s)) if s == "accept" => Ok(Some(Policy::Accept)),
        Some(Value::String(s)) if s == "hold" => Ok(Some(Policy::Hold)),
        Some(Value::String(s)) if s == "refuse" => Ok(Some(Policy::Refuse)),
        Some(_) => anyhow::bail!("unrecognized crossSessionInbound"),
    }
}

fn settings(path: &Path) -> Result<Value> {
    match process::read_json(path) {
        Ok(value) => Ok(value),
        Err(e)
            if e.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            }) =>
        {
            Ok(serde_json::json!({}))
        }
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "linux")]
fn environment(pid: u32) -> Result<BTreeMap<String, String>> {
    let bytes = process::read(Path::new(&format!("/proc/{pid}/environ")), 1024 * 1024)?;
    Ok(bytes
        .split(|b| *b == 0)
        .filter_map(|s| {
            let text = std::str::from_utf8(s).ok()?;
            let (key, value) = text.split_once('=')?;
            // Do not retain credentials or unrelated environment data.
            (key == "HOME"
                || key == "CLAUDE_CONFIG_DIR"
                || ((key.starts_with("CLAUDE_") || key.starts_with("ANTHROPIC_"))
                    && (key.contains("SETTINGS") || key.contains("POLICY"))))
            .then(|| (key.to_string(), value.to_string()))
        })
        .collect())
}

#[cfg(not(target_os = "linux"))]
fn environment(_pid: u32) -> Result<BTreeMap<String, String>> {
    anyhow::bail!("Claude effective policy inspection is unverified on this platform")
}

fn option<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>> {
    let mut found = None;
    for (n, arg) in args.iter().enumerate() {
        let value = if arg == name {
            Some(
                args.get(n + 1)
                    .context("missing Claude option value")?
                    .as_str(),
            )
        } else {
            arg.strip_prefix(&format!("{name}="))
        };
        if let Some(value) = value {
            ensure!(found.is_none(), "repeated Claude {name}");
            found = Some(value);
        }
    }
    Ok(found)
}

fn managed(base: &Path) -> Result<Vec<Value>> {
    let mut values = vec![settings(base)?];
    let dropins = base
        .parent()
        .context("managed settings have no directory")?
        .join("managed-settings.d");
    let entries = match std::fs::read_dir(&dropins) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(values),
        Err(e) => return Err(e).context("reading managed Claude drop-ins"),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.context("reading managed Claude drop-in")?.path();
        if path.extension().is_some_and(|e| e == "json")
            && path
                .file_name()
                .is_some_and(|n| !n.to_string_lossy().starts_with('.'))
        {
            paths.push(path);
        }
    }
    paths.sort();
    for path in paths {
        values.push(settings(&path)?);
    }
    Ok(values)
}

pub(super) fn readiness(session: &Session) -> Result<Readiness> {
    let env = environment(session.pid)?;
    // Overrides/host policies whose provenance or runtime output cannot be reconstructed
    // from these files must not silently look like an empty managed policy.
    for key in env.keys() {
        ensure!(
            !key.contains("POLICY")
                && (!key.contains("SETTINGS") || key == "CLAUDE_CODE_MANAGED_SETTINGS_PATH"),
            "unverified Claude policy environment: {key}"
        );
    }
    let directory = env
        .get("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| env.get("HOME").map(|h| Path::new(h).join(".claude")))
        .context("Claude process has no configuration directory")?;
    ensure!(
        directory.is_absolute(),
        "relative Claude configuration directory"
    );
    let process = SystemProcesses
        .inspect(session.pid)?
        .context("Claude process exited")?;
    ensure!(
        process.started == session.proc_start,
        "Claude process changed during policy read"
    );
    let args = &process.argv;
    let sources = option(args, "--setting-sources")?.unwrap_or("user,project,local");
    let sources: Vec<_> = sources.split(',').collect();
    ensure!(
        sources
            .iter()
            .all(|s| ["user", "project", "local", ""].contains(s)),
        "unknown Claude settings source"
    );
    let user = if sources.contains(&"user") {
        settings(&directory.join("settings.json"))?
    } else {
        serde_json::json!({})
    };
    let cli = match option(args, "--settings")? {
        Some(text) if text.trim_start().starts_with('{') => {
            serde_json::from_str(text).context("parsing Claude CLI settings")?
        }
        Some(file) => settings(&session.cwd.join(file))?,
        None => serde_json::json!({}),
    };
    // Missing accept is not evidence of readiness in bypassPermissions (live capture).
    let mut policy = value(&cli)?.or(value(&user)?).unwrap_or(Policy::Hold);
    let managed_path = env
        .get("CLAUDE_CODE_MANAGED_SETTINGS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/claude-code/managed-settings.json"));
    ensure!(
        managed_path.is_absolute(),
        "relative Claude managed settings path"
    );
    let mut admin = managed(&managed_path)?;
    // Remote/embedding policies may have runtime state not represented by the cache.
    // Refuse rather than guessing that an absent key means effective acceptance.
    let remote = settings(&directory.join("remote-settings.json"))?;
    ensure!(
        remote.as_object().is_some_and(|v| v.is_empty()),
        "remote Claude policy requires effective-policy attestation"
    );
    for settings in admin.drain(..) {
        ensure!(
            settings.get("policyHelper").is_none() && settings.get("policyHelpers").is_none(),
            "dynamic Claude policy requires attestation"
        );
        if let Some(managed) = value(&settings)? {
            policy = policy.max(managed);
        }
    }
    if sources.contains(&"project") {
        policy = policy.max(
            value(&settings(&session.cwd.join(".claude/settings.json"))?)?
                .unwrap_or(Policy::Accept),
        );
    }
    if sources.contains(&"local") {
        let mut locals = vec![session.cwd.join(".claude/settings.local.json")];
        // Claude 2.1.304 also reads local settings at the main checkout root for worktrees.
        if session.cwd.join(".git").exists() {
            let root = corral_git::git_common_dir(&session.cwd)?;
            locals.push(
                root.parent()
                    .context("git common directory has no parent")?
                    .join(".claude/settings.local.json"),
            );
            locals.push(corral_git::repo_root(&session.cwd)?.join(".claude/settings.local.json"));
        }
        for local in locals {
            policy = policy.max(value(&settings(&local)?)?.unwrap_or(Policy::Accept));
        }
    }
    Ok(match policy {
        Policy::Accept => Readiness::Native,
        Policy::Hold => Readiness::Blocked {
            reason: BlockReason::InboundHold,
        },
        Policy::Refuse => Readiness::Blocked {
            reason: BlockReason::PolicyRefused,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_policy_values_and_duplicate_cli_settings_fail_closed() {
        assert!(value(&serde_json::json!({"crossSessionInbound":"future"})).is_err());
        assert!(value(&serde_json::json!({"crossSessionInbound":null})).is_err());
        assert!(
            option(
                &["--settings".into(), "{}".into(), "--settings={}".into()],
                "--settings"
            )
            .is_err()
        );
    }
}
