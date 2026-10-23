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

/// (harness, shared harness): the harness's loader also reads the shared harness's skills dir, so when that
/// dir's home marker exists corral installs there only; a second copy would be reported as a collision.
pub const SKILL_SHARED: &[(&str, &str)] = &[("omo", "agents")];

/// First line of the skill body; `corral skill install` only overwrites files that carry it.
pub const SKILL_MARKER: &str =
    "<!-- managed by `corral skill install`; local edits are overwritten -->";

pub const MODEL_SELECTION_VERSION: &str = "pm-model-selection-v1";
pub const MODEL_SELECTION_PROCEDURE: &str = include_str!("../../../docs/pm/model-selection.md");

/// The corral verbs for agents that plan and inspect work (not dispatched runs), with what each is for.
pub const VERBS: &[(&str, &str)] = &[
    (
        "setup status|catalog|discover|procedure [--json]",
        "read durable setup answers, current mappings and policy, local inventory or the shipped selection procedure",
    ),
    (
        "setup bootstrap --harness <key> [--model <id>] [--effort <level>] [--confirm]",
        "preview one existing connection; --confirm starts or reuses the setup PM without a profile catalog",
    ),
    (
        "setup begin --expected-revision <n>",
        "resume the interview, retaining saved answers and any pending proposal",
    ),
    (
        "setup answers --expected-revision <n> --file <json-map>",
        "merge non-secret access and preference answers while gathering",
    ),
    (
        "setup validate|propose --file <json> [--expected-revision <n>]",
        "validate exact settings or save a sourced/provisional proposal for owner review",
    ),
    (
        "setup diff|apply",
        "preview or apply only the exact approved setup; never dispatches work",
    ),
    (
        "setup approve --file <json> --expected-revision <n>",
        "record the owner's exact approval and any evidence-gap acknowledgement",
    ),
    (
        "setup cancel --expected-revision <n>",
        "cancel without applying; retain answers for reopening",
    ),
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
        "read authorized actions, pending decisions and automatic waits; never syncs or writes",
    ),
    (
        "pm brief [--json]",
        "compact slots, status counts and the next decision",
    ),
    (
        "pm act <action-id> [--json]",
        "revalidate a revision-bound automatic action through existing executors; never execute cached shell text",
    ),
    (
        "pm present <decision-id> --delivery <receipt-id> [--confirmed]",
        "record presentation only; omission of --confirmed preserves delivery uncertainty and the pending answer",
    ),
    (
        "pm resolve <decision-id> --answer \"<actual owner answer>\"",
        "record an actual workspace decision response; native and portable questions use their matching response operations",
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
        "log <id> [--all] [--json]",
        "task lifecycle timeline, oldest first; --all includes noise; --json emits one entry per line",
    ),
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
        "import <file> [--dry-run] [--json] [--origin owner|pm|agent]",
        "import a Markdown plan as a non-dispatchable tracker and dependent children; --dry-run previews IDs and edges without writes; PMs and agents pass their own origin",
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
        "explicit owner start; PM automatic work uses revision-bound pm act instead",
    ),
    ("review <id> [--diff]", "what a finished run changed"),
    (
        "plan cancel <id> [--json]",
        "end a planner session while retaining plan snapshots and verdicts",
    ),
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
        "inspect legacy executor state; only a confirmed finite operating grant delegates automatic work",
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
Read the shared workset: executable actions, pending decisions, waits, scoped progress,
policy revision and whether independent work continues. Present each new decision once,
then acknowledge its exact identity with `pm present`; presentation is not an answer.
An unconfirmed presentation remains delivery-uncertain and must not be blindly resent.
Execute an authorized action with `{bin} pm act <action-id> --json`, inspect its durable
outcome and recompute until no progress or an external wait. Pending questions do not
block independent authorized work. Assist requires explicit owner start/acceptance;
scoped Autopilot delegates only the finite confirmed workload and listed authority.
Readbacks never grant permission; never infer a pass from a board event or worker summary.
Anything outside the card goes to the owner, or becomes a bounded spike on an explicitly
selected strong profile. The PM never designs, reviews code, edits project files, or
writes long specifications itself. The independent reviewer is a separate role.
Profile setup, model research and custom-profile proposals are PM configuration work:
follow the shipped model-selection procedure and obtain the owner's exact approval.
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
blocked, escalated, config_changed, lessons_due, run_resumed, wait_transition and
pm_decision_resolved (all ten by default). Delivery and presentation accounting never wakes
its own subscription.
Sync emits lessons_due once per project and distillation request when its configured schedule is due.
Delivery position persists in `.corral/state/pm-events.json`.
Effective-policy replacement is separate and mandatory for a remembered live PM,
even with push disabled. Its confirmed, refused or uncertain delivery is visible in
brief health and survives restart; it does not restart workers or replay commands.
Sync advances past events while push is disabled or the PM/herdr is absent, so they do not form a
backlog. The first enabled sync with a live PM and no saved cursor considers retained history;
run one sync with push off first if you want to start at the current end.
"#
    )
}

/// The PM skill: board coordination, setup and model selection.
pub const PM_SKILL: &str = "corral";
/// The dispatched coding run's skill, named by the run prompt.
pub const WORKER_SKILL: &str = "corral-worker";
/// The planning session's skill, named by the plan prompt.
pub const PLANNER_SKILL: &str = "corral-planner";
/// The filing agent's skill.
pub const SUBMIT_SKILL: &str = "corral-submit";
/// Every managed role skill, each installed as `<skills dir>/<name>/SKILL.md`. Reviewers have none:
/// their guidance lives in the versioned review contracts.
pub const ROLE_SKILLS: [&str; 4] = [PM_SKILL, WORKER_SKILL, PLANNER_SKILL, SUBMIT_SKILL];

/// What installing one role skill did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillAction {
    /// No file existed; the skill was written.
    Wrote,
    /// A stale managed copy (or a forced foreign file) was replaced.
    Refreshed,
    /// The managed copy already matched this binary; nothing was written.
    Current,
    /// The file was not written by corral and was left byte-identical.
    Unmanaged,
    /// A corral-owned duplicate in a directory the harness shares with another was removed.
    Removed,
}

impl SkillAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wrote => "wrote",
            Self::Refreshed => "refreshed",
            Self::Current => "up to date",
            Self::Unmanaged => "skipped (not written by corral)",
            Self::Removed => "removed duplicate",
        }
    }
}

/// One role skill's install outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillWrite {
    pub role: &'static str,
    pub path: PathBuf,
    pub action: SkillAction,
}

/// Dispatch-time install for `harness` (a `SKILL_HARNESSES` name) when that harness is installed under `home`:
/// writes missing role skills and refreshes stale managed copies; foreign files are reported as `Unmanaged`
/// and never touched. Empty when the harness is unknown or not installed.
pub fn ensure_skills(home: &Path, harness: &str) -> Result<Vec<SkillWrite>> {
    let Some((_, marker, _)) = SKILL_HARNESSES.iter().find(|(name, ..)| *name == harness) else {
        return Ok(Vec::new());
    };
    if !home.join(marker).is_dir() {
        return Ok(Vec::new());
    }
    install_harness_skills(home, harness, false)
}

fn harness_row(name: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    SKILL_HARNESSES.iter().find(|(n, ..)| *n == name)
}

/// The shared harness whose skills dir also serves `harness`, when its home marker exists under `home`.
fn shared_row(
    home: &Path,
    harness: &str,
) -> Option<&'static (&'static str, &'static str, &'static str)> {
    let (_, shared) = SKILL_SHARED.iter().find(|(h, _)| *h == harness)?;
    harness_row(shared).filter(|(_, marker, _)| home.join(marker).is_dir())
}

/// The skills dir corral installs into for `harness`: the shared dir when it applies, else the harness's own.
pub fn skills_dir_for(home: &Path, harness: &str) -> Option<PathBuf> {
    let (_, _, own) = harness_row(harness)?;
    let dir = shared_row(home, harness).map_or(*own, |(_, _, shared)| *shared);
    Some(home.join(dir))
}

/// Installs every role skill for `harness` into `skills_dir_for`, then removes the corral-owned copies that
/// the harnesses loading that dir would duplicate in their own dirs (whichever of them was named). A copy
/// that resolves to the same file as the installed one is an alias, never a duplicate. An unmanaged
/// installed copy keeps the own copy.
pub fn install_harness_skills(home: &Path, harness: &str, force: bool) -> Result<Vec<SkillWrite>> {
    let Some(own) = harness_row(harness) else {
        return Ok(Vec::new());
    };
    let serving = shared_row(home, harness).unwrap_or(own);
    let mut writes = install_role_skills(&home.join(serving.2), force)?;
    let mut removed = Vec::new();
    let consumers = SKILL_SHARED
        .iter()
        .filter(|(_, shared)| *shared == serving.0);
    for (consumer, _) in consumers {
        let Some((_, _, consumer_dir)) = harness_row(consumer) else {
            continue;
        };
        for write in writes.iter().filter(|w| w.action != SkillAction::Unmanaged) {
            let path = home.join(consumer_dir).join(write.role).join("SKILL.md");
            if distinct_owned_copy(&path, &write.path) {
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
                // Only an empty directory goes; anything else in it is not corral's.
                let _ = path.parent().map(std::fs::remove_dir);
                removed.push(SkillWrite {
                    role: write.role,
                    path,
                    action: SkillAction::Removed,
                });
            }
        }
    }
    writes.extend(removed);
    Ok(writes)
}

/// Corral-owned copies in `harness`'s own skills dir that its shared dir already serves.
pub fn duplicate_skills(home: &Path, harness: &str) -> Vec<PathBuf> {
    let (Some((_, _, own)), Some((_, _, shared))) =
        (harness_row(harness), shared_row(home, harness))
    else {
        return Vec::new();
    };
    ROLE_SKILLS
        .iter()
        .map(|role| {
            (
                home.join(own).join(role).join("SKILL.md"),
                home.join(shared).join(role).join("SKILL.md"),
            )
        })
        .filter(|(path, served)| distinct_owned_copy(path, served))
        .map(|(path, _)| path)
        .collect()
}

/// A corral-owned `path` that is a different physical file from `served`; two roots reaching one file
/// (for example through a symlinked ancestor directory) are one copy.
fn distinct_owned_copy(path: &Path, served: &Path) -> bool {
    let same_file = match (std::fs::canonicalize(path), std::fs::canonicalize(served)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    corral_owned(path) && !same_file
}

/// A regular `SKILL.md` carrying `SKILL_MARKER` in a real (non-symlinked) role directory.
fn corral_owned(path: &Path) -> bool {
    let real_dir = path
        .parent()
        .and_then(|dir| std::fs::symlink_metadata(dir).ok())
        .is_some_and(|meta| meta.file_type().is_dir());
    let regular = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file());
    real_dir && regular && std::fs::read(path).is_ok_and(|text| contains(&text, SKILL_MARKER))
}

/// Installs every role skill under `skills_dir`. A file without `SKILL_MARKER` is left alone unless `force`.
pub fn install_role_skills(skills_dir: &Path, force: bool) -> Result<Vec<SkillWrite>> {
    ROLE_SKILLS
        .iter()
        .map(|&role| {
            let dir = skills_dir.join(role);
            let path = dir.join("SKILL.md");
            let text = role_skill_markdown(role).context("unknown role skill")?;
            let action = match std::fs::symlink_metadata(&path) {
                Err(err) if err.kind() == ErrorKind::NotFound => SkillAction::Wrote,
                Err(err) => {
                    return Err(err).with_context(|| format!("checking {}", path.display()));
                }
                Ok(_) => match std::fs::read(&path) {
                    Ok(old) if old == text.as_bytes() => SkillAction::Current,
                    Ok(old) if force || contains(&old, SKILL_MARKER) => SkillAction::Refreshed,
                    Ok(_) => SkillAction::Unmanaged,
                    // A dangling symlink or unreadable entry is not corral's to replace.
                    Err(_) if !force => SkillAction::Unmanaged,
                    Err(_) => SkillAction::Refreshed,
                },
            };
            if matches!(action, SkillAction::Wrote | SkillAction::Refreshed) {
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("creating {}", dir.display()))?;
                std::fs::write(&path, &text)
                    .with_context(|| format!("writing {}", path.display()))?;
            }
            Ok(SkillWrite { role, path, action })
        })
        .collect()
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// The managed marker line for `role`: `SKILL_MARKER` followed by the binary version and the SHA-256 digest of
/// the unstamped body, so an upgrade or content change makes an installed copy stale.
pub fn skill_stamp(role: &str, unstamped: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(unstamped.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{SKILL_MARKER}\n<!-- corral-skill: {role} version={} sha256={hex} -->",
        env!("CARGO_PKG_VERSION")
    )
}

/// The full versioned SKILL.md text of a role skill, or None for an unknown role.
pub fn role_skill_markdown(role: &str) -> Option<String> {
    let body = match role {
        PM_SKILL => pm_skill_body(),
        WORKER_SKILL => include_str!("../assets/skills/corral-worker.md").to_string(),
        PLANNER_SKILL => include_str!("../assets/skills/corral-planner.md").to_string(),
        SUBMIT_SKILL => include_str!("../assets/skills/corral-submit.md").to_string(),
        _ => return None,
    };
    let unstamped = body.replace("{{marker}}", SKILL_MARKER);
    Some(body.replace("{{marker}}", &skill_stamp(role, &unstamped)))
}

/// The full PM SKILL.md text for the installed binary's version.
pub fn skill_markdown() -> String {
    role_skill_markdown(PM_SKILL).unwrap_or_default()
}

fn pm_skill_body() -> String {
    include_str!("../assets/SKILL.md")
        .replace("{{commands}}", verbs_block("corral").trim_end())
        .replace("{{awareness}}", awareness_block("corral").trim_end())
        .replace("{{pm_runbook}}", pm_runbook("corral").trim_end())
}

/// Shared operational guidance for new PM panes and installed agent skills.
pub(crate) fn pm_runbook(bin: &str) -> String {
    format!(
        "{}\n\n{}",
        include_str!("../assets/pm-runbook.md"),
        MODEL_SELECTION_PROCEDURE
    )
    .replace("{{bin}}", bin)
    .replace("corral setup ", &format!("{bin} setup "))
}
