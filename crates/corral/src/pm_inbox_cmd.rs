//! `corral pm inbox`: durable requests from assistants and the owner to the PM, each with a
//! recorded outcome (T-544 pm-inbox). Posting and withdrawing act as the caller; claiming and
//! resolving are the PM's.
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use corral_core::pm_inbox::{self, ItemState, PmInboxItem, name};
use corral_protocol::cmd::{Command, pm::PmCommand};

#[derive(Subcommand)]
pub enum PmInboxCmd {
    /// File a request for the PM: an action request, question, handoff or routed owner request
    Post {
        /// action_request, question, handoff or routing
        #[arg(long)]
        kind: String,
        /// One line naming the request
        #[arg(long)]
        subject: String,
        /// The author's reading and context (Markdown, at most 32 KiB)
        #[arg(long)]
        body_file: Option<PathBuf>,
        /// A task ID or artifact ref the item concerns; repeatable
        #[arg(long = "ref")]
        refs: Vec<String>,
        /// The corral command the item asks for, such as "approve T-5"
        #[arg(long)]
        action: Option<String>,
        /// The owner's verbatim words behind the request (evidence of intent, not authority)
        #[arg(long)]
        quote: Option<String>,
        /// The owner input the quote comes from
        #[arg(long, requires = "quote")]
        input: Option<String>,
        /// Retry key: posting again with the same key returns the earlier item
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List open and claimed items, oldest first
    List {
        /// Only items in this state: open, claimed, resolved or withdrawn
        #[arg(long)]
        state: Option<String>,
        /// Include resolved and withdrawn items
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one item with its body, history and outcome
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// The PM claims an open item at its current revision
    Claim {
        id: String,
        #[arg(long)]
        expected_revision: u32,
        #[arg(long)]
        json: bool,
    },
    /// The PM resolves its claimed item; an assistant author receives the outcome once
    Resolve {
        id: String,
        #[arg(long)]
        expected_revision: u32,
        /// done, declined or escalated_to_owner
        #[arg(long)]
        outcome: String,
        /// The outcome the author reads (at most 4 KiB)
        #[arg(long)]
        text: String,
        /// A task ID or artifact ref the outcome produced; repeatable
        #[arg(long = "ref")]
        refs: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// The author or the owner withdraws an open or claimed item
    Withdraw {
        id: String,
        #[arg(long)]
        expected_revision: u32,
        #[arg(long, default_value = "")]
        reason: String,
        #[arg(long)]
        json: bool,
    },
}

impl PmInboxCmd {
    /// Posting and withdrawing name their caller (owner or assistant) as the author.
    pub fn acts_as_caller(&self) -> bool {
        matches!(self, Self::Post { .. } | Self::Withdraw { .. })
    }
}

fn print(item: &PmInboxItem, json: bool, verb: &str) -> Result<()> {
    if json {
        let mut value = serde_json::to_value(item)?;
        value["body"] = item.body.clone().into();
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{} {verb} (revision {})", item.id, item.revision);
    }
    Ok(())
}

fn show(item: &PmInboxItem) {
    println!(
        "{} [{}] revision {} from {} ({}): {}",
        item.id,
        name(&item.state),
        item.revision,
        item.from,
        name(&item.kind),
        item.subject
    );
    println!("Posted: {}", item.created);
    if !item.refs.is_empty() {
        println!("Refs: {}", item.refs.join(", "));
    }
    if let Some(action) = &item.requested_action {
        println!("Requested: {}", action.command());
    }
    if let Some(words) = &item.owner_words {
        println!("Owner words: {}", words.quote);
    }
    for step in &item.history {
        println!(
            "Revision {}: {} by {} at {}",
            step.revision,
            name(&step.state),
            step.by,
            step.at
        );
    }
    if let Some(outcome) = &item.outcome {
        println!(
            "Outcome: {} by {}: {}",
            name(&outcome.resolution),
            outcome.by,
            outcome.text
        );
        if !outcome.refs.is_empty() {
            println!("Outcome refs: {}", outcome.refs.join(", "));
        }
    }
    if let Some(withdrawn) = &item.withdrawn {
        println!("Withdrawn by {}: {}", withdrawn.by, withdrawn.reason);
    }
    if !item.body.trim().is_empty() {
        println!("\n{}", item.body.trim_end());
    }
}

pub fn run(inv: &crate::invocation::Invocation, cmd: PmInboxCmd) -> Result<()> {
    match cmd {
        PmInboxCmd::Post {
            kind,
            subject,
            body_file,
            refs,
            action,
            quote,
            input,
            key,
            json,
        } => {
            let body = body_file
                .map(|path| {
                    std::fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))
                })
                .transpose()?
                .unwrap_or_default();
            inv.submit(Command::Pm(PmCommand::InboxPost {
                kind,
                subject,
                body,
                refs,
                action,
                quote,
                input,
                key,
                json,
            }))?;
        }
        PmInboxCmd::List { state, all, json } => {
            let ctx = inv.ctx()?;
            let store = &ctx.store;
            let state = state
                .map(|s| {
                    pm_inbox::parse::<ItemState>(
                        "--state",
                        &s,
                        "open, claimed, resolved, withdrawn",
                    )
                })
                .transpose()?;
            let items: Vec<_> = store
                .pm_inbox_items()?
                .into_iter()
                .filter(|item| state.map_or(all || item.pending(), |s| item.state == s))
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else if items.is_empty() {
                println!("No PM inbox items.");
            }
            for item in items.iter().filter(|_| !json) {
                println!(
                    "{}  {}  r{}  {}  {}  {}",
                    item.id,
                    name(&item.state),
                    item.revision,
                    item.from,
                    name(&item.kind),
                    item.subject
                );
            }
        }
        PmInboxCmd::Show { id, json } => {
            let ctx = inv.ctx()?;
            let store = &ctx.store;
            let item = store.pm_inbox_item(&id)?;
            if json {
                print(&item, true, "")?;
            } else {
                show(&item);
            }
        }
        PmInboxCmd::Claim {
            id,
            expected_revision,
            json,
        } => inv.submit(Command::Pm(PmCommand::InboxClaim {
            id,
            expected_revision,
            json,
        }))?,
        PmInboxCmd::Resolve {
            id,
            expected_revision,
            outcome,
            text,
            refs,
            json,
        } => {
            inv.submit(Command::Pm(PmCommand::InboxResolve {
                id,
                expected_revision,
                outcome,
                text,
                refs,
                json,
            }))?;
        }
        PmInboxCmd::Withdraw {
            id,
            expected_revision,
            reason,
            json,
        } => {
            inv.submit(Command::Pm(PmCommand::InboxWithdraw {
                id,
                expected_revision,
                reason,
                json,
            }))?;
        }
    }
    Ok(())
}
