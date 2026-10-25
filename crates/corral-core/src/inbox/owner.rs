use std::fs;
use std::io::Write;

use anyhow::{Context, Result, bail, ensure};
use serde_json::json;

use super::storage::{paths, read_optional, write_json};
use super::*;
use crate::{Store, StoreLock, Task};

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

    /// Exact-ID retry with no container liveness guard (the PM inbox has none).
    pub fn retry(&self, seq: u64) -> Result<Message> {
        self.retry_with(seq, |_| Ok(()))
    }

    /// Re-posts a terminal message to this same recipient under the Design 4 eligibility
    /// matrix: never attempted (Failed, Cancelled or Abandoned), or Failed after exclusively
    /// definitive Rejected attempts whose latest is `retryable_after_fix`. The evidence is
    /// checked under the delivery lock, then again with `live` inside the reservation's task
    /// transaction under the posting lock, so evidence landing in between still refuses.
    /// A message is re-posted at most once: a `retried` record is claimed before the post and
    /// kept unless the post is definitively refused before reservation. Its replacement
    /// identity is recorded before reservation can commit, independently of announcements.
    pub fn retry_with(&self, seq: u64, live: impl FnOnce(&Task) -> Result<()>) -> Result<Message> {
        let _deliver = self.delivery_lock()?;
        let original = self.message(seq)?;
        let marker = self.record_path(seq, "retried");
        let already = || {
            anyhow::anyhow!(
                "retry_refused: already_retried: {} already has a retry claim (see {}); inspect it with `corral inbox show {}`",
                original.id,
                marker.display(),
                original.id
            )
        };
        if marker.exists() {
            return Err(already());
        }
        if let Some(refusal) = self.retry_refusal(&original, self.state()?.next_envelope)? {
            bail!("retry_refused: {refusal}");
        }
        let (_, body) = original
            .body
            .split_once('\n')
            .context("message body header missing")?;
        if !create_new_json(&marker, &json!({"at": Timestamp::now()}))? {
            return Err(already());
        }
        let mut reserving = false;
        let posted = self.post_with_reservation(
            Post {
                kind: &original.kind,
                urgency: original.urgency,
                author: &original.author,
                body,
                binding: original.binding.clone(),
                subject: original.subject.clone(),
                max_payload: MAX_PAYLOAD,
            },
            |task, _| {
                live(task)?;
                let next = self.recipient.state_mut(task)?.next_envelope;
                match self.retry_refusal(&original, next)? {
                    Some(refusal) => bail!("retry_refused: {refusal}"),
                    None => Ok(()),
                }
            },
            |replacement| {
                write_json(
                    &marker,
                    &json!({"at": Timestamp::now(), "from": original.id, "to": replacement.id}),
                )?;
                reserving = true;
                Ok(())
            },
        );
        let retried = match posted {
            Ok(retried) => retried,
            Err(error) if reserving => return Err(error),
            Err(error) => {
                match fs::remove_file(&marker) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(error.context(format!("releasing retry claim: {e}")));
                    }
                }
                return Err(error);
            }
        };
        self.event(
            "message_retried",
            json!({"from": original.id, "to": retried.id}),
        )?;
        Ok(retried)
    }

    /// The first reason the message's own evidence forbids a re-post, as `code: explanation`.
    /// Reads every envelope in `1..next_envelope`: a missing one makes membership unknown.
    fn retry_refusal(&self, message: &Message, next_envelope: u64) -> Result<Option<String>> {
        let (seq, id) = (message.seq, &message.id);
        let inspect =
            format!("inspect it with `corral inbox show {id}` and choose an owner disposition");
        if self.receipt(seq, ReceiptLevel::Observed)?.is_some() {
            return Ok(Some(format!(
                "observed: {id} already reached the agent; nothing to retry"
            )));
        }
        let Some(terminal) = self.terminal(seq)? else {
            return Ok(Some(format!(
                "not_terminal: {id} is still pending delivery; run `corral sync` and wait for its outcome"
            )));
        };
        if self.receipt(seq, ReceiptLevel::Admitted)?.is_some() {
            return Ok(Some(format!(
                "admitted: the transport admitted {id}, so it may have been delivered; {inspect}"
            )));
        }
        if message.binding.question_request.is_some() {
            return Ok(Some(format!(
                "alternate_route: {id} answers a native question outside envelopes, so its delivery is unknown; {inspect}"
            )));
        }
        let mut latest = None;
        for number in 1..next_envelope {
            let Some(envelope) = read_optional::<Envelope>(&self.envelope_path(number))? else {
                return Ok(Some(format!(
                    "evidence_incomplete: envelope env{number} is missing, so {id}'s delivery history is unknown; {inspect}"
                )));
            };
            if !envelope.members.contains(&seq) {
                continue;
            }
            if let Some(stage) = envelope
                .stages
                .iter()
                .find(|s| !matches!(s, Stage::Dispatching | Stage::Outcome | Stage::Blocked))
            {
                return Ok(Some(format!(
                    "submitted: env{number} recorded stage {stage:?} for {id}; {inspect}"
                )));
            }
            if let Some(outcome) = envelope
                .outcome
                .as_ref()
                .filter(|o| !matches!(o, SendOutcome::Rejected { .. }))
            {
                return Ok(Some(format!(
                    "uncertain_prior_attempt: env{number} ended {} for {id}; {inspect}",
                    outcome_label(Some(outcome))
                )));
            }
            for (index, attempt) in envelope.attempts.iter().enumerate() {
                match &attempt.outcome {
                    Some(SendOutcome::Rejected {
                        retryable_after_fix,
                        ..
                    }) => latest = Some(*retryable_after_fix),
                    other => {
                        return Ok(Some(format!(
                            "uncertain_prior_attempt: env{number} attempt {} was {} for {id}; {inspect}",
                            index + 1,
                            outcome_label(other.as_ref())
                        )));
                    }
                }
            }
        }
        Ok(match latest {
            None => None,
            Some(_) if terminal != TerminalState::Failed => Some(format!(
                "attempted_{}: {id} was {} after a delivery attempt; {inspect}",
                terminal.suffix(),
                terminal.suffix()
            )),
            Some(false) => Some(format!(
                "not_retryable: the transport rejected {id} as not retryable after a fix; {inspect}"
            )),
            Some(true) => None,
        })
    }

    /// [`Inbox::mark_terminal`] for a caller already holding this recipient's posting lock
    /// ([`Inbox::lock`]), which is not reentrant.
    pub fn mark_terminal_held(
        &self,
        _posting: &StoreLock,
        seq: u64,
        state: TerminalState,
        reason: &str,
        via: &str,
    ) -> Result<bool> {
        self.message(seq)?;
        if self.receipt(seq, ReceiptLevel::Observed)?.is_some() || self.terminal(seq)?.is_some() {
            self.event(
                "message_terminal_noop",
                json!({"seq":seq,"state":state,"via":via}),
            )?;
            return Ok(false);
        }
        if !create_new_json(
            &self.record_path(seq, state.suffix()),
            &json!({"reason":reason,"via":via,"at":Timestamp::now()}),
        )? {
            return Ok(false);
        }
        self.event(
            &format!("message_{}", state.suffix()),
            json!({"seq":seq,"reason":reason,"via":via}),
        )?;
        Ok(true)
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
        // Terminal composer actions ship in later phases: refuse before any repair or write.
        if matches!(recovery, Recovery::Submit | Recovery::Discard) {
            return Err(terminal_adapter_unavailable());
        }
        let _deliver = self.delivery_lock()?;
        self.recover_publication()?;
        let state = self.state()?;
        let number = state
            .open_envelope
            .context("recipient has no open envelope")?;
        let envelope = self.envelope(number)?;
        match recovery {
            Recovery::Submit | Recovery::Discard => return Err(terminal_adapter_unavailable()),
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

/// Exclusive, synced record creation; `false` when the record already exists.
fn create_new_json(path: &std::path::Path, value: &Value) -> Result<bool> {
    let bytes = serde_json::to_vec_pretty(value).context("serializing inbox record")?;
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
    };
    file.write_all(&bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    file.sync_all().context("syncing inbox record")?;
    Ok(true)
}

fn terminal_adapter_unavailable() -> anyhow::Error {
    anyhow::anyhow!(
        "terminal_adapter_unavailable: terminal draft recovery requires an enabled terminal adapter; none is installed, so use --resend or --abandon"
    )
}

fn outcome_label(outcome: Option<&SendOutcome>) -> &'static str {
    match outcome {
        None => "unresolved",
        Some(SendOutcome::Admitted { .. }) => "admitted",
        Some(SendOutcome::Accepted) => "accepted",
        Some(SendOutcome::Held { .. }) => "held",
        Some(SendOutcome::Rejected { .. }) => "rejected",
        Some(SendOutcome::Uncertain { .. }) => "uncertain",
        Some(SendOutcome::Conflicted { .. }) => "conflicted",
    }
}
