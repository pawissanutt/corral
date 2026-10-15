//! `corral skill`: prints the agent skill, or installs it into agent harness skill directories.

use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use corral_ops::{SKILL_HARNESSES as HARNESSES, SKILL_MARKER, skill_markdown};

pub const HARNESS_NAMES: [&str; 4] = ["omo", "omp", "claude", "agents"];

pub fn print() {
    print!("{}", skill_markdown());
}

/// Writes `<skills dir>/corral/SKILL.md` for each target: the `--dir` override, else the named harnesses,
/// else every harness found in $HOME. Identical files are left alone; a file without `SKILL_MARKER` was not
/// written by corral and is refused unless `force`.
pub fn install(harnesses: &[String], dir: Option<PathBuf>, force: bool) -> Result<()> {
    let targets = targets(harnesses, dir)?;
    if targets.is_empty() {
        bail!(
            "no agent harness found (looked for ~/.omo, ~/.omp, ~/.claude, ~/.agents); pass --harness or --dir"
        );
    }
    let text = skill_markdown();
    let mut refused = Vec::new();
    for (label, skills_dir) in targets {
        let file = skills_dir.join("corral").join("SKILL.md");
        match std::fs::read_to_string(&file) {
            Ok(old) if old == text => {
                println!("{label}: up to date {}", file.display());
                continue;
            }
            Ok(old) if !force && !old.contains(SKILL_MARKER) => {
                eprintln!(
                    "{label}: {} exists and was not written by corral; left it (use --force to replace)",
                    file.display()
                );
                refused.push(label);
                continue;
            }
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("reading {}", file.display())),
        }
        let parent = skills_dir.join("corral");
        std::fs::create_dir_all(&parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        std::fs::write(&file, &text).with_context(|| format!("writing {}", file.display()))?;
        println!("{label}: wrote {}", file.display());
    }
    if !refused.is_empty() {
        bail!("skill not installed for: {}", refused.join(", "));
    }
    Ok(())
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
