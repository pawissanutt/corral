//! The closed owner-action kinds: each is exactly one existing `corral` command with the flags
//! that command takes for the owner, and a class. Ordinary kinds run on the owner's instruction;
//! the confirm class (dropping or force-moving a task, discarding one, config and automation
//! changes) waits for the owner's answer to a confirmation the host renders.
use anyhow::Result;
use corral_core::{Status, Store};

use crate::agent::{RejectCode, Rejection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Ordinary,
    Confirm,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Ordinary => "ordinary",
            Class::Confirm => "confirm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A task ID, normalized before it reaches the command.
    Task,
    Key,
    /// One of a fixed set of subcommands.
    Choice(&'static [&'static str]),
}

/// One kind: the command words, its target, further positionals and the owner flags it takes.
#[derive(Debug)]
pub struct Spec {
    pub kind: &'static str,
    pub words: &'static [&'static str],
    pub target: Target,
    pub positionals: &'static [&'static str],
    /// Flag and whether it takes a value. Attribution, root and output flags are never offered.
    pub flags: &'static [(&'static str, bool)],
    /// The payload's `expected_revision` becomes the command's `--expected-revision`.
    pub revision: bool,
}

const ROUTING: [(&str, bool); 5] = [
    ("--profile", true),
    ("--model", true),
    ("--effort", true),
    ("--base", true),
    ("--headless", false),
];

const fn spec(
    kind: &'static str,
    words: &'static [&'static str],
    positionals: &'static [&'static str],
    flags: &'static [(&'static str, bool)],
) -> Spec {
    Spec {
        kind,
        words,
        target: Target::Task,
        positionals,
        flags,
        revision: false,
    }
}

pub const KINDS: [Spec; 19] = [
    spec(
        "approve",
        &["approve"],
        &[],
        &[
            ("--merge", true),
            ("--keep-worktree", false),
            ("--keep-branch", false),
        ],
    ),
    spec(
        "verdict",
        &["verdict"],
        &["verdict"],
        &[
            ("--notes", true),
            ("--notes-file", true),
            ("--attempt", true),
            ("--stronger", false),
        ],
    ),
    spec("review_run", &["review-run"], &[], &[("--reviewer", true)]),
    spec(
        "dispatch",
        &["dispatch"],
        &[],
        &[
            ROUTING[0],
            ROUTING[1],
            ROUTING[2],
            ROUTING[3],
            ROUTING[4],
            ("--plan", false),
            ("--queue", false),
            ("--force", false),
        ],
    ),
    spec("queue", &["queue", "add"], &[], &ROUTING),
    spec("unqueue", &["queue", "remove"], &[], &[]),
    spec(
        "move",
        &["move"],
        &["status"],
        &[
            ("--force", false),
            ("--owner-ok", false),
            ("--reason", true),
        ],
    ),
    spec(
        "park",
        &["attention", "park"],
        &[],
        &[
            ("--reason", true),
            ("--resume-when", true),
            ("--resume-action", true),
            ("--review-by", true),
        ],
    ),
    Spec {
        revision: true,
        ..spec("unpark", &["attention", "unpark"], &[], &[])
    },
    spec("revert", &["revert"], &[], &[]),
    spec(
        "plan_accept",
        &["plan", "accept"],
        &[],
        &[("--attempt", true)],
    ),
    spec(
        "plan_verdict",
        &["plan", "verdict"],
        &["verdict"],
        &[
            ("--plan", true),
            ("--attempt", true),
            ("--owner", false),
            ("--notes-file", true),
        ],
    ),
    spec(
        "plan_review",
        &["plan", "review"],
        &[],
        &[("--reviewer", true)],
    ),
    Spec {
        revision: true,
        ..spec(
            "plan_member_gate",
            &["plan", "member-gate"],
            &[],
            &[("--file", true), ("--reason", true), ("--tracker", true)],
        )
    },
    spec("plan_add_member", &["plan", "add-member"], &["member"], &[]),
    spec(
        "plan_move_member",
        &["plan", "move-member"],
        &["member"],
        &[("--to", true), ("--gate-file", true), ("--reason", true)],
    ),
    spec(
        "discard",
        &["discard"],
        &[],
        &[
            ("--keep-branch", false),
            ("--owner-ok", false),
            ("--reason", true),
        ],
    ),
    Spec {
        target: Target::Key,
        ..spec(
            "config_set",
            &["config", "set"],
            &["value"],
            &[("--user", false), ("--workspace", false)],
        )
    },
    Spec {
        target: Target::Choice(&["autopilot", "assist", "turbo", "pause", "resume"]),
        ..spec(
            "mode",
            &["mode"],
            &[],
            &[
                ("--project", true),
                ("--include-new", false),
                ("--expires-after-days", true),
                ("--max-new-tasks", true),
                ("--task", true),
                ("--exclude", true),
                ("--plan-scope", true),
                ("--dispatch", false),
                ("--accept", false),
                ("--plan-accept", false),
                ("--aggregate-accept", false),
                ("--confirm", true),
                ("--off", false),
            ],
        )
    },
];

/// One resolved action: the exact argv the command runs with and how it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub kind: &'static str,
    pub argv: Vec<String>,
    /// `corral <argv>`, shell-quoted, exactly as a confirmation shows it.
    pub command: String,
    pub class: Class,
    pub target: String,
    /// The task the action names, checked for an in-flight holder before it runs.
    pub task: Option<String>,
}

fn bad(reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

fn unsupported(reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::UnsupportedAction, reason).into()
}

fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/=@%+,-".contains(&b));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

pub fn render(argv: &[String]) -> String {
    let mut words = vec!["corral".to_owned()];
    words.extend(argv.iter().map(|word| quote(word)));
    words.join(" ")
}

fn target(spec: &Spec, target: Option<&str>) -> Result<String> {
    let target = target.ok_or_else(|| bad(format!("{} needs a target", spec.kind)))?;
    match spec.target {
        Target::Task => {
            Store::normalize_id(target).map_err(|_| bad(format!("{target} is not a task ID")))
        }
        Target::Key if !target.is_empty() && !target.starts_with('-') => {
            if target.chars().any(char::is_whitespace) {
                return Err(bad("a config key is one word"));
            }
            Ok(target.to_owned())
        }
        Target::Key => Err(bad("a config key is one word")),
        Target::Choice(choices) if choices.contains(&target) => Ok(target.to_owned()),
        Target::Choice(choices) => Err(bad(format!(
            "{} takes one of {}",
            spec.kind,
            choices.join(", ")
        ))),
    }
}

/// The action for a payload's kind, target, arguments and expected revision, or a refusal
/// before any effect: `unsupported_action` for a kind outside the closed set, `bad_request`
/// for a flag or positional that command does not take.
pub fn resolve(
    kind: &str,
    target_word: Option<&str>,
    args: &[String],
    expected_revision: Option<u64>,
) -> Result<Action> {
    if kind == "finish" {
        return Err(unsupported(
            "finish lands with the cowork-landing member (corral assistant scratch finish); \
             post the request with corral pm inbox post or tell the owner",
        ));
    }
    let spec = KINDS.iter().find(|spec| spec.kind == kind).ok_or_else(|| {
        unsupported(format!(
            "{kind} is not an owner action; route it to the PM with corral pm inbox post"
        ))
    })?;
    let target = target(spec, target_word)?;
    let mut argv: Vec<String> = spec.words.iter().map(|w| (*w).to_owned()).collect();
    argv.push(target.clone());
    let mut positionals = Vec::new();
    let mut forced = false;
    let mut tokens = args.iter();
    while let Some(token) = tokens.next() {
        if !token.starts_with('-') {
            positionals.push(token.as_str());
            argv.push(token.clone());
            continue;
        }
        let Some((flag, takes)) = spec.flags.iter().find(|(flag, _)| flag == token) else {
            let allowed: Vec<&str> = spec.flags.iter().map(|(flag, _)| *flag).collect();
            return Err(bad(format!(
                "{kind} takes no {token}; its flags are: {}",
                if allowed.is_empty() {
                    "none".to_owned()
                } else {
                    allowed.join(" ")
                }
            )));
        };
        forced |= *flag == "--force";
        argv.push(token.clone());
        if *takes {
            let value = tokens
                .next()
                .ok_or_else(|| bad(format!("{token} needs a value")))?;
            argv.push(value.clone());
        }
    }
    if positionals.len() != spec.positionals.len() {
        return Err(bad(format!(
            "{kind} takes the target and {} more argument(s): {}",
            spec.positionals.len(),
            spec.positionals.join(" ")
        )));
    }
    match (spec.revision, expected_revision) {
        (true, Some(revision)) => argv.extend(["--expected-revision".into(), revision.to_string()]),
        (false, Some(_)) => {
            return Err(bad(format!(
                "{kind} takes no expected_revision; only unpark and plan_member_gate do"
            )));
        }
        (_, None) => {}
    }
    let class = match kind {
        "move" => {
            let status = Status::parse(positionals[0])
                .ok_or_else(|| bad(format!("unknown status {}", positionals[0])))?;
            if status == Status::Dropped || forced {
                Class::Confirm
            } else {
                Class::Ordinary
            }
        }
        "discard" | "config_set" | "mode" => Class::Confirm,
        _ => Class::Ordinary,
    };
    Ok(Action {
        kind: spec.kind,
        command: render(&argv),
        argv,
        class,
        task: (spec.target == Target::Task).then(|| target.clone()),
        target,
    })
}
