use std::borrow::Cow;
use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use corral_core::inbox::{Inbox, InboxEntry, Recipient, SendOutcome};
use corral_protocol::cmd::Command;
use corral_protocol::cmd::inbox::InboxCommand;

use crate::invocation::Invocation;

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
    pub(crate) cmd: Option<InboxCmd>,
}

#[derive(Subcommand)]
pub(crate) enum InboxCmd {
    /// Show the complete message, receipts, and effect state
    Show { id: MessageId },
    /// Request that held information be flushed by the delivery service
    Flush {
        #[arg(value_parser = parse_recipient)]
        key: Recipient,
    },
    /// Re-post a terminal message whose evidence proves it never arrived, with a new id
    Retry { id: MessageId },
    /// Inspect exact-recipient retry evidence without posting or certifying a later retry
    InspectRetry { id: MessageId },
    /// Advance bounded legacy owner identity indexing (not retry-history certification)
    IndexOwner {
        #[arg(value_parser = parse_recipient)]
        key: Recipient,
        #[arg(long, default_value_t = 64, value_parser = clap::value_parser!(u8).range(1..=64))]
        limit: u8,
    },
    /// Record the owner's recovery decision for an open envelope
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
pub(crate) struct RecoverArgs {
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
pub(crate) struct MarkArgs {
    id: MessageId,
    #[command(flatten)]
    action: MarkAction,
    #[arg(long)]
    reason: String,
}

fn status(entry: &InboxEntry) -> Cow<'static, str> {
    if entry.observed.is_some() {
        "observed".into()
    } else if let Some(terminal) = entry.terminal {
        terminal.suffix().into()
    } else if let Some(hold) = &entry.hold {
        let generation = hold
            .generation
            .map(|n| format!(", generation {n}"))
            .unwrap_or_default();
        format!(
            "held: PM host refused input ({}{generation}); awaiting PM replacement",
            hold.code
        )
        .into()
    } else if matches!(
        entry.envelope.as_ref().and_then(|e| e.outcome.as_ref()),
        Some(SendOutcome::Uncertain { .. } | SendOutcome::Conflicted { .. })
    ) {
        "uncertain".into()
    } else if entry.admitted.is_some() {
        "admitted".into()
    } else {
        match entry.envelope.as_ref().and_then(|e| e.outcome.as_ref()) {
            Some(SendOutcome::Admitted { .. }) => "admitted",
            Some(SendOutcome::Accepted) => "accepted",
            Some(SendOutcome::Held { .. }) => "held",
            Some(SendOutcome::Rejected { .. }) => "failed",
            Some(SendOutcome::Uncertain { .. } | SendOutcome::Conflicted { .. }) => "uncertain",
            None => "pending",
        }
        .into()
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

pub fn run(inv: &Invocation, args: InboxArgs) -> Result<()> {
    match args.cmd {
        None => {
            let store = &inv.ctx()?.store;
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
                let messages = if args.all {
                    inbox.messages()?
                } else {
                    inbox.pending_messages()?
                };
                for message in messages {
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
            let store = &inv.ctx()?.store;
            let entry = Inbox::new(store, id.recipient)?.entry(id.seq)?;
            if args.json {
                let mut value = serde_json::to_value(&entry)?;
                value["message"]["body"] = entry.message.body.into();
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                print_entry(&entry);
                if let Some(hold) = entry.hold.as_ref().filter(|hold| hold.supervised) {
                    println!(
                        "Next: corrald runs `{}` (pm.supervise = true)",
                        hold.release
                    );
                } else if let Some(hold) = &entry.hold {
                    println!("Next: `{}` (pm.supervise = false)", hold.release);
                }
                println!("\n{}", entry.message.body);
                println!(
                    "\nReceipts and effect:\n{}",
                    serde_json::to_string_pretty(&entry)?
                );
            }
        }
        Some(InboxCmd::Flush { key }) => {
            return inv.submit(Command::Inbox(InboxCommand::Flush { key: key.key()? }));
        }
        Some(InboxCmd::Retry { id }) => {
            return inv.submit(Command::Inbox(InboxCommand::Retry {
                id: format!("{}#{}", id.recipient.key()?, id.seq),
            }));
        }
        Some(InboxCmd::InspectRetry { id }) => {
            let store = &inv.ctx()?.store;
            ensure!(
                !inv.entry().invoker().inbox_is_agent(store)?,
                "inspect_refused: disallowed_context: ask the PM or the owner to inspect retry evidence"
            );
            let inspection = Inbox::new(store, id.recipient)?.inspect_retry(id.seq)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&inspection)?);
            } else {
                println!(
                    "{}\t{}\n{}\nNext: {}",
                    inspection.message_id,
                    inspection.reason_code,
                    inspection.reason,
                    inspection.next_action
                );
            }
        }
        Some(InboxCmd::IndexOwner { key, limit }) => {
            return inv.submit(Command::Inbox(InboxCommand::IndexOwner {
                key: key.key()?,
                limit,
                json: args.json,
            }));
        }
        Some(InboxCmd::Recover(recover)) => {
            return inv.submit(Command::Inbox(InboxCommand::Recover {
                key: recover.key.key()?,
                resend: recover.action.resend,
                abandon: recover.action.abandon,
                submit: recover.action.submit,
                discard: recover.action.discard,
            }));
        }
        Some(InboxCmd::Mark(mark)) => {
            return inv.submit(Command::Inbox(InboxCommand::Mark {
                id: format!("{}#{}", mark.id.recipient.key()?, mark.id.seq),
                observed: mark.action.observed,
                cancel: mark.action.cancel,
                reason: mark.reason,
            }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use corral_core::Store;
    use corral_ops::command::Invoker;

    #[test]
    fn assistant_shell_is_an_agent_caller_even_with_the_pm_marker() {
        // Given a child with CORRAL_ASSISTANT and an inherited CORRAL_PM, but no agent marker.
        let root = tempfile::tempdir().unwrap();
        Store::init(root.path()).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "inbox_cmd::tests::assistant_caller_subprocess",
                "--nocapture",
            ])
            .env("INBOX_CALLER_ROOT", root.path())
            .env("CORRAL_ASSISTANT", "A-1")
            .env("CORRAL_PM", "1")
            .env_remove("AI_AGENT")
            .env_remove("PI_CODING_AGENT")
            .current_dir(root.path());
        // When the inbox owner guard classifies that caller.
        let output = command.stdin(std::process::Stdio::null()).output().unwrap();
        // Then the child ran exactly that test and saw an agent, not the PM or the owner.
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn assistant_caller_subprocess() {
        let Ok(root) = std::env::var("INBOX_CALLER_ROOT") else {
            return;
        };
        let store = Store::open(std::path::Path::new(&root)).unwrap();
        let invoker = Invoker::from_process(
            corral_protocol::cmd::ClientKind::Cli,
            None,
            corral_protocol::cmd::Via::Cli,
            None,
            false,
            None,
        );
        assert!(invoker.inbox_is_agent(&store).unwrap());
    }
}
