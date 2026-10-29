//! The classification of legacy files (plan section 12, import checks; mapping in
//! `.omo/evidence/T-335-legacy-inventory.md`): every file maps to a declared area the import
//! reads, or to a declared non-import; anything else refuses the migration.

use serde::Serialize;

/// What the import does with a legacy file. Every kept file is also stored as a blob listed in
/// `legacy_files`, so its exact bytes survive finalize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Task,
    Events,
    /// Stored as its recorded content and referenced from `artifact-blobs`.
    Artifact,
    /// A dispatch prompt, referenced from `run-prompts`.
    RunPrompt,
    /// A state file the import converts into the record its area names.
    Record,
    /// An authorized-submission journal, imported as an authorization row.
    Authorization,
    /// The agent inbox tree, imported as its raw record areas.
    Inbox,
    /// Evidence and history with no v2 reader of its own: kept as a blob only.
    Evidence,
    /// Derivable caches the v2 store rebuilds under its cache root.
    Cache,
    /// Uncommitted temporary files.
    Tmp,
    Lock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Area {
    pub name: &'static str,
    pub disposition: Disposition,
}

const fn area(name: &'static str, disposition: Disposition) -> Area {
    Area { name, disposition }
}

fn is_task_id(name: &str) -> bool {
    name.strip_prefix("T-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Run prompts are `<task>-<run>.md` with a run id `R<n>`; other prompt files are evidence.
fn is_run_prompt(name: &str) -> bool {
    name.strip_suffix(".md")
        .and_then(|stem| stem.rsplit_once('-'))
        .is_some_and(|(task, run)| {
            is_task_id(task)
                && run
                    .strip_prefix('R')
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// The area of a legacy path relative to `.corral`, or `None` when it is unknown.
pub fn classify(path: &str) -> Option<Area> {
    use Disposition::*;
    let parts: Vec<&str> = path.split('/').collect();
    Some(match parts.as_slice() {
        ["tasks", name] if name.ends_with(".md") => area("tasks", Task),
        ["events.jsonl"] => area("events", Events),
        ["herdr-session.json"] => area("session", Record),
        [name] if name.starts_with("launch-evidence") || name.starts_with("corral.toml.") => {
            area("workspace-evidence", Evidence)
        }
        ["artifacts", task, _run, _file] if is_task_id(task) => area("artifacts", Artifact),
        ["prompts", name] if is_run_prompt(name) => area("run-prompts", RunPrompt),
        ["prompts", _] => area("prompt-evidence", Evidence),
        ["inbox", ..] => area("inbox", Inbox),
        ["state", rest @ ..] => return classify_state(rest),
        _ => return None,
    })
}

fn classify_state(rest: &[&str]) -> Option<Area> {
    use Disposition::*;
    let file = *rest.last()?;
    if rest.len() == 1 && (file.starts_with(".tmp") || file.ends_with(".tmp")) {
        return Some(area("tmp", Tmp));
    }
    let json = |dir: &str| rest.len() == 2 && rest[0] == dir && file.ends_with(".json");
    Some(match rest {
        _ if json("action-authorizations") => area("authorizations", Authorization),
        _ if json("operating-claims") => area("operating-claims", Record),
        _ if json("operating-starts") => area("operating-starts", Record),
        _ if json("integration") => area("integration", Record),
        _ if json("availability") => area("availability", Record),
        _ if json("provider-stall") => area("provider-stall", Record),
        _ if json("review-prepared") => area("review-prepared", Record),
        _ if json("lessons") => area("lessons-distill", Record),
        _ if json("profile-proposals") => area("profile-proposals", Record),
        _ if json("post-land") => area("post-land", Record),
        ["provider-stall", name] if name.ends_with(".launch") => area("provider-session", Record),
        ["herdr-plugin", "herdr-plugin.toml"] => area("herdr-plugin", Record),
        ["artifact-decoded", ..] => area("artifact-decoded", Cache),
        ["content-search.json" | "content-search.fingerprint"] => area("content-search", Cache),
        ["harnesses.json"] => area("harnesses", Cache),
        ["pane-liveness.json"] => area("pane-liveness", Cache),
        ["content-search.lock"] => area("lock", Lock),
        [
            "gates" | "plan-gates" | "reviews" | "review-watch" | "watch" | "plans" | "post-land"
            | "herdr-plugin" | "schema-3-backup" | "tmp",
            ..,
        ] => area("evidence", Evidence),
        [name] => return classify_state_file(name),
        _ => return None,
    })
}

fn classify_state_file(name: &str) -> Option<Area> {
    use Disposition::*;
    let record = |area_name| Some(area(area_name, Record));
    match name {
        "automation.json" => record("automation"),
        "config-observation.json" => record("config-observation"),
        "max-diff-lines-warned" => record("config-warning"),
        "desktop-notifications.json" => record("desktop-notifications"),
        "pm.json" => record("pm"),
        "pm-decisions.json" => record("pm-decisions"),
        "pm-events.json" => record("pm-events"),
        "pm-inbox.json" => record("pm-inbox"),
        "pm-policy.json" => record("pm-policy"),
        "resources.json" => record("resources"),
        "resource-alert.json" => record("resource-alert"),
        "review-launch.json" => record("review-launch"),
        "inbox-send-budget.json" => record("inbox-send-budget"),
        "reviewed-artifacts.json" => record("reviewed-artifacts"),
        "artifact-server.json" => record("artifact-server"),
        "profile-setup.json" => record("profile-setup"),
        "profile-selections.json" => record("profile-selections"),
        "profile-repairs.json" => record("profile-repairs"),
        "profile-setup-intent.json" => record("profile-setup-intent"),
        "tour-seen" => record("tour-seen"),
        _ if name.starts_with("main-movement-") && name.ends_with(".json") => {
            let key = &name["main-movement-".len()..name.len() - ".json".len()];
            if is_task_id(key) {
                record("main-movement")
            } else {
                Some(area("evidence", Evidence))
            }
        }
        _ if name.starts_with("plan-decompose-") && name.ends_with(".json") => {
            record("plan-decompose")
        }
        _ if name.starts_with("plan-review-") && name.ends_with(".json") => {
            record("plan-review-prepared")
        }
        _ if name.ends_with(".md") => Some(area("evidence", Evidence)),
        _ => None,
    }
}
