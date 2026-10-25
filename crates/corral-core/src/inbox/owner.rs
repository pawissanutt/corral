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

/// A read-only evidence assessment, never a replay authorization or a stored certificate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RetryInspection {
    pub message_id: String,
    pub recipient: Recipient,
    pub evidence_complete: bool,
    #[serde(rename = "eligibility")]
    pub eligible: bool,
    pub reason_code: String,
    pub reason: String,
    pub witness: Option<RetryWitness>,
    pub next_action: String,
}

impl Inbox<'_> {
    /// Inspect only this recipient under delivery -> inbox -> task locks, with no writes.
    pub fn inspect_retry(&self, seq: u64) -> Result<RetryInspection> {
        let _deliver = self.delivery_lock()?;
        let _inbox = self.lock()?;
        match self.recipient.task() {
            Some(id) => self.store.read_locked(id, |task| {
                let mut snapshot = task.clone();
                let state = self.recipient.state_mut(&mut snapshot)?;
                let mut inspection = self.inspect_retry_held(seq, state)?;
                if self.recipient_ended(task) {
                    inspection.eligible = false;
                    inspection.reason_code = "recipient_ended".into();
                    inspection.reason =
                        "the exact recipient ended; owner disposition required".into();
                    inspection.next_action = format!("corral inbox show {}#{}", self.key(), seq);
                }
                Ok(inspection)
            }),
            None => self.inspect_retry_held(seq, &self.state()?),
        }
    }

    fn inspect_retry_held(&self, seq: u64, state: &InboxState) -> Result<RetryInspection> {
        ensure!(
            seq > 0 && seq < state.next_seq,
            "uncommitted inbox identity"
        );
        let id = format!("{}#{seq}", self.key());
        let known = state
            .owner_header
            .as_ref()
            .is_some_and(|header| header.iter().any(|entry| entry.seq == seq));
        let message = match self.message(seq) {
            Ok(message) => message,
            Err(error) if known => {
                return Ok(RetryInspection {
                    message_id: id.clone(),
                    recipient: self.recipient.clone(),
                    evidence_complete: false,
                    eligible: false,
                    reason_code: "evidence_incomplete".into(),
                    reason: format!("publication evidence: {error:#}"),
                    witness: None,
                    next_action: format!(
                        "Owner: repair committed publication or dispose recipient {}; no retry is authorized",
                        self.key()
                    ),
                });
            }
            Err(error) => return Err(error),
        };
        let mut witness = None;
        let mut refusal = match self.retry_refusal(&message, state, &mut witness) {
            Ok(refusal) => refusal,
            Err(error) => Some(format!("evidence_incomplete: {error:#}")),
        };
        if refusal.is_none() && self.record_path(seq, "retried").exists() {
            refusal = Some(format!(
                "already_retried: {id} already has a retry claim (see {}); publication may be incomplete",
                self.record_path(seq, "retried").display()
            ));
        }
        if refusal.is_none() {
            let mut next = state.clone();
            if let Err(error) = self.admit_owner(&mut next, &message.kind) {
                refusal = Some(error.to_string());
            }
        }
        let reason_code = refusal
            .as_ref()
            .map(|reason| reason.split(':').next().unwrap_or(reason))
            .unwrap_or("eligible")
            .to_string();
        let eligible = refusal.is_none();
        let next_action = match reason_code.as_str() {
            "eligible" => format!("corral inbox retry {id}"),
            "owner_header_index_pending" => {
                format!("corral inbox index-owner {} --limit 64", self.key())
            }
            "evidence_incomplete" => format!(
                "Owner: inspect and repair the exact recipient {} evidence; no retry is authorized",
                self.key()
            ),
            _ => format!("corral inbox show {id}"),
        };
        Ok(RetryInspection {
            message_id: id.clone(),
            recipient: self.recipient.clone(),
            evidence_complete: !matches!(
                reason_code.as_str(),
                "evidence_incomplete" | "alternate_route"
            ),
            eligible,
            reason_code,
            reason: refusal.unwrap_or_else(|| {
                "all evidence permits a new exact-recipient post after fixing the cause".into()
            }),
            witness,
            next_action,
        })
    }

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
        {
            let _inbox = self.lock()?;
            if let Some(refusal) = self.retry_refusal(&original, &self.state()?, &mut None)? {
                bail!("retry_refused: {refusal}");
            }
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
                ensure!(
                    !self.recipient_ended(task),
                    "retry_refused: recipient_ended: inspect it with `corral inbox show {}`",
                    original.id
                );
                let state = self.recipient.state_mut(task)?;
                match self.retry_refusal(&original, state, &mut None)? {
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
    fn retry_refusal(
        &self,
        message: &Message,
        state: &InboxState,
        witness: &mut Option<RetryWitness>,
    ) -> Result<Option<String>> {
        let (seq, id) = (message.seq, &message.id);
        let inspect =
            format!("inspect it with `corral inbox show {id}` and choose an owner disposition");
        if self.receipt(seq, ReceiptLevel::Observed)?.is_some() {
            return Ok(Some(format!(
                "observed: {id} already reached the agent; nothing to retry"
            )));
        }
        let terminal = self.terminal(seq)?;
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
        ensure!(seq < state.next_seq, "uncommitted inbox identity");
        let retained = state
            .owner_header
            .as_ref()
            .and_then(|header| header.iter().find(|entry| entry.seq == seq));
        let mut pending = retained.and_then(|entry| entry.retry_evidence.pending);
        let mut latest = None;
        for number in 1..state.next_envelope {
            crate::work::record(crate::work::Work::InboxEnvelopeRead, 1);
            let envelope = match read_optional::<Envelope>(&self.envelope_path(number)) {
                Ok(Some(envelope)) if envelope.number == number => envelope,
                _ => {
                    return Ok(Some(format!(
                        "evidence_incomplete: envelope env{number} is missing or corrupt, so {id}'s delivery history is unknown; {inspect}"
                    )));
                }
            };
            if !envelope.members.contains(&seq) {
                continue;
            }
            if envelope.idempotency_key != format!("{}@env{number}", self.key())
                || envelope
                    .members
                    .iter()
                    .any(|seq| *seq == 0 || *seq >= state.next_seq)
                || self.payload(&envelope).is_err()
                || envelope
                    .attempts
                    .last()
                    .is_some_and(|attempt| attempt.outcome != envelope.outcome)
            {
                return Ok(Some(format!(
                    "evidence_incomplete: env{number} has inconsistent publication evidence for {id}; {inspect}"
                )));
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
            if envelope.attempts.is_empty()
                && (envelope.outcome.is_some()
                    || envelope
                        .stages
                        .iter()
                        .any(|stage| matches!(stage, Stage::Dispatching | Stage::Outcome)))
            {
                return Ok(Some(format!(
                    "evidence_incomplete: env{number} has submission evidence without an attempt for {id}; {inspect}"
                )));
            }
            for (index, attempt) in envelope.attempts.iter().enumerate() {
                let reference = AttemptRef {
                    envelope: number,
                    ordinal: u64::try_from(index)?
                        .checked_add(1)
                        .context("attempt ordinal exhausted")?,
                };
                match &attempt.outcome {
                    Some(SendOutcome::Rejected {
                        retryable_after_fix,
                        ..
                    }) => {
                        latest = Some(*retryable_after_fix);
                        if pending == Some(reference) {
                            pending = None;
                        }
                    }
                    other => {
                        *witness = Some(RetryWitness {
                            reason: outcome_witness(other.as_ref())
                                .unwrap_or(WitnessReason::UnresolvedPrior),
                            attempt: Some(reference),
                        });
                        return Ok(Some(format!(
                            "uncertain_prior_attempt: env{number} attempt {} was {} for {id}; {inspect}",
                            index + 1,
                            outcome_label(other.as_ref())
                        )));
                    }
                }
            }
            if let Some(reason) = outcome_witness(envelope.outcome.as_ref()) {
                *witness = Some(RetryWitness {
                    reason,
                    attempt: None,
                });
                return Ok(Some(format!(
                    "uncertain_prior_attempt: env{number} ended {} for {id}; {inspect}",
                    outcome_label(envelope.outcome.as_ref())
                )));
            }
        }
        if let Some(entry) = retained {
            if let RetryHistory::Barred { first } = &entry.retry_evidence.history {
                *witness = Some(first.clone());
                return Ok(Some(format!(
                    "retained_bar: {id} retains {:?}; {inspect}",
                    first.reason
                )));
            }
            if entry.retry_evidence.route == RetryRoute::AlternateUnknown {
                return Ok(Some(format!(
                    "alternate_route: {id} has unknown alternate delivery; {inspect}"
                )));
            }
        }
        if let Some(attempt) = pending {
            *witness = Some(RetryWitness {
                reason: WitnessReason::UnresolvedPrior,
                attempt: Some(attempt),
            });
            return Ok(Some(format!(
                "evidence_incomplete: {id} retains an unresolved attempt fence; {inspect}"
            )));
        }
        let Some(terminal) = terminal else {
            return Ok(Some(format!(
                "not_terminal: {id} is still pending delivery; run `corral sync` and wait for its outcome"
            )));
        };
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

fn outcome_witness(outcome: Option<&SendOutcome>) -> Option<WitnessReason> {
    match outcome {
        None | Some(SendOutcome::Rejected { .. }) => None,
        Some(SendOutcome::Accepted) => Some(WitnessReason::Accepted),
        Some(SendOutcome::Admitted { .. }) => Some(WitnessReason::Admitted),
        Some(SendOutcome::Held { .. }) => Some(WitnessReason::Held),
        Some(SendOutcome::Uncertain { .. }) => Some(WitnessReason::Uncertain),
        Some(SendOutcome::Conflicted { .. }) => Some(WitnessReason::Conflicted),
    }
}
