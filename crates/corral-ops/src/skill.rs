//! The corral agent skill. `VERBS` is the one verb list shared by the skill and the PM prompt.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// (harness, home-relative dir whose presence means it is installed, home-relative skills dir)
pub const SKILL_HARNESSES: &[(&str, &str, &str)] = &[
    ("omo", ".omo", ".omo/agent/skills"),
    ("omp", ".omp", ".omp/agent/skills"),
    ("claude", ".claude", ".claude/skills"),
    ("agents", ".agents", ".agents/skills"),
];

/// First line of the skill body; `corral skill install` only overwrites files that carry it.
pub const SKILL_MARKER: &str =
    "<!-- managed by `corral skill install`; local edits are overwritten -->";

/// The corral verbs for agents that plan and inspect work (not dispatched runs), with what each is for.
pub const VERBS: &[(&str, &str)] = &[
    (
        "list --json [--all]",
        "open tasks with status, project and runs",
    ),
    ("show <id> [--json]", "one task with its spec and runs"),
    (
        "new \"<title>\" -p <project> --kind <kind> --size <size> [--idea | --ready] [--body \"<markdown spec>\"] [--depends T-001,T-002]",
        "file a task (backlog by default); prints its id",
    ),
    ("move <id> <idea|backlog|ready|dropped>", "re-plan a task"),
    (
        "route <id> [--json]",
        "explain the selected profile, model, effort and matching rule without launching",
    ),
    (
        "dispatch <id> [--profile <name>] [--explain]",
        "start an agent run (ask the user first)",
    ),
    ("review <id> [--diff]", "what a finished run changed"),
    (
        "autopilot status",
        "whether the review autopilot is on and where it has each task",
    ),
    (
        "verdict <id> <approve|changes|escalate> --notes-file <path>",
        "record your verdict when asked to review a run",
    ),
    (
        "artifact list [--task <id>] [--json]",
        "reports, plans, diffs and evidence from past runs",
    ),
    (
        "artifact add <id> <path> --kind <report|plan|evidence|screenshot|log> --title \"<title>\"",
        "attach a file to a task",
    ),
    (
        "pin <T-042 | T-042:R1:<artifact-file>> [--label \"<label>\"] [--follow-latest]",
        "pin a task or artifact the user will revisit; --follow-latest tracks reports later runs regenerate",
    ),
    (
        "pins [--json]",
        "pinned tasks and artifacts in the user's order; broken pins are flagged",
    ),
    (
        "open <ref | pin-number>",
        "open a pinned or named task/artifact for the user",
    ),
];

/// The verb list as an indented markdown block, each verb prefixed with `bin`.
pub fn verbs_block(bin: &str) -> String {
    VERBS
        .iter()
        .map(|(verb, what)| format!("    {bin} {verb}\n        # {what}\n"))
        .collect()
}

/// Dispatch-time install: writes `<skills dir>/corral/SKILL.md` for `harness` (a `SKILL_HARNESSES` name) when
/// that harness is installed under `home` and has no corral skill yet. An existing file, corral's or foreign,
/// is left alone, so this is idempotent. Returns the written path, or None when nothing was written.
pub fn ensure_skill(home: &Path, harness: &str) -> Result<Option<PathBuf>> {
    let Some((_, marker, skills)) = SKILL_HARNESSES.iter().find(|(name, ..)| *name == harness)
    else {
        return Ok(None);
    };
    if !home.join(marker).is_dir() {
        return Ok(None);
    }
    let dir = home.join(skills).join("corral");
    let file = dir.join("SKILL.md");
    match std::fs::symlink_metadata(&file) {
        Ok(_) => return Ok(None),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("checking {}", file.display())),
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(&file, skill_markdown())
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(Some(file))
}

/// The full SKILL.md text for the installed binary's version.
pub fn skill_markdown() -> String {
    include_str!("../assets/SKILL.md")
        .replace("{{marker}}", SKILL_MARKER)
        .replace("{{commands}}", verbs_block("corral").trim_end())
}
