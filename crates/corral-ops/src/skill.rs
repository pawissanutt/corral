//! The corral agent skill. `VERBS` is the one verb list shared by the skill and the PM prompt.

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
        "new \"<title>\" -p <project> [--idea | --ready] [--body \"<markdown spec>\"] [--depends T-001,T-002]",
        "file a task (backlog by default); prints its id",
    ),
    ("move <id> <idea|backlog|ready|dropped>", "re-plan a task"),
    (
        "dispatch <id> [--agent <name>]",
        "start an agent run (ask the user first)",
    ),
    ("review <id> [--diff]", "what a finished run changed"),
    (
        "artifact list [--task <id>] [--json]",
        "reports, plans, diffs and evidence from past runs",
    ),
    (
        "artifact add <id> <path> --kind <report|plan|evidence|screenshot|log> --title \"<title>\"",
        "attach a file to a task",
    ),
];

/// The verb list as an indented markdown block, each verb prefixed with `bin`.
pub fn verbs_block(bin: &str) -> String {
    VERBS
        .iter()
        .map(|(verb, what)| format!("    {bin} {verb}\n        # {what}\n"))
        .collect()
}

/// The full SKILL.md text for the installed binary's version.
pub fn skill_markdown() -> String {
    include_str!("../assets/SKILL.md")
        .replace("{{marker}}", SKILL_MARKER)
        .replace("{{commands}}", verbs_block("corral").trim_end())
}
