//! `inbox` command executors.
use anyhow::{Context, Result, ensure};
use corral_core::Store;
use corral_core::inbox::{Inbox, Recipient, Recovery, TerminalState};
use corral_protocol::cmd::inbox::InboxCommand;

use super::{Exec, Invoker, Verb};

impl Verb for InboxCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        let store = &cx.ctx.store;
        match self {
            Self::IndexOwner { key, limit, json } => {
                ensure!(
                    caller(store, cx.invoker)? != Caller::Agent,
                    "index_refused: disallowed_context: ask the PM or the owner to index this inbox"
                );
                let inbox = Inbox::new(store, Recipient::parse_key(&key)?)?;
                let header = inbox.index_owner(usize::from(limit))?;
                if json {
                    cx.out.json_pretty(&header)?;
                } else {
                    cx.out.line(format_args!(
                        "{} retained {}/16; indexing {}",
                        inbox.key(),
                        header.len(),
                        if header.legacy_debt.is_some() {
                            "pending"
                        } else {
                            "complete"
                        }
                    ));
                    if let Some(debt) = header.legacy_debt {
                        cx.out.line(format_args!(
                            "{:?}: next {} through {}; run `corral inbox index-owner {} --limit 64` after capacity is available",
                            debt.phase,
                            debt.next_seq,
                            debt.end_seq,
                            inbox.key()
                        ));
                    }
                }
            }
            Self::Recover {
                key,
                resend,
                abandon,
                submit,
                discard: _,
            } => {
                let action = if resend {
                    Recovery::Resend
                } else if abandon {
                    Recovery::Abandon
                } else if submit {
                    Recovery::Submit
                } else {
                    Recovery::Discard
                };
                ensure!(
                    caller(store, cx.invoker)? == Caller::Owner,
                    "recover_refused: owner_only: only the owner may recover an open envelope, not a PM, worker or reviewer"
                );
                let inbox = Inbox::new(store, Recipient::parse_key(&key)?)?;
                inbox.recover(action)?;
                cx.out
                    .line(format_args!("{} recovery recorded", inbox.key()));
            }
            Self::Mark {
                id,
                observed,
                cancel: _,
                reason,
            } => {
                let (recipient, seq) = message_id(&id)?;
                ensure!(!reason.trim().is_empty(), "mark requires a reason");
                let inbox = Inbox::new(store, recipient)?;
                let changed = if observed {
                    inbox.mark_observed(seq, &reason)?
                } else {
                    let _deliver = inbox.delivery_lock()?;
                    inbox.mark_terminal(seq, TerminalState::Cancelled, &reason, "manual")?
                };
                cx.out.line(format_args!(
                    "{}#{} {}",
                    inbox.key(),
                    seq,
                    if changed { "marked" } else { "unchanged" }
                ));
            }
            Self::Flush { key } => {
                let inbox = Inbox::new(store, Recipient::parse_key(&key)?)?;
                inbox.request_flush()?;
                cx.out.line(format_args!("{} flush requested", inbox.key()));
            }
            Self::Retry { id } => {
                let (recipient, seq) = message_id(&id)?;
                ensure!(
                    caller(store, cx.invoker)? != Caller::Agent,
                    "retry_refused: disallowed_context: workers and reviewers cannot re-post inbox messages; ask the PM or the owner"
                );
                let message = crate::delivery::retry(store, &recipient, seq)?;
                cx.out.line(format_args!("{}", message.id));
            }
            Self::Other => return Err(super::unknown_command("other")),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    Owner,
    Pm,
    Agent,
}

fn caller(store: &Store, invoker: &Invoker) -> Result<Caller> {
    // An assistant shell is never the PM, even when it inherits CORRAL_PM.
    if invoker.caller.env.corral_assistant.is_some() {
        return Ok(Caller::Agent);
    }
    if invoker.pm_marker() {
        return Ok(Caller::Pm);
    }
    if invoker.agent_entry() {
        return Ok(Caller::Agent);
    }
    let cwd = invoker
        .cwd()
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

impl Invoker {
    /// Shares the inbox caller guard with local retry-evidence inspection.
    pub fn inbox_is_agent(&self, store: &Store) -> Result<bool> {
        Ok(caller(store, self)? == Caller::Agent)
    }
}

fn message_id(value: &str) -> Result<(Recipient, u64)> {
    let (key, seq) = value.split_once('#').context("expected inbox id key#seq")?;
    let recipient = Recipient::parse_key(key)?;
    ensure!(
        !seq.is_empty() && seq.bytes().all(|b| b.is_ascii_digit()),
        "inbox sequence must be a positive integer"
    );
    let seq = seq.parse::<u64>().context("inbox sequence out of range")?;
    ensure!(seq > 0, "inbox sequence must be positive");
    Ok((recipient, seq))
}
