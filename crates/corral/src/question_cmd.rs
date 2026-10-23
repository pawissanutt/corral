//! `corral question`: PM and owner operations on Pi-native agent questions, by key and revision.
//! `answer --owner` and `deadline` are owner-only (trust-based like other `--owner` verbs) and
//! absent from every Pi role loadout.
use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use corral_core::agent::QuestionKind;
use corral_ops::Ctx;
use corral_ops::questions::{self, Actor, AnswerInput, Applied, QuestionView};

#[derive(Args)]
pub struct QuestionArgs {
    #[command(subcommand)]
    cmd: QuestionCmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Kind {
    Clarification,
    TechnicalChoice,
    Permission,
    OwnerOnly,
}

impl From<Kind> for QuestionKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Clarification => QuestionKind::Clarification,
            Kind::TechnicalChoice => QuestionKind::TechnicalChoice,
            Kind::Permission => QuestionKind::Permission,
            Kind::OwnerOnly => QuestionKind::OwnerOnly,
        }
    }
}

#[derive(Subcommand)]
enum QuestionCmd {
    /// List Pi-native agent questions; pending ones (open or escalated) by default
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

fn actor(owner: bool) -> Actor<'static> {
    if owner { Actor::Owner } else { Actor::Pm(None) }
}

fn print_applied(applied: &Applied, json: bool) -> Result<()> {
    let Some(outcome) = applied.outcome() else {
        anyhow::bail!("the CLI never replays a host request");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(outcome)?);
    } else {
        let message = outcome
            .message_id
            .as_deref()
            .map(|id| format!("; message {id}"))
            .unwrap_or_default();
        println!(
            "{} {:?} (revision {}){message}; delivery {}",
            outcome.key, outcome.state, outcome.revision, outcome.delivery
        );
    }
    Ok(())
}

fn print_view(view: &QuestionView) {
    let q = &view.question;
    let closure = view
        .closure
        .map(|c| format!(" [{}]", c.as_str()))
        .unwrap_or_default();
    println!(
        "{}  rev {}  {:?}{closure}  {:?}{}  {}",
        q.key,
        q.revision,
        view.effective_state,
        q.kind,
        if q.blocking { " blocking" } else { "" },
        q.text.lines().next().unwrap_or_default()
    );
}

pub fn run(ctx: &Ctx, args: QuestionArgs) -> Result<()> {
    match args.cmd {
        QuestionCmd::List { task, all, json } => {
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
            let view = questions::show(ctx, &key)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&view)?);
            } else {
                print_view(&view);
                println!("{}", serde_json::to_string_pretty(&view.question)?);
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
        } => {
            let input = AnswerInput {
                text,
                option,
                basis_refs: basis,
                scope_note,
            };
            print_applied(
                &questions::answer(ctx, &key, rev, actor(owner), input)?,
                json,
            )
        }
        QuestionCmd::Escalate {
            key,
            rev,
            note,
            kind,
            json,
        } => {
            let kind = kind.map(QuestionKind::from);
            print_applied(
                &questions::escalate(ctx, &key, rev, actor(false), note, kind)?,
                json,
            )
        }
        QuestionCmd::Cancel {
            key,
            rev,
            reason,
            owner,
            json,
        } => print_applied(
            &questions::cancel(ctx, &key, rev, actor(owner), &reason)?,
            json,
        ),
        QuestionCmd::Deadline {
            key,
            rev,
            at,
            instruction,
            json,
        } => {
            let at: jiff::Timestamp = at
                .parse()
                .with_context(|| format!("--at {at:?} is not an RFC 3339 timestamp"))?;
            print_applied(
                &questions::deadline(ctx, &key, rev, at, &instruction)?,
                json,
            )
        }
    }
}
