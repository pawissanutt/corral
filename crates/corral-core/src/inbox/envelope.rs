use anyhow::{Context, Result, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::*;

impl Inbox<'_> {
    pub fn has_pending_delivery(&self, flush: bool) -> Result<bool> {
        for message in self.pending_messages()? {
            if (flush || message.urgency == Urgency::Wake)
                && message.binding.question_request.is_none()
                && self.receipt(message.seq, ReceiptLevel::Observed)?.is_none()
                && self.terminal(message.seq)?.is_none()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn retire_unattempted(&self, envelope: &Envelope) -> Result<()> {
        ensure!(
            envelope.attempts.is_empty(),
            "attempted envelope cannot be retired"
        );
        let _lock = self.lock()?;
        self.update_state(|s| {
            // Publication already authorized delivery, including an info-only owner flush.
            // Preserve that eligibility before retirement so interrupted rebinding can resume.
            self.save_record(
                "control",
                "flush",
                &json!({"at":Timestamp::now(),"via":"replacement","envelope":envelope.number}),
            )?;
            // Persist the adapter's source before any write can hide the old publication.
            s.replacement_envelope = Some(envelope.number);
            self.save_record(
                "settled",
                &format!("{:020}", envelope.number),
                &json!({"reason":"unattempted publication replacement"}),
            )?;
            s.open_envelope = None;
            s.binding = None;
            s.observe_cursor = ObserveCursor::default();
            Ok(())
        })
    }
    pub fn envelope_path(&self, number: u64) -> PathBuf {
        self.dir()
            .join("envelopes")
            .join(format!("{number:06}.json"))
    }

    pub fn envelope(&self, number: u64) -> Result<Envelope> {
        self.record("envelopes", &format!("{number:020}"))?
            .context("open envelope record missing")
    }

    pub fn envelopes(&self) -> Result<Vec<Envelope>> {
        let mut envelopes = Vec::new();
        for row in self.records("envelopes")? {
            envelopes
                .push(serde_json::from_value::<Envelope>(row.doc).context("decoding envelope")?);
        }
        envelopes.sort_by_key(|e| e.number);
        Ok(envelopes)
    }

    /// Attempted envelopes still eligible for evidence in their original session.
    /// This index is independent of the latest message-envelope mapping after resend.
    pub fn pending_observation_envelopes(&self) -> Result<Vec<Envelope>> {
        let _lock = self.lock()?;
        self.records("observation-pending")?
            .into_iter()
            .map(|row| {
                self.envelope(
                    row.key
                        .parse()
                        .context("invalid observation envelope key")?,
                )
            })
            .collect()
    }

    pub(super) fn has_unobserved_members(&self, envelope: &Envelope) -> Result<bool> {
        for seq in &envelope.members {
            if self.receipt(*seq, ReceiptLevel::Observed)?.is_none() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn clear_observation_index(&self, envelope: &Envelope) -> Result<()> {
        let key = format!("{:020}", envelope.number);
        self.delete_record("observation-pending", &key)?;
        for seq in &envelope.members {
            self.delete_record(&format!("observation-owners/{seq:020}"), &key)?;
        }
        Ok(())
    }

    pub fn payload(&self, envelope: &Envelope) -> Result<String> {
        let payload: String = self
            .record("payloads", &format!("{:020}", envelope.number))?
            .context("frozen envelope payload missing")?;
        ensure!(
            hash(&payload) == envelope.payload_sha256,
            "envelope payload hash mismatch"
        );
        Ok(payload)
    }

    /// Publish once under deliver lock; subsequent changes may only update outcome/stages/attempts.
    pub fn publish(
        &self,
        binding: &SessionBinding,
        adapter: &str,
        max_payload: usize,
        flush: bool,
    ) -> Result<Option<Envelope>> {
        let _lock = self.lock()?;
        let state = self.state()?;
        ensure!(
            state.open_envelope.is_none(),
            "recipient already has an open envelope"
        );
        let mut unread = Vec::new();
        for message in self.pending_messages()? {
            if message.binding.question_request.is_none()
                && self.receipt(message.seq, ReceiptLevel::Observed)?.is_none()
                && self.terminal(message.seq)?.is_none()
            {
                unread.push(message);
            }
        }
        if !flush && !unread.iter().any(|m| m.urgency == Urgency::Wake) {
            return Ok(None);
        }
        if unread.is_empty() {
            return Ok(None);
        }
        let number = state.next_envelope;
        let mut payload = format!("[corral envelope {}@env{number}]", self.key());
        let mut members = Vec::new();
        let mut owner = false;
        for message in unread {
            if payload.len() + 2 + message.body.len() > max_payload.min(MAX_PAYLOAD) {
                ensure!(
                    !members.is_empty(),
                    "stored message exceeds adapter payload cap"
                );
                break;
            }
            payload.push_str("\n\n");
            payload.push_str(&message.body);
            owner |= message.owner_kind();
            members.push(message.seq);
        }
        let payload_path = self
            .dir()
            .join("envelopes")
            .join(format!("{number:06}.payload"));
        let envelope = Envelope {
            number,
            members,
            binding: binding.clone(),
            adapter: adapter.into(),
            mode: if owner { "steer" } else { "follow_up" }.into(),
            payload_sha256: hash(&payload),
            payload_path,
            idempotency_key: format!("{}@env{number}", self.key()),
            outcome: None,
            stages: Vec::new(),
            attempts: Vec::new(),
        };
        self.update_state(|state| {
            self.save_record("payloads", &format!("{number:020}"), &payload)?;
            self.save_record("envelopes", &format!("{number:020}"), &envelope)?;
            for seq in &envelope.members {
                self.save_record("message-envelope", &format!("{seq:020}"), &number)?;
            }
            state.next_envelope = number
                .checked_add(1)
                .context("envelope sequence exhausted")?;
            state.open_envelope = Some(number);
            state.replacement_envelope = None;
            if state.binding.as_ref() != Some(binding) {
                state.observe_cursor = ObserveCursor::default();
            }
            state.binding = Some(binding.clone());
            Ok(())
        })?;
        Ok(Some(envelope))
    }

    /// Reattach a publication interrupted before its task state was saved.
    pub fn recover_publication(&self) -> Result<()> {
        let _lock = self.lock()?;
        let state = self.state()?;
        if let Some(number) = state.replacement_envelope {
            let envelope = self.envelope(number)?;
            let mut pending = false;
            for seq in &envelope.members {
                if self.receipt(*seq, ReceiptLevel::Observed)?.is_none()
                    && self.terminal(*seq)?.is_none()
                {
                    pending = true;
                    break;
                }
            }
            // A completed replacement obligation must not pin later independent messages.
            if !pending {
                self.update_state(|s| {
                    s.replacement_envelope = None;
                    Ok(())
                })?;
            }
        }
        if state.open_envelope.is_some() {
            return Ok(());
        }
        // Publication and its pointer commit together. Only the latest reserved identity
        // can need reattachment; older envelopes can never become open again.
        if let Some(envelope) = state
            .next_envelope
            .checked_sub(1)
            .filter(|number| *number > 0)
            .map(|number| self.record::<Envelope>("envelopes", &format!("{number:020}")))
            .transpose()?
            .flatten()
            && !self.settled(&envelope)?
        {
            self.update_state(|s| {
                s.open_envelope = Some(envelope.number);
                s.replacement_envelope = None;
                if s.binding.as_ref() != Some(&envelope.binding) {
                    s.observe_cursor = ObserveCursor::default();
                }
                s.binding = Some(envelope.binding.clone());
                Ok(())
            })?;
        }
        Ok(())
    }

    pub fn save_envelope(&self, envelope: &Envelope) -> Result<()> {
        let _lock = self.lock()?;
        let mut old = self.envelope(envelope.number)?;
        old.outcome = envelope.outcome.clone();
        old.stages.clone_from(&envelope.stages);
        old.attempts.clone_from(&envelope.attempts);
        ensure!(old == *envelope, "envelope immutable fields changed");
        self.payload(envelope)?;
        self.store.records_transaction(|| {
            let key = format!("{:020}", envelope.number);
            self.save_record("envelopes", &key, envelope)?;
            if !envelope.attempts.is_empty() && self.has_unobserved_members(envelope)? {
                self.save_record("observation-pending", &key, &true)?;
                for seq in &envelope.members {
                    if self.receipt(*seq, ReceiptLevel::Observed)?.is_none() {
                        self.save_record("late-evidence", &format!("{seq:020}"), &true)?;
                        self.save_record(&format!("observation-owners/{seq:020}"), &key, &true)?;
                    }
                }
            } else {
                self.clear_observation_index(envelope)?;
            }
            Ok(())
        })
    }

    pub fn settled(&self, envelope: &Envelope) -> Result<bool> {
        if self.owner_settled(envelope.number)? {
            return Ok(true);
        }
        for seq in &envelope.members {
            if self.receipt(*seq, ReceiptLevel::Observed)?.is_none()
                && self.terminal(*seq)?.is_none()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn close_settled(&self, envelope: &Envelope) -> Result<bool> {
        let _lock = self.lock()?;
        if !self.settled(envelope)? {
            return Ok(false);
        }
        self.update_state(|state| {
            if state.open_envelope == Some(envelope.number) {
                state.open_envelope = None;
            }
            Ok(())
        })?;
        Ok(true)
    }

    pub fn begin_attempt(&self, envelope: &mut Envelope, now: Timestamp) -> Result<()> {
        envelope.stages.push(Stage::Dispatching);
        envelope.attempts.push(Attempt {
            at: now,
            outcome: None,
        });
        self.save_envelope(envelope)
    }

    pub fn finish_attempt(&self, envelope: &mut Envelope, outcome: SendOutcome) -> Result<()> {
        envelope
            .attempts
            .last_mut()
            .context("no dispatching attempt")?
            .outcome = Some(outcome.clone());
        envelope.outcome = Some(outcome);
        envelope.stages.push(Stage::Outcome);
        self.save_envelope(envelope)?;
        self.event("envelope_attempt", json!({"envelope": envelope.number, "members": envelope.members,
            "adapter":envelope.adapter,"binding":envelope.binding,"mode":envelope.mode,"outcome":envelope.outcome}))
    }

    /// A started effect retains its original receipt across retries.
    pub fn start_effect(
        &self,
        seq: u64,
        stale: bool,
        decision_receipt: Option<crate::ResumeReceipt>,
    ) -> Result<Effect> {
        let _lock = self.lock()?;
        let key = format!("{seq:020}");
        if let Some(mut effect) = self.record::<Effect>("effects", &key)? {
            if stale && effect.status == EffectStatus::Started {
                effect.status = EffectStatus::SkippedStale;
                self.store.records_transaction(|| {
                    self.save_record("effects", &key, &effect)?;
                    self.refresh_message_index(seq)
                })?;
            }
            return Ok(effect);
        }
        let receipt = self
            .receipt(seq, ReceiptLevel::Observed)?
            .context("effect requires observed receipt")?;
        let effect = Effect {
            status: if stale {
                EffectStatus::SkippedStale
            } else {
                EffectStatus::Started
            },
            receipt,
            decision_receipt,
        };
        self.store.records_transaction(|| {
            self.save_record("effects", &key, &effect)?;
            self.refresh_message_index(seq)
        })?;
        Ok(effect)
    }

    pub fn finish_effect(&self, seq: u64) -> Result<()> {
        let _lock = self.lock()?;
        let key = format!("{seq:020}");
        let mut effect: Effect = self
            .record("effects", &key)?
            .context("effect not started")?;
        if effect.status != EffectStatus::Started {
            return Ok(());
        }
        self.update_state(|state| {
            state.effects_applied.insert(seq);
            effect.status = EffectStatus::Applied;
            self.save_record("effects", &key, &effect)?;
            self.refresh_message_index(seq)
        })
    }
}

fn hash(payload: &str) -> String {
    format!("{:x}", Sha256::digest(payload.as_bytes()))
}
