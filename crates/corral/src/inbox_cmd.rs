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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    Owner,
    Pm,
    Agent,
}

fn caller(store: &Store) -> Result<Caller> {
    // An assistant shell is never the PM, even when it inherits CORRAL_PM.
    if std::env::var_os("CORRAL_ASSISTANT").is_some() {
        return Ok(Caller::Agent);
    }
    if std::env::var("CORRAL_PM").as_deref() == Ok("1") {
        return Ok(Caller::Pm);
    }
    if corral_ops::agent_entry() {
        return Ok(Caller::Agent);
    }
    let cwd = std::env::current_dir()
        .context("reading inbox command directory")?
        .canonicalize()
        .context("resolving inbox command directory")?;
    for task in store.list()? {
        for run in &task.runs {
            if run
                .worktree
                .canonicalize()
                .is_ok_and(|worktree| cwd.starts_with(worktree))
            {
                return Ok(Caller::Agent);
            }
        }
    }
    Ok(Caller::Owner)
}

fn status(entry: &InboxEntry) -> &'static str {
    if entry.observed.is_some() {
        "observed"
    } else if let Some(terminal) = entry.terminal {
        terminal.suffix()
    } else if matches!(
        entry.envelope.as_ref().and_then(|e| e.outcome.as_ref()),
        Some(SendOutcome::Uncertain { .. } | SendOutcome::Conflicted { .. })
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
            Some(SendOutcome::Uncertain { .. } | SendOutcome::Conflicted { .. }) => "uncertain",
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
            ensure!(
                caller(store)? != Caller::Agent,
                "retry_refused: disallowed_context: workers and reviewers cannot re-post inbox messages; ask the PM or the owner"
            );
            let message = corral_ops::delivery::retry(store, &id.recipient, id.seq)?;
            println!("{}", message.id);
        }
        Some(InboxCmd::InspectRetry { id }) => {
            ensure!(
                caller(store)? != Caller::Agent,
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
            ensure!(
                caller(store)? != Caller::Agent,
                "index_refused: disallowed_context: ask the PM or the owner to index this inbox"
            );
            let inbox = Inbox::new(store, key)?;
            let header = inbox.index_owner(usize::from(limit))?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&header)?);
            } else {
                println!(
                    "{} retained {}/16; indexing {}",
                    inbox.key(),
                    header.len(),
                    if header.legacy_debt.is_some() {
                        "pending"
                    } else {
                        "complete"
                    }
                );
                if let Some(debt) = header.legacy_debt {
                    println!(
                        "{:?}: next {} through {}; run `corral inbox index-owner {} --limit 64` after capacity is available",
                        debt.phase,
                        debt.next_seq,
                        debt.end_seq,
                        inbox.key()
                    );
                }
            }
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
            ensure!(
                caller(store)? == Caller::Owner,
                "recover_refused: owner_only: only the owner may recover an open envelope, not a PM, worker or reviewer"
            );
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
                let _deliver = inbox.delivery_lock()?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(caller(&store).unwrap(), Caller::Agent);
    }
}
