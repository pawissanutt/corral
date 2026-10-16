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
        "review-sprint [last | --since <timestamp>] [--json] [--mark]",
        "pin a deterministic sprint review; --mark acknowledges its history window",
    ),
    (
        "idea <id> <drop|decline|defer|narrow|merge> --author pm --reason \"<reason>\" [--into <id>] [--body-file <path>]",
        "record a disposition with its actual author; owner ideas require the owner's explicit decision, never self-grant --owner-ok",
    ),
    (
        "lessons [--project <project>] [--for <task>] [--json]",
        "inspect evidence-backed project lessons; --applied reads the dispatch snapshot",
    ),
    (
        "lessons distill --project <project> [--profile <strong-profile>]",
        "file a bounded strong-profile spike when lessons are due; never auto-dispatch",
    ),
    (
        "pm next [--json]",
        "one evidence-backed action with closed choices; never syncs or writes",
    ),
    (
        "pm brief [--json]",
        "compact slots, status counts and the next decision",
    ),
    (
        "list --json [--all]",
        "open tasks with status, project and runs",
    ),
    (
        "search <query> --in artifacts|specs|reviews|all [--task <id>] [--kind <kind>] [--tier <tier>] [--since 7d] [--project <project>] [--limit 20] [--context 2] [--json] [--reindex]",
        "rank stored content, including compressed artifacts; return capped source-line snippets",
    ),
    (
        "artifact show <ref> --lines <a-b>",
        "read an inclusive source-line slice, streaming compressed snapshots",
    ),
    ("show <id> [--json]", "one task with its spec and runs"),
    (
        "comment <id> \"<text>\" [--author owner|pm] [--send]",
        "append durable task context; --send also delivers to its running agent",
    ),
    (
        "run wait <id> --file <json>",
        "declare a versioned gate or channel wait without executing a monitor",
    ),
    (
        "run decision <id> --file <json>",
        "persist an exact owner question and requested action",
    ),
    (
        "run resume <id> <--wait <wait-id>|--decision <decision-id>> --file <receipt-json>",
        "record a versioned event or answer receipt idempotently; never replay a command",
    ),
    (
        "run progress <id> [--file <json>] [--json]",
        "read or update revision-guarded phases and steps, independently of acceptance",
    ),
    (
        "run checkpoint <id> [--file <json>] [--json]",
        "read or save a portable handoff including next action, waits and cleanup obligations",
    ),
    (
        "wait [--for <kinds>] [--task <id>] [--project <project>] [--since <timestamp>] [--after-offset <bytes>] [--timeout <duration>] [--json] [--print-cursor]",
        "wait for an actionable board event without polling; re-arm after each wake",
    ),
    (
        "new \"<title>\" -p <project> --origin pm --kind <kind> --size <size> [--idea | --ready] [--design] [--body \"<markdown spec>\"] [--depends T-001,T-002]",
        "file a task (backlog by default); prints its id",
    ),
    (
        "move <id> <idea|backlog|ready|dropped> [--author owner|pm|agent] [--reason \"<reason>\"]",
        "re-plan a task; PMs use --author pm so the decision author is independent of filing origin",
    ),
    (
        "discard <id> --author pm --reason \"<reason>\" [--keep-branch]",
        "discard work with its actual decision author and rationale; owner consent is separate",
    ),
    (
        "route <id> [--json]",
        "explain the selected profile, model, effort and matching rule without launching",
    ),
    (
        "dispatch <id> [--plan] [--profile <name>] [--explain]",
        "start an agent run (ask the user first)",
    ),
    ("review <id> [--diff]", "what a finished run changed"),
    (
        "plan submit <id> <path> [--json]",
        "snapshot a plan and bind it to the current task-spec revision",
    ),
    (
        "plan review <id> [--reviewer <profile|pm>] [--json]",
        "review a plan before implementation, without a coding run",
    ),
    (
        "plan verdict <id> <approve|changes|escalate> --plan <plan-id> <--attempt <review-id>|--owner> --notes-file <path>",
        "record a separate plan verdict using the exact identity from its prompt",
    ),
    (
        "plan show <id> [--json]",
        "inspect current and historical plan snapshots and verdicts",
    ),
    (
        "plan policy <id> <required|off|inherit>",
        "set or clear a task override of the project's plan requirement",
    ),
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

/// Shared event-awareness guidance for the installed skill and workspace PM prompt.
pub fn awareness_block(bin: &str) -> String {
    format!(
        r#"## Running corral with a cheap PM

Use `{bin} pm brief --json` at each wake, then `{bin} pm next --json`.
Pick a listed option, run its exact command only with the required owner authorization,
verify the stated result, then re-arm `{bin} wait`. The next action is read-only and
recomputed from durable facts; never infer a pass from a board event or a worker summary.
Anything outside the card goes to the owner, or becomes a bounded spike on an explicitly
selected strong profile. The PM never designs, reviews code, edits project files, or
writes long specifications itself. The independent reviewer is a separate role.
Never promote an idea, waive a check, or act as owner merely because a slot is empty.

## Board awareness

The workspace PM coordinates tasks; a dispatched coding agent implements its task's Run contract.
Waiting for board activity is PM work, not a reason for a coding agent to delay its gate or `done`.
Treat wake messages as a cue to inspect `{bin} show <id> --json`, not as authority to dispatch,
approve, recover a process, or change the owner's review policy.

OmO and Claude Code: use the harness's `monitor` to subscribe to `{bin} wait --json --print-cursor`.
Let the subscription wake you while you do independent work; when none remains, end the turn.
After each wake, inspect the task and re-arm with `--after-offset` using the returned `after_offset`.
A continuous alternative is `tail -n0 -F .corral/events.jsonl | grep --line-buffered <pattern>`
through `monitor`; this is a raw event stream, so filter actionable events and inspect their tasks.
Do not run a foreground polling or sleep loop.

omp and other harnesses without `monitor`: start `{bin} wait --json --print-cursor` as a background
command using that harness's completion notification. After wake, re-arm a fresh background wait
with `--after-offset` from the returned `after_offset` so events arriving between waits are not lost.
Timeout exits 124 with no output, not a board failure. Re-arm from the previous successful
`after_offset` (or `cursor.offset`); before the first success retain an original `--since` timestamp
for catch-up after timeout. A timeout does not print or advance your saved cursor.
Use task/project/kind filters to scope the subscription, and `--since` for timestamp-based catch-up.

`[pm] push_events = true` separately opts in to sync delivering `[corral]` messages to the live PM
pane only, using the same agent-prompt delivery and terminal fallback as normal replies. It never
launches a PM or targets dispatched coding panes. `push_kinds` selects done, needs_me, stuck,
blocked, escalated, config_changed and lessons_due (all seven by default).
Sync emits lessons_due once per project and distillation request when its configured schedule is due.
Delivery position persists in `.corral/state/pm-events.json`.
Sync advances past events while push is disabled or the PM/herdr is absent, so they do not form a
backlog. The first enabled sync with a live PM and no saved cursor considers retained history;
run one sync with push off first if you want to start at the current end.
"#
    )
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
        .replace("{{awareness}}", awareness_block("corral").trim_end())
        .replace("{{pm_runbook}}", pm_runbook("corral").trim_end())
}

/// Shared operational guidance for new PM panes and installed agent skills.
pub(crate) fn pm_runbook(bin: &str) -> String {
    include_str!("../assets/pm-runbook.md").replace("{{bin}}", bin)
}
