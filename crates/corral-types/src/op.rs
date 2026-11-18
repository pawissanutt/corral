use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{NewTask, Priority, Status, TaskEdit, Usage};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    New {
        task: NewTask,
    },
    Edit {
        id: String,
        edit: TaskEdit,
    },
    Comment {
        id: String,
        text: String,
    },
    Move {
        id: String,
        status: Status,
    },
    Dispatch {
        id: String,
        profile: Option<String>,
        base: Option<String>,
    },
    Reply {
        id: String,
        text: String,
    },
    Stop {
        id: String,
    },
    Retry {
        id: String,
        profile: Option<String>,
        fresh: bool,
        note: Option<String>,
    },
    Done {
        id: String,
        summary: String,
    },
    Gate {
        id: String,
    },
    Review {
        id: String,
        profile: Option<String>,
    },
    Verdict {
        id: String,
        approve: bool,
        notes: Option<String>,
    },
    Changes {
        id: String,
        text: String,
    },
    Approve {
        id: String,
    },
    Drop {
        id: String,
    },
    Reopen {
        id: String,
    },
    ArtifactAdd {
        id: String,
        path: PathBuf,
        kind: Option<String>,
        title: Option<String>,
    },
    UsageAdd {
        usage: Usage,
    },
    Sync,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Next {
    pub op: Op,
    pub force: bool,
}

impl Next {
    pub const fn run(op: Op) -> Self {
        Self { op, force: false }
    }

    pub const fn force(op: Op) -> Self {
        Self { op, force: true }
    }

    /// Display the CLI equivalent; editor-only and wire-only data stays in `op`.
    pub fn command(&self) -> String {
        self.to_string()
    }

    /// Render a command for the program name or path used by the caller.
    pub fn command_with_program(&self, program: &str) -> String {
        self.command_with_args(program, &[])
    }

    /// Render a command with global arguments such as an explicit workspace root.
    pub fn command_with_args(&self, program: &str, args: &[&str]) -> String {
        fmt::from_fn(|f| self.fmt_command(f, program, args)).to_string()
    }
}

impl fmt::Display for Next {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_command(f, "corral", &[])
    }
}

impl Next {
    fn fmt_command(&self, f: &mut fmt::Formatter<'_>, program: &str, args: &[&str]) -> fmt::Result {
        shell_word(f, program)?;
        positionals(f, args)?;
        f.write_str(" ")?;
        match &self.op {
            Op::New { task } => invocation(f, "new", &[&task.title], |f| {
                optional(f, "-p", task.project.as_deref())?;
                match task.status {
                    Status::Idea => f.write_str(" --idea")?,
                    Status::Ready => f.write_str(" --ready")?,
                    _ => (),
                }
                if task.priority != Priority::P2 {
                    option(f, "--priority", task.priority.as_str())?;
                }
                optional(f, "--kind", task.kind.as_deref())?;
                optional(f, "--size", task.size.as_deref())?;
                optional(f, "--parent", task.parent.as_deref())?;
                if let Some((first, rest)) = task.depends_on.split_first() {
                    option(f, "--depends-on", first)?;
                    for dependency in rest {
                        f.write_str(",")?;
                        shell_word(f, dependency)?;
                    }
                }
                Ok(())
            }),
            Op::Edit { id, .. } => plain(f, "edit", &[id]),
            Op::Comment { id, text } => plain(f, "comment", &[id, text]),
            Op::Move { id, status } => invocation(f, "move", &[id, status.as_str()], |f| {
                flag(f, " --force", self.force)
            }),
            Op::Dispatch { id, profile, base } => invocation(f, "dispatch", &[id], |f| {
                optional(f, "--profile", profile.as_deref())?;
                optional(f, "--base", base.as_deref())?;
                flag(f, " --force", self.force)
            }),
            Op::Reply { id, text } => plain(f, "reply", &[id, text]),
            Op::Stop { id } => plain(f, "stop", &[id]),
            Op::Retry {
                id,
                profile,
                fresh,
                note,
            } => invocation(f, "retry", &[id], |f| {
                optional(f, "--profile", profile.as_deref())?;
                flag(f, " --fresh", *fresh)?;
                optional(f, "--note", note.as_deref())
            }),
            Op::Done { id, summary } => {
                invocation(f, "done", &[id], |f| option(f, "--summary", summary))
            }
            Op::Gate { id } => plain(f, "gate", &[id]),
            Op::Review { id, profile } => invocation(f, "review", &[id], |f| {
                optional(f, "--profile", profile.as_deref())
            }),
            Op::Verdict { id, approve, notes } => invocation(
                f,
                "verdict",
                &[id, if *approve { "approve" } else { "changes" }],
                |f| optional(f, "--notes", notes.as_deref()),
            ),
            Op::Changes { id, text } => plain(f, "changes", &[id, text]),
            Op::Approve { id } => {
                invocation(f, "approve", &[id], |f| flag(f, " --force", self.force))
            }
            Op::Drop { id } => plain(f, "drop", &[id]),
            Op::Reopen { id } => plain(f, "reopen", &[id]),
            Op::ArtifactAdd {
                id,
                path,
                kind,
                title,
            } => invocation(f, "artifact add", &[id, &path.to_string_lossy()], |f| {
                optional(f, "--kind", kind.as_deref())?;
                optional(f, "--title", title.as_deref())
            }),
            Op::UsageAdd { usage } => {
                f.write_str("usage add")?;
                option(f, "--model", &usage.model)?;
                optional(f, "--task", usage.task.as_deref())?;
                optional(f, "--run", usage.run.as_deref())?;
                for (name, count) in [
                    ("--input", usage.input),
                    ("--output", usage.output),
                    ("--cache-read", usage.cache_read),
                    ("--cache-write", usage.cache_write),
                ] {
                    if count != 0 {
                        write!(f, " {name} {count}")?;
                    }
                }
                if let Some(usd) = usage.usd {
                    let separator = if usd.is_sign_negative() { '=' } else { ' ' };
                    write!(f, " --usd{separator}{usd}")?;
                }
                optional(f, "--billing", usage.billing.as_deref())
            }
            Op::Sync => f.write_str("sync"),
        }
    }
}

fn plain(f: &mut fmt::Formatter<'_>, verb: &str, operands: &[&str]) -> fmt::Result {
    invocation(f, verb, operands, |_| Ok(()))
}

fn invocation(
    f: &mut fmt::Formatter<'_>,
    verb: &str,
    operands: &[&str],
    options: impl FnOnce(&mut fmt::Formatter<'_>) -> fmt::Result,
) -> fmt::Result {
    f.write_str(verb)?;
    if operands.iter().any(|value| value.starts_with('-')) {
        options(f)?;
        f.write_str(" --")?;
        positionals(f, operands)
    } else {
        positionals(f, operands)?;
        options(f)
    }
}

fn positionals(f: &mut fmt::Formatter<'_>, values: &[&str]) -> fmt::Result {
    for value in values {
        f.write_str(" ")?;
        shell_word(f, value)?;
    }
    Ok(())
}

fn optional(f: &mut fmt::Formatter<'_>, name: &str, value: Option<&str>) -> fmt::Result {
    match value {
        Some(value) => option(f, name, value),
        None => Ok(()),
    }
}

fn option(f: &mut fmt::Formatter<'_>, name: &str, value: &str) -> fmt::Result {
    let separator = if value.starts_with('-') { '=' } else { ' ' };
    write!(f, " {name}{separator}")?;
    shell_word(f, value)
}

fn flag(f: &mut fmt::Formatter<'_>, flag: &str, enabled: bool) -> fmt::Result {
    if enabled { f.write_str(flag) } else { Ok(()) }
}

fn shell_word(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'%' | b'+' | b'=' | b','
                )
        })
    {
        return f.write_str(value);
    }
    f.write_str("'")?;
    for (index, piece) in value.split('\'').enumerate() {
        if index != 0 {
            f.write_str("'\\''")?;
        }
        f.write_str(piece)?;
    }
    f.write_str("'")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refusal {
    pub reason: String,
    pub next: Vec<Next>,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)?;
        for next in &self.next {
            write!(f, "\nnext: {next}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Refusal {}

#[cfg(test)]
#[path = "op_tests.rs"]
mod tests;
