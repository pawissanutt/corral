//! Setup checks and safe repairs. No operation here starts an agent or changes panes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::{Config, Status, Store};
use corral_herdr::{Herdr, friendly_error};
use corral_ops::{Ctx, SKILL_HARNESSES};
use serde::Serialize;

use crate::{plugin, skill};

mod environment;
mod gates;
mod resources;
mod terminal_runs;

#[derive(Debug, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub ok: bool,
    pub hard: bool,
    pub message: String,
    pub fix: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
    pub changes: Vec<String>,
}

impl Report {
    pub fn healthy(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
    }

    pub fn startup_ready(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.ok || check.id == "herdr_server" || !check.hard)
    }

    pub fn print(&self) {
        for change in &self.changes {
            println!("{change}");
        }
        for check in &self.checks {
            println!(
                "{} {}: {}",
                if check.ok {
                    "ok"
                } else if check.hard {
                    "FAIL"
                } else {
                    "WARN"
                },
                check.id,
                check.message
            );
            if let Some(fix) = &check.fix {
                println!("  Fix: {fix}");
            }
        }
    }
}

fn check(id: &'static str, result: Result<bool>, hard: bool, fix: &str) -> Check {
    let (ok, message) = match result {
        Ok(true) => (true, "ready".into()),
        Ok(false) => (false, "not set up".into()),
        Err(error) => (false, friendly_error(&error)),
    };
    Check {
        id,
        ok,
        hard,
        message,
        fix: (!ok).then(|| fix.to_string()),
    }
}

pub fn session(config: &Config, requested: Option<String>) -> Option<String> {
    requested
        .or_else(|| {
            std::env::var("CORRAL_HERDR_SESSION")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| config.herdr.session.clone())
        .or_else(|| (std::env::var("HERDR_ENV").as_deref() != Ok("1")).then(|| "corral".into()))
}

fn skill_targets(installed: &[corral_ops::HarnessInfo]) -> Vec<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut names: Vec<_> = SKILL_HARNESSES
        .iter()
        .filter(|(name, marker, _)| {
            home.as_ref().is_some_and(|h| h.join(marker).is_dir())
                || installed.iter().any(|h| h.key == *name && h.installed())
        })
        .map(|(name, ..)| name.to_string())
        .collect();
    if names.is_empty() {
        names.push("agents".into());
    }
    names
}

fn skills_present(names: &[String]) -> Result<bool> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    Ok(SKILL_HARNESSES
        .iter()
        .filter(|(name, ..)| names.iter().any(|n| n == name))
        .all(|(_, _, dir)| home.join(dir).join("corral/SKILL.md").is_file()))
}

fn terminal_worktrees(store: &Store) -> Result<Check> {
    let mut paths = Vec::new();
    for task in store.list()? {
        if matches!(task.status, Status::Done | Status::Dropped) {
            for run in &task.runs {
                if run.worktree.is_dir() {
                    paths.push(format!(
                        "{} {}: {}",
                        task.id,
                        run.id,
                        run.worktree.display()
                    ));
                }
            }
        }
    }
    Ok(Check {
        id: "terminal_worktrees",
        ok: paths.is_empty(),
        hard: false,
        message: if paths.is_empty() {
            "No worktree directories remain for Done/Dropped tasks.".into()
        } else {
            format!("Worktree directories remain: {}", paths.join("; "))
        },
        fix: (!paths.is_empty()).then(|| "Inspect retained work before removing it; cleanup warnings are recorded in task events.".into()),
    })
}

pub fn inspect(start: &Path, requested: Option<String>, fix: bool) -> Result<Report> {
    // Missing stores are reported, while invalid existing configs keep their useful parse errors.
    let store = match Store::find_root(start) {
        Ok(root) => Some(Store::open(&root)?),
        Err(_) => None,
    };
    let default_config = Config::default_for(start);
    let config = store.as_ref().map_or(&default_config, |s| &s.config);
    let herdr = Herdr::new(session(config, requested));
    let version = herdr.version();
    let installed = version.is_ok();
    let mut checks = vec![check(
        "herdr_version",
        version.map(|_| true),
        true,
        &format!(
            "Install or update the terminal engine: `{}`.",
            corral_herdr::INSTALL_HERDR
        ),
    )];
    if let Some(store) = &store {
        checks.push(terminal_worktrees(store)?);
        checks.push(terminal_runs::check(store)?);
        checks.extend(gates::checks(store));
        checks.extend(resources::checks(&Ctx::from_store(store.clone())));
    }
    let workspaces = herdr.workspace_list();
    if workspaces
        .as_ref()
        .is_ok_and(|workspaces| !workspaces.is_empty())
    {
        environment::report(&herdr);
    }
    checks.push(check(
        "herdr_server",
        workspaces.map(|_| true),
        true,
        "Run `corral up` to start or attach the board's terminal engine.",
    ));
    checks.push(check(
        "workspace",
        Ok(store.is_some()),
        true,
        "Run `corral init` in your workspace; repositories are detected automatically.",
    ));
    checks.push(check(
        "projects",
        Ok(store.as_ref().is_some_and(|s| !s.config.projects.is_empty())),
        true,
        "Run `corral init` from a git repository or a directory containing repositories; for an existing workspace, add one with `corral config set projects.app.path '\"/path/to/repo\"'`.",
    ));
    let harnesses = corral_ops::detect_harnesses(config);
    checks.push(check(
        "harness",
        Ok(harnesses
            .iter()
            .any(|h| h.key != "generic" && h.installed())),
        true,
        "Install an agent harness (omo, omp or claude), then run `corral harness list --refresh`.",
    ));
    let names = skill_targets(&harnesses);
    let mut changes = Vec::new();
    let mut plugin_error = None;
    if fix {
        if installed && let Some(store) = &store {
            let ctx = Ctx {
                store: store.clone(),
                herdr: herdr.clone(),
                ..Ctx::from_store(store.clone())
            };
            match plugin::setup(&ctx) {
                Ok(messages) => changes.extend(messages),
                Err(error) => plugin_error = Some(error),
            }
        } else {
            match plugin::merge_keybindings(&config.herdr.keys) {
                Ok(messages) => changes.extend(messages),
                Err(error) => plugin_error = Some(error),
            }
        }
        match skill::install_report(&names, None, false) {
            Ok(messages) => changes.extend(messages),
            Err(error) => changes.push(friendly_error(&error)),
        }
    }
    checks.push(check(
        "plugin",
        match plugin_error {
            Some(error) => Err(error),
            None => match &store {
                Some(store) => {
                    herdr.plugin_linked(&store.state_dir().join("herdr-plugin/herdr-plugin.toml"))
                }
                None => Ok(false),
            },
        },
        false,
        "Run `corral doctor --fix` (or `corral plugin install`) after `corral init`.",
    ));
    checks.push(check(
        "keybindings",
        plugin::keybindings_present(&config.herdr.keys),
        false,
        "Run `corral doctor --fix` to merge keybindings with a backup and diff.",
    ));
    checks.push(artifact_opener(&corral_ops::viewers::SystemHost));
    checks.push(check(
        "skill",
        skills_present(&names),
        false,
        "Run `corral doctor --fix` to install the corral agent skill.",
    ));
    Ok(Report { checks, changes })
}

/// How `corral artifact serve` (and opening an HTML artifact) reaches a browser from here. Informational: a
/// printed URL over SSH is a supported route, not a setup failure.
fn artifact_opener(host: &dyn corral_ops::viewers::Host) -> Check {
    use corral_ops::viewers::{UrlRoute, url_route};
    let message = match url_route("http://127.0.0.1:<port>/", 0, host) {
        UrlRoute::Launch(argv) => format!("served artifacts open with {}", argv[0]),
        UrlRoute::Print(why) => format!(
            "served artifact URLs are printed ({why}); set CORRAL_OPENER to choose a browser"
        ),
    };
    Check {
        id: "artifact_opener",
        ok: true,
        hard: false,
        message,
        fix: None,
    }
}

pub fn run(start: &Path, fix: bool, json: bool) -> Result<()> {
    let report = inspect(start, None, fix)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        report.print();
    }
    if !report.healthy() {
        bail!("setup needs attention; use the fixes listed above");
    }
    Ok(())
}
