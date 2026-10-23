//! `corral skill`: prints the agent skill, or installs it into agent harness skill directories.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use corral_ops::{
    SKILL_HARNESSES as HARNESSES, SkillAction, install_harness_skills, install_role_skills,
    skill_markdown,
};

pub const HARNESS_NAMES: [&str; 4] = ["omo", "omp", "claude", "agents"];

/// Where one report label installs: a bare skills directory, or a harness under $HOME (which may share another
/// harness's directory).
enum Target {
    Dir(PathBuf),
    Harness(&'static str, PathBuf),
}

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
    // Harnesses sharing a directory resolve to the same files; report each file once.
    let mut seen = HashSet::new();
    for (label, target) in targets {
        let writes = match target {
            Target::Dir(dir) => install_role_skills(&dir, force)?,
            Target::Harness(name, home) => install_harness_skills(&home, name, force)?,
        };
        for write in writes {
            if !seen.insert(write.path.clone()) {
                continue;
            }
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

fn targets(harnesses: &[String], dir: Option<PathBuf>) -> Result<Vec<(String, Target)>> {
    if let Some(dir) = dir {
        return Ok(vec![("dir".to_string(), Target::Dir(dir))]);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    if harnesses.is_empty() {
        return Ok(HARNESSES
            .iter()
            .filter(|(_, marker, _)| home.join(marker).is_dir())
            .map(|(name, ..)| (name.to_string(), Target::Harness(name, home.clone())))
            .collect());
    }
    harnesses
        .iter()
        .map(|h| {
            HARNESSES
                .iter()
                .find(|(name, ..)| name == h)
                .map(|(name, ..)| (name.to_string(), Target::Harness(name, home.clone())))
                .ok_or_else(|| anyhow!("unknown harness '{h}' ({})", HARNESS_NAMES.join(", ")))
        })
        .collect()
}
