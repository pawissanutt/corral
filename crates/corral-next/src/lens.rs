use std::{borrow::Cow, io::Write, path::Path};

use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use corral_engine::{
    Config,
    facts::{FactsReader, FactsReport, LensResult, LensStatus},
};
use serde::Serialize;

use crate::{launch_workspace_matches, output::Output, tool_project};

#[derive(Subcommand)]
pub(crate) enum LensCommand {
    /// List committed lenses and their status at the project's base tip.
    List {
        #[arg(long, short)]
        project: Option<String>,
    },
    /// File an ordinary task to draft a behaviour lens.
    Draft {
        project: String,
        #[arg(long, value_parser = ["state", "sequence"])]
        kind: String,
        #[arg(long)]
        about: String,
        #[arg(long)]
        ready: bool,
    },
    /// File an ordinary task to repair a committed lens.
    Repair {
        project: String,
        id: String,
        #[arg(long)]
        ready: bool,
    },
    /// Check lenses at the project base tip or a specified Git commit.
    Check {
        #[arg(long, short)]
        project: Option<String>,
        #[arg(long)]
        at: Option<String>,
        /// A path relative to the repository or its configured lens directory.
        file: Option<String>,
    },
}

#[derive(Serialize)]
struct Checked<'a> {
    project: &'a str,
    commit: &'a str,
    provider: Option<FactsReport>,
    lenses: Vec<LensResult>,
}

pub(crate) fn run(
    root: &Path,
    cwd: &Path,
    command: LensCommand,
    out: &mut Output<'_>,
) -> Result<u8> {
    let (check, project, at, file) = match command {
        LensCommand::List { project } => (false, project, None, None),
        LensCommand::Check { project, at, file } => (true, project, at, file),
        LensCommand::Draft {
            project,
            kind,
            about,
            ready,
        } => {
            let engine = corral_engine::Engine::open(root.to_path_buf())?;
            crate::apply(
                &engine,
                out,
                corral_engine::Op::LensDraft {
                    project,
                    kind,
                    about,
                    ready,
                },
                false,
            )?;
            return Ok(0);
        }
        LensCommand::Repair { project, id, ready } => {
            let engine = corral_engine::Engine::open(root.to_path_buf())?;
            crate::apply(
                &engine,
                out,
                corral_engine::Op::LensRepair { project, id, ready },
                false,
            )?;
            return Ok(0);
        }
    };
    let config = Config::load(&root.join(".corral-next/corral.toml"))?;
    let project = tool_project(
        &config,
        root,
        cwd,
        project.as_deref(),
        None,
        launch_workspace_matches(root),
    )?
    .context("no project configured")?;
    let reader = FactsReader::new(root, &config, project)?;
    let rev = match at {
        Some(at) => reader.resolve_revision(&at)?,
        None => reader.resolve_revision(&reader.project_tip(reader.base_branch()?.as_deref())?)?,
    };
    let (mut lenses, provider) = reader.check_lenses(&rev)?;
    if let Some(file) = file {
        let prefix = format!("{}/", reader.lens_dir());
        // Prefer an exact repository-relative path if one also looks like a
        // path relative to the lens directory.
        let matches = if lenses.iter().any(|lens| lens.file == file) {
            file.clone()
        } else {
            format!("{prefix}{file}")
        };
        lenses.retain(|lens| lens.file == matches);
        ensure!(
            !lenses.is_empty(),
            "no lens file {file:?} at {}",
            rev.commit()
        );
    }
    let view = Checked {
        project,
        commit: rev.commit(),
        provider,
        lenses,
    };
    out.value(&view, |out, view| {
        if view.lenses.is_empty() {
            return writeln!(
                out,
                "No lens files for {} at {}",
                terminal(view.project),
                view.commit
            );
        }
        if let Some(error) = view
            .provider
            .as_ref()
            .and_then(|report| report.error.as_ref())
        {
            writeln!(out, "Facts provider unavailable: {}", terminal(error))?;
        }
        for result in &view.lenses {
            let status = result.status.as_str();
            let file = terminal(&result.file);
            if let Some(lens) = &result.lens {
                writeln!(
                    out,
                    "{file}: {status} — {} ({})",
                    terminal(lens.title()),
                    terminal(lens.id())
                )?;
            } else {
                writeln!(out, "{file}: {status}")?;
            }
            for failure in &result.failures {
                writeln!(
                    out,
                    "{file}: {}: {}",
                    terminal(&failure.element),
                    terminal(&failure.message)
                )?;
            }
            for missing in &result.missing_states {
                writeln!(out, "{file}: state {}: missing", terminal(missing))?;
            }
            for extra in &result.extra_states {
                writeln!(out, "{file}: state {}: extra", terminal(extra))?;
            }
            if check {
                for skipped in &result.not_checked {
                    writeln!(
                        out,
                        "{file}: {}: {}",
                        terminal(&skipped.element),
                        terminal(&skipped.message)
                    )?;
                }
            }
        }
        Ok(())
    })?;
    Ok(u8::from(
        check
            && view
                .lenses
                .iter()
                .any(|lens| lens.status != LensStatus::Current),
    ))
}

fn terminal(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    Cow::Owned(escaped)
}
