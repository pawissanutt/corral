use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use corral_core::Store;
use corral_core::inbox::{Inbox, InboxEntry, Recipient, Recovery, SendOutcome, TerminalState};

#[derive(Clone, Debug)]
pub struct MessageId {
    recipient: Recipient,
    seq: u64,
}

impl FromStr for MessageId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (key, seq) = value.split_once('#').context("expected inbox id key#seq")?;
        let recipient = Recipient::parse_key(key)?;
        ensure!(
            !seq.is_empty() && seq.bytes().all(|b| b.is_ascii_digit()),
            "inbox sequence must be a positive integer"
        );
        let seq = seq.parse::<u64>().context("inbox sequence out of range")?;
        ensure!(seq > 0, "inbox sequence must be positive");
        Ok(Self { recipient, seq })
    }
}

fn parse_recipient(key: &str) -> Result<Recipient> {
    Recipient::parse_key(key)
}

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct InboxArgs {
    /// Restrict the list to one recipient key
    #[arg(long = "for", value_parser = parse_recipient)]
    recipient: Option<Recipient>,
    #[arg(long, global = true)]
    json: bool,
    /// Include observed and terminal messages
    #[arg(long)]
    all: bool,
    #[command(subcommand)]
    cmd: Option<InboxCmd>,
}

#[derive(Subcommand)]
enum InboxCmd {
    /// Show the complete message, receipts, and effect state
    Show { id: MessageId },
    /// Request that held information be flushed by the delivery service
    Flush {
        #[arg(value_parser = parse_recipient)]
        key: Recipient,
    },
    /// Re-post a failed message with a new id
    Retry { id: MessageId },
    /// Record an explicit recovery decision for an open envelope
    Recover(RecoverArgs),
    /// Record manual observation or cancellation
    Mark(MarkArgs),
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct RecoveryAction {
    #[arg(long)]
    resend: bool,
    #[arg(long)]
    abandon: bool,
    #[arg(long)]
    submit: bool,
    #[arg(long)]
    discard: bool,
}

#[derive(Args)]
struct RecoverArgs {
    #[arg(value_parser = parse_recipient)]
    key: Recipient,
    #[command(flatten)]
    action: RecoveryAction,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct MarkAction {
    #[arg(long)]
    observed: bool,
    #[arg(long)]
    cancel: bool,
}

#[derive(Args)]
struct MarkArgs {
    id: MessageId,
    #[command(flatten)]
    action: MarkAction,
    #[arg(long)]
    reason: String,
}

fn status(entry: &InboxEntry) -> &'static str {
    if entry.observed.is_some() {
        "observed"
    } else if let Some(terminal) = entry.terminal {
        terminal.suffix()
    } else if matches!(
        entry.envelope.as_ref().and_then(|e| e.outcome.as_ref()),
        Some(SendOutcome::Uncertain { .. })
    ) {
        "uncertain"
    } else if entry.admitted.is_some() {
        "admitted"
    } else {
        match entry.envelope.as_ref().and_then(|e| e.outcome.as_ref()) {
            Some(SendOutcome::Admitted { .. }) => "admitted",
            Some(SendOutcome::Accepted) => "accepted",
            Some(SendOutcome::Held { .. }) => "held",
            Some(SendOutcome::Rejected { .. }) => "failed",
            Some(SendOutcome::Uncertain { .. }) => "uncertain",
            None => "pending",
        }
    }
}

fn print_entry(entry: &InboxEntry) {
    println!(
        "{}\t{}\t{}\t{}",
        entry.message.id,
        status(entry),
        entry.message.kind,
        entry.message.created
    );
}

pub fn run(store: &Store, args: InboxArgs) -> Result<()> {
    match args.cmd {
        None => {
            let recipients = match args.recipient {
                Some(recipient) => vec![recipient],
                None => Inbox::keys(store)?
                    .iter()
                    .map(|key| Recipient::parse_key(key))
                    .collect::<Result<Vec<_>>>()?,
            };
            let mut entries = Vec::new();
            for recipient in recipients {
                let inbox = Inbox::new(store, recipient)?;
                for message in inbox.messages()? {
                    let entry = inbox.entry(message.seq)?;
                    if args.all || (entry.observed.is_none() && entry.terminal.is_none()) {
                        entries.push(entry);
                    }
                }
            }
            if args.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for entry in &entries {
                    print_entry(entry);
                }
            }
        }
        Some(InboxCmd::Show { id }) => {
            let entry = Inbox::new(store, id.recipient)?.entry(id.seq)?;
            if args.json {
                let mut value = serde_json::to_value(&entry)?;
                value["message"]["body"] = entry.message.body.into();
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                print_entry(&entry);
                println!("\n{}", entry.message.body);
                println!(
                    "\nReceipts and effect:\n{}",
                    serde_json::to_string_pretty(&entry)?
                );
            }
        }
        Some(InboxCmd::Flush { key }) => {
            let inbox = Inbox::new(store, key)?;
            inbox.request_flush()?;
            println!("{} flush requested", inbox.key());
        }
        Some(InboxCmd::Retry { id }) => {
            let message = Inbox::new(store, id.recipient)?.retry(id.seq)?;
            println!("{}", message.id);
        }
        Some(InboxCmd::Recover(recover)) => {
            let action = if recover.action.resend {
                Recovery::Resend
            } else if recover.action.abandon {
                Recovery::Abandon
            } else if recover.action.submit {
                Recovery::Submit
            } else {
                Recovery::Discard
            };
            let inbox = Inbox::new(store, recover.key)?;
            inbox.recover(action)?;
            println!("{} recovery recorded", inbox.key());
        }
        Some(InboxCmd::Mark(mark)) => {
            ensure!(!mark.reason.trim().is_empty(), "mark requires a reason");
            let inbox = Inbox::new(store, mark.id.recipient)?;
            let changed = if mark.action.observed {
                inbox.mark_observed(mark.id.seq, &mark.reason)?
            } else {
                inbox.mark_terminal(
                    mark.id.seq,
                    TerminalState::Cancelled,
                    &mark.reason,
                    "manual",
                )?
            };
            println!(
                "{}#{} {}",
                inbox.key(),
                mark.id.seq,
                if changed { "marked" } else { "unchanged" }
            );
        }
    }
    Ok(())
}
