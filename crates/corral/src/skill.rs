//! `corral skill`: prints the agent skill, or installs it into agent harness skill directories.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use corral_ops::{SKILL_HARNESSES as HARNESSES, SkillAction, install_role_skills, skill_markdown};

pub const HARNESS_NAMES: [&str; 4] = ["omo", "omp", "claude", "agents"];

pub fn print() {
    print!("{}", skill_markdown());
}

/// Writes `<skills dir>/<role>/SKILL.md` for every role skill and each target: the `--dir` override, else the
/// named harnesses, else every harness found in $HOME. Current files are left alone, stale managed copies are
/// refreshed, and a file without `SKILL_MARKER` was not written by corral and is refused unless `force`.
pub fn install(harnesses: &[String], dir: Option<PathBuf>, force: bool) -> Result<()> {
    let (messages, refusal) = install_report(harnesses, dir, force)?;
    for message in messages {
        println!("{message}");
    }
    match refusal {
        Some(refusal) => bail!("{refusal}"),
        None => Ok(()),
    }
}

/// Installs every role and returns one report line per written, refreshed or current role, plus the refusal
/// message when any role file was not written by corral (each refused role is also warned on stderr).
pub fn install_report(
    harnesses: &[String],
    dir: Option<PathBuf>,
    force: bool,
) -> Result<(Vec<String>, Option<String>)> {
    let targets = targets(harnesses, dir)?;
    if targets.is_empty() {
        bail!(
            "no agent harness found (looked for ~/.omo, ~/.omp, ~/.claude, ~/.agents); pass --harness or --dir"
        );
    }
    let mut refused = Vec::new();
    let mut messages = Vec::new();
    for (label, skills_dir) in targets {
        for write in install_role_skills(&skills_dir, force)? {
            let line = format!(
                "{label}/{}: {} {}",
                write.role,
                write.action.label(),
                write.path.display()
            );
            if write.action == SkillAction::Unmanaged {
                eprintln!("{line} (use --force to replace)");
                refused.push(format!("{label}/{}", write.role));
            } else {
                messages.push(line);
            }
        }
    }
    let refusal =
        (!refused.is_empty()).then(|| format!("skill not installed for: {}", refused.join(", ")));
    Ok((messages, refusal))
}

fn targets(harnesses: &[String], dir: Option<PathBuf>) -> Result<Vec<(String, PathBuf)>> {
    if let Some(dir) = dir {
        return Ok(vec![("dir".to_string(), dir)]);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    if harnesses.is_empty() {
        return Ok(HARNESSES
            .iter()
            .filter(|(_, marker, _)| home.join(marker).is_dir())
            .map(|(name, _, skills)| (name.to_string(), home.join(skills)))
            .collect());
    }
    harnesses
        .iter()
        .map(|h| {
            HARNESSES
                .iter()
                .find(|(name, ..)| name == h)
                .map(|(name, _, skills)| (name.to_string(), home.join(skills)))
                .ok_or_else(|| anyhow!("unknown harness '{h}' ({})", HARNESS_NAMES.join(", ")))
        })
        .collect()
}
