//! `corral question`: PM and owner operations on corral-pi agent questions, by key and revision.
//! `answer --owner` and `deadline` are absent from every Pi role loadout.
use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use corral_ops::questions::{self, QuestionView};
use corral_protocol::cmd::{Command, question::QuestionCommand};

#[derive(Args)]
pub struct QuestionArgs {
    #[command(subcommand)]
    pub(crate) cmd: QuestionCmd,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Kind {
    Clarification,
    TechnicalChoice,
    Permission,
    OwnerOnly,
}

#[derive(Subcommand)]
pub(crate) enum QuestionCmd {
    /// List corral-pi agent questions; pending ones (open or escalated) by default
    List {
        /// Only this task's questions
        #[arg(long)]
        task: Option<String>,
        /// Include answered, expired, withdrawn and cancelled questions
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one question with its effective state, triage, answer and deadline
    Show {
        /// Question key <task>/<run>[/<review-child>]/Q<n>
        key: String,
        #[arg(long)]
        json: bool,
    },
    /// Answer at the current revision; a PM answer needs --basis refs and --scope-note
    Answer {
        key: String,
        /// The question's current revision; a stale revision is refused
        #[arg(long)]
        rev: u32,
        #[arg(long)]
        text: String,
        /// One of the question's options
        #[arg(long)]
        option: Option<String>,
        /// Canonical basis ref (repeatable): spec:<task>@<revision>, plan:<task>@<plan>,
        /// decision:<task>@<plan>#<n>, decision:<task>/<run>/<id> or comment:<task>#<n>
        #[arg(long = "basis")]
        basis: Vec<String>,
        /// How the answer stays inside the approved write scope and acceptance
        #[arg(long)]
        scope_note: Option<String>,
        /// Answer as the owner: any pending question, or supersede an answer
        #[arg(long)]
        owner: bool,
        #[arg(long)]
        json: bool,
    },
    /// Answer several members of one question group (a batched ask) in one command
    AnswerGroup {
        /// The group key or any member's key
        key: String,
        /// JSON array of {"key", "rev", "text"?, "option"?}; omit to answer every pending
        /// member at its current revision with --text
        #[arg(long)]
        answers: Option<String>,
        /// Shared answer text for members without their own
        #[arg(long, default_value = "")]
        text: String,
        /// Canonical basis ref (repeatable), shared by every member
        #[arg(long = "basis")]
        basis: Vec<String>,
        #[arg(long)]
        scope_note: Option<String>,
        #[arg(long)]
        owner: bool,
        #[arg(long)]
        json: bool,
    },
    /// Escalate an open question to the owner, optionally reclassifying it upward
    Escalate {
        key: String,
        #[arg(long)]
        rev: u32,
        #[arg(long)]
        note: Option<String>,
        /// New kind; reclassification goes upward only
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        #[arg(long)]
        json: bool,
    },
    /// Cancel a pending question with a reason
    Cancel {
        key: String,
        #[arg(long)]
        rev: u32,
        #[arg(long)]
        reason: String,
        /// Cancel as the owner
        #[arg(long)]
        owner: bool,
        #[arg(long)]
        json: bool,
    },
    /// Owner only: expire the question at --at with --instruction unless its revision changes first
    Deadline {
        key: String,
        #[arg(long)]
        rev: u32,
        /// RFC 3339 timestamp
        #[arg(long)]
        at: String,
        /// Delivered to the agent as the owner_deadline answer
        #[arg(long)]
        instruction: String,
        #[arg(long)]
        json: bool,
    },
}

fn print_view(view: &QuestionView) {
    let q = &view.question;
    let closure = view
        .closure
        .map(|c| format!(" [{}]", c.as_str()))
        .unwrap_or_default();
    let group = q
        .group
        .as_deref()
        .map(|g| format!("  group {g}"))
        .unwrap_or_default();
    println!(
        "{}  rev {}  {:?}{closure}  {:?}{}{group}  {}",
        q.key,
        q.revision,
        view.effective_state,
        q.kind,
        if q.blocking { " blocking" } else { "" },
        q.text.lines().next().unwrap_or_default()
    );
}

pub fn run(inv: &crate::invocation::Invocation, args: QuestionArgs) -> Result<()> {
    match args.cmd {
        QuestionCmd::List { task, all, json } => {
            let ctx = inv.ctx()?;
            let views = questions::list(ctx, task.as_deref(), all)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&views)?);
            } else if views.is_empty() {
                println!("no questions");
            } else {
                views.iter().for_each(print_view);
            }
            Ok(())
        }
        QuestionCmd::Show { key, json } => {
            let ctx = inv.ctx()?;
            let view = questions::show(ctx, &key)?;
            // A grouped question shows every member in full, each with its own key and revision.
            let members = match view.question.group {
                Some(_) => questions::group(ctx, &key)?,
                None => Vec::new(),
            };
            if json {
                let mut value = serde_json::to_value(&view)?;
                if !members.is_empty() {
                    value["group_members"] = serde_json::to_value(&members)?;
                }
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                print_view(&view);
                println!("{}", serde_json::to_string_pretty(&view.question)?);
                for member in members
                    .iter()
                    .filter(|m| m.question.key != view.question.key)
                {
                    println!("group member:");
                    print_view(member);
                    println!("{}", serde_json::to_string_pretty(&member.question)?);
                }
            }
            Ok(())
        }
        QuestionCmd::Answer {
            key,
            rev,
            text,
            option,
            basis,
            scope_note,
            owner,
            json,
        } => inv.submit(Command::Question(QuestionCommand::Answer {
            key,
            rev,
            text,
            option,
            basis,
            scope_note,
            owner,
            json,
        })),
        QuestionCmd::AnswerGroup {
            key,
            answers,
            text,
            basis,
            scope_note,
            owner,
            json,
        } => inv.submit(Command::Question(QuestionCommand::AnswerGroup {
            key,
            answers,
            text,
            basis,
            scope_note,
            owner,
            json,
        })),
        QuestionCmd::Escalate {
            key,
            rev,
            note,
            kind,
            json,
        } => inv.submit(Command::Question(QuestionCommand::Escalate {
            key,
            rev,
            note,
            kind: kind.map(|kind| {
                match kind {
                    Kind::Clarification => "clarification",
                    Kind::TechnicalChoice => "technical_choice",
                    Kind::Permission => "permission",
                    Kind::OwnerOnly => "owner_only",
                }
                .to_owned()
            }),
            json,
        })),
        QuestionCmd::Cancel {
            key,
            rev,
            reason,
            owner,
            json,
        } => inv.submit(Command::Question(QuestionCommand::Cancel {
            key,
            rev,
            reason,
            owner,
            json,
        })),
        QuestionCmd::Deadline {
            key,
            rev,
            at,
            instruction,
            json,
        } => inv.submit(Command::Question(QuestionCommand::Deadline {
            key,
            rev,
            at,
            instruction,
            json,
        })),
    }
}
