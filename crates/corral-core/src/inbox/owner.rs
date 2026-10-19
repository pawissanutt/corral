use std::fs;

use anyhow::{Context, Result, ensure};
use serde_json::json;

use super::storage::{paths, read_optional, write_json};
use super::*;
use crate::Store;

#[derive(Debug, Clone, Copy)]
pub enum Recovery {
    Resend,
    Abandon,
    Submit,
    Discard,
}

impl Inbox<'_> {
    /// Phase 1 launches are always legacy; preserve any historical inbox records.
    pub fn record_pm_launch(&self) -> Result<()> {
        ensure!(
            self.recipient == Recipient::Pm,
            "PM launch state requires PM recipient"
        );
        let _deliver = self.delivery_lock()?;
        let _inbox = self.lock()?;
        let mut pm = self.pm_state()?;
        pm.delivery = DeliveryMode::Legacy;
        fs::create_dir_all(self.store.state_dir()).context("creating PM inbox state directory")?;
        write_json(&self.store.state_dir().join("pm-inbox.json"), &pm)
    }
    pub fn keys(store: &Store) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for path in paths(&store.dir().join("inbox"))? {
            if path.is_dir()
                && let Some(key) = path.file_name().and_then(|n| n.to_str())
            {
                Recipient::parse_key(key)?;
                keys.push(key.into());
            }
        }
        keys.sort();
        Ok(keys)
    }

    pub fn request_flush(&self) -> Result<()> {
        let _deliver = self.delivery_lock()?;
        let _inbox = self.lock()?;
        fs::create_dir_all(self.dir()).context("creating inbox for flush")?;
        write_json(
            &self.dir().join("flush.json"),
            &json!({"at":Timestamp::now(),"via":"manual"}),
        )?;
        self.event("inbox_flush_requested", json!({"via":"manual"}))
    }

    pub fn flush_requested(&self) -> bool {
        self.dir().join("flush.json").is_file()
    }

    pub fn consume_flush(&self) -> Result<()> {
        match fs::remove_file(self.dir().join("flush.json")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).context("consuming inbox flush request"),
        }
    }

    pub fn retry(&self, seq: u64) -> Result<Message> {
        let _deliver = self.delivery_lock()?;
        ensure!(
            self.terminal(seq)? == Some(TerminalState::Failed),
            "retry requires a failed message"
        );
        let original = self.message(seq)?;
        let (_, body) = original
            .body
            .split_once('\n')
            .context("message body header missing")?;
        self.post(Post {
            kind: &original.kind,
            urgency: original.urgency,
            author: &original.author,
            body,
            binding: original.binding.clone(),
            subject: original.subject.clone(),
            max_payload: MAX_PAYLOAD,
        })
    }

    pub fn mark_observed(&self, seq: u64, reason: &str) -> Result<bool> {
        ensure!(
            !reason.trim().is_empty(),
            "manual observation requires a reason"
        );
        let _deliver = self.delivery_lock()?;
        let envelope = self
            .envelopes()?
            .into_iter()
            .find(|e| e.members.contains(&seq));
        self.record_receipt(
            seq,
            ReceiptLevel::Observed,
            &Receipt {
                envelope: envelope.as_ref().map(|e| e.number),
                binding: envelope.map(|e| e.binding),
                observed_at: Timestamp::now(),
                via: "manual".into(),
                detail: json!({"reason":reason}),
            },
        )
    }

    pub fn recover(&self, recovery: Recovery) -> Result<()> {
        let _deliver = self.delivery_lock()?;
        self.recover_publication()?;
        let state = self.state()?;
        let number = state
            .open_envelope
            .context("recipient has no open envelope")?;
        let envelope = self.envelope(number)?;
        match recovery {
            Recovery::Submit | Recovery::Discard => {
                // Native adapters and terminal composer actions ship in later phases.
                anyhow::bail!(
                    "terminal draft recovery requires an enabled terminal adapter; none ships in this phase"
                );
            }
            Recovery::Abandon => {
                for seq in &envelope.members {
                    self.mark_terminal(
                        *seq,
                        TerminalState::Abandoned,
                        "owner abandoned delivery",
                        "manual",
                    )?;
                }
            }
            Recovery::Resend => {
                ensure!(
                    !envelope.attempts.is_empty(),
                    "unattempted envelope does not need recovery"
                );
                ensure!(!self.settled(&envelope)?, "envelope is already settled");
            }
        }
        let _inbox = self.lock()?;
        if matches!(recovery, Recovery::Resend) {
            // Owner resend is a wake even when the original envelope contained only info.
            // Persist eligibility before retiring the old envelope so a crash cannot lose it.
            write_json(
                &self.dir().join("flush.json"),
                &json!({"at":Timestamp::now(),"via":"manual","resend_envelope":number}),
            )?;
        }
        // Keep original attempt and uncertainty evidence. A resend is a NEW frozen envelope.
        write_json(
            &self
                .dir()
                .join("envelopes")
                .join(format!("{number:06}.settled.json")),
            &json!({"via":"manual","action":match recovery {
                Recovery::Resend => "resend", Recovery::Abandon => "abandon",
                Recovery::Submit => "submit", Recovery::Discard => "discard",
            },"at":Timestamp::now()}),
        )?;
        self.update_state(|s| {
            s.open_envelope = None;
            Ok(())
        })?;
        self.event("inbox_recovered", json!({"envelope":number,"via":"manual","duplicate_possible":matches!(recovery,Recovery::Resend)}))
    }

    pub(super) fn owner_settled(&self, number: u64) -> Result<bool> {
        Ok(read_optional::<Value>(
            &self
                .dir()
                .join("envelopes")
                .join(format!("{number:06}.settled.json")),
        )?
        .is_some())
    }
}
