use anyhow::{Context, Result, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::*;

/// Published ahead of every other unread message of its recipient (T-252 recovery).
const RECOVERY_NOTICE_KIND: &str = "recovery_notice";

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
        let actual = self.envelope(envelope.number)?;
        ensure!(
            actual.attempts.is_empty()
                && actual.outcome.is_none()
                && actual
                    .stages
                    .iter()
                    .all(|stage| matches!(stage, Stage::Blocked)),
            "attempted envelope cannot be retired"
        );
        for seq in &actual.members {
            // Observation settles this member; publication excludes it from the replacement.
            // Its receipt and cumulative bar remain authoritative, so this never replays it.
            ensure!(
                self.receipt(*seq, ReceiptLevel::Observed)?.is_some()
                    || self.receipt(*seq, ReceiptLevel::Admitted)?.is_none(),
                "submitted envelope cannot be retired"
            );
        }
        self.update_state(|s| {
            // Publication already authorized delivery, including an info-only owner flush.
            // Preserve that eligibility before retirement so interrupted rebinding can resume.
            self.save_record(
                "control",
                "flush",
                &json!({"at":self.store.now(),"via":"replacement","envelope":envelope.number}),
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
        crate::work::record(crate::work::Work::InboxEnvelopeRead, 1);
        self.record("envelopes", &format!("{number:020}"))?
            .context("open envelope record missing")
    }

    pub fn envelopes(&self) -> Result<Vec<Envelope>> {
        let mut envelopes = Vec::new();
        for row in self.records("envelopes")? {
            crate::work::record(crate::work::Work::InboxEnvelopeRead, 1);
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
        let state = self.state_head()?;
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
        // A resumed Pi host reads its recovery notice before anything queued while it was lost.
        unread.sort_by_key(|m| m.kind != RECOVERY_NOTICE_KIND);
        if unread.is_empty() {
            return Ok(None);
        }
        let number = state.next_envelope;
        // Escape member-looking lines in stored bodies, including messages posted by older
        // versions. The framing marker lets consumers reject ambiguous legacy payloads.
        let mut payload = format!(
            "[corral envelope {}@env{number}]\n[corral framing escaped-v1]",
            self.key()
        );
        let mut members = Vec::new();
        let mut owner = false;
        for message in unread {
            let body = message.body.replace("\n\n[corral ", "\n\n\\[corral ");
            if payload.len() + 2 + body.len() > max_payload.min(MAX_PAYLOAD) {
                ensure!(
                    !members.is_empty(),
                    "stored message exceeds adapter payload cap"
                );
                break;
            }
            payload.push_str("\n\n");
            payload.push_str(&body);
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
            retry_bar: None,
        };
        let next = number
            .checked_add(1)
            .context("envelope sequence exhausted")?;
        // A standalone freeze survives a failed pointer publication. In an existing
        // record/task unit both phases still join that unit and roll back together.
        self.store.records_transaction(|| {
            ensure!(
                self.record::<Envelope>("envelopes", &format!("{number:020}"))?
                    .is_none(),
                "frozen publication requires recovery before reuse"
            );
            self.save_record("payloads", &format!("{number:020}"), &payload)?;
            self.save_record("envelopes", &format!("{number:020}"), &envelope)?;
            for seq in &envelope.members {
                self.save_record("message-envelope", &format!("{seq:020}"), &number)?;
            }
            Ok(())
        })?;
        #[cfg(test)]
        super::header::checkpoint("envelope_written")?;
        self.update_state(|state| {
            state.next_envelope = next;
            state.open_envelope = Some(number);
            Self::annotate_owner(state, &envelope, false);
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
        let state = self.state_head()?;
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
            let envelope = self.envelope(state.open_envelope.context("open envelope")?)?;
            self.repair_owner_annotation(&envelope)?;
            return Ok(());
        }
        // Only the not-yet-pointed freeze or the latest published identity may need
        // reattachment. A settled latest identity must never reopen older history.
        for number in [
            Some(state.next_envelope),
            state.next_envelope.checked_sub(1).filter(|n| *n > 0),
        ]
        .into_iter()
        .flatten()
        {
            let Some(envelope) = self.record::<Envelope>("envelopes", &format!("{number:020}"))?
            else {
                continue;
            };
            let settled = self.settled(&envelope)?;
            let next = number
                .checked_add(1)
                .context("envelope sequence exhausted")?;
            if !settled || next > state.next_envelope {
                self.update_state(|s| {
                    s.next_envelope = s.next_envelope.max(next);
                    if !settled {
                        s.open_envelope = Some(envelope.number);
                        Self::annotate_owner(s, &envelope, true);
                        s.replacement_envelope = None;
                        if s.binding.as_ref() != Some(&envelope.binding) {
                            s.observe_cursor = ObserveCursor::default();
                        }
                        s.binding = Some(envelope.binding.clone());
                    }
                    Ok(())
                })?;
            }
            break;
        }
        Ok(())
    }

    pub fn save_envelope(&self, envelope: &Envelope) -> Result<()> {
        let _lock = self.lock()?;
        let old = self.envelope(envelope.number)?;
        self.validate_envelope(&old, envelope)?;
        let mut next = envelope.clone();
        next.retry_bar = old.retry_bar.clone().or(next.retry_bar);
        if old.outcome != envelope.outcome
            || old.attempts != envelope.attempts
            || old.retry_bar != next.retry_bar
            || old.stages.iter().chain(&envelope.stages).any(|stage| {
                !matches!(stage, Stage::Outcome | Stage::Blocked)
                    && old.stages.contains(stage) != envelope.stages.contains(stage)
            })
        {
            let first = next
                .retry_bar
                .get_or_insert(RetryWitness {
                    reason: WitnessReason::UntrackedMutation,
                    attempt: None,
                })
                .clone();
            self.project_members(&envelope.members, |entry| {
                entry.retry_evidence.coverage = RetryCoverage::Unknown;
                entry.retry_evidence.bar(first.reason, first.attempt);
                if entry.envelope == Some(envelope.number) {
                    entry.delivery = Some(DeliveryHeader {
                        state: DeliveryState::Unknown,
                        at: None,
                        attempt: None,
                    });
                }
            })?;
            self.retain_retry_bars(&envelope.members, &first)?;
        }
        // The bar and rewritten evidence commit atomically in the existing envelope record.
        self.persist_envelope(&next)
    }

    fn validate_envelope(&self, old: &Envelope, envelope: &Envelope) -> Result<()> {
        let mut old = old.clone();
        old.outcome = envelope.outcome.clone();
        old.stages.clone_from(&envelope.stages);
        old.attempts.clone_from(&envelope.attempts);
        old.retry_bar.clone_from(&envelope.retry_bar);
        ensure!(old == *envelope, "envelope immutable fields changed");
        self.payload(envelope)?;
        Ok(())
    }

    fn persist_envelope(&self, envelope: &Envelope) -> Result<()> {
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

    /// Records that envelope `number`'s bound session refused attempt `refusal.attempt` before
    /// admitting it. The adapter writes it before the attempt's outcome, so an interrupted
    /// attempt keeps the evidence.
    pub fn record_refusal(&self, number: u64, refusal: &Refusal) -> Result<()> {
        self.save_record("refused", &format!("{number:020}"), refusal)
    }

    /// The recorded pre-admission refusal of `envelope`'s latest attempt, when there is one.
    pub fn refusal(&self, envelope: &Envelope) -> Result<Option<Refusal>> {
        let latest = u64::try_from(envelope.attempts.len())?;
        Ok(self
            .record::<Refusal>("refused", &format!("{:020}", envelope.number))?
            .filter(|refusal| latest > 0 && refusal.attempt == latest))
    }

    /// The hold on an unsettled member of `envelope` (neither observed nor terminal): its latest
    /// attempt was refused before admission and the envelope was not yet requeued.
    pub(super) fn hold_of(&self, envelope: &Envelope) -> Result<Option<InboxHold>> {
        if self.owner_settled(envelope.number)? {
            return Ok(None);
        }
        Ok(self.refusal(envelope)?.map(|refusal| InboxHold {
            envelope: envelope.number,
            code: refusal.code,
            generation: refusal.generation,
            supervised: self.store.config.pm.supervise,
            release: PM_REPLACE.into(),
        }))
    }

    /// Settles a refused envelope (see [`Refusal`]) without a terminal state: each member that
    /// is neither observed nor terminal returns to pending, so the next publication carries it
    /// in a fresh envelope with a new idempotency key. Caller holds the deliver lock.
    pub fn requeue_refused(&self, envelope: &Envelope, refusal: &Refusal) -> Result<Vec<u64>> {
        let _lock = self.lock()?;
        let mut members = Vec::new();
        for seq in &envelope.members {
            if self.receipt(*seq, ReceiptLevel::Observed)?.is_none()
                && self.terminal(*seq)?.is_none()
            {
                members.push(*seq);
            }
        }
        let number = envelope.number;
        let ids: Vec<String> = members
            .iter()
            .map(|seq| format!("{}#{seq}", self.key()))
            .collect();
        self.update_state(|s| {
            if !members.is_empty() {
                // The refused publication authorized delivery, even of an info-only flush.
                self.save_record(
                    "control",
                    "flush",
                    &json!({"at":self.store.now(),"via":"requeue","envelope":number}),
                )?;
            }
            self.save_record(
                "settled",
                &format!("{number:020}"),
                &json!({"via":"requeue","code":refusal.code,"generation":refusal.generation,
                    "at":self.store.now()}),
            )?;
            if s.open_envelope == Some(number) {
                s.open_envelope = None;
            }
            self.event(
                "delivery_requeued",
                json!({"envelope":number,"code":refusal.code,"generation":refusal.generation,
                    "members":ids}),
            )
        })?;
        Ok(members)
    }

    pub fn begin_attempt(&self, envelope: &mut Envelope, now: Timestamp) -> Result<()> {
        let _lock = self.lock()?;
        let old = self.envelope(envelope.number)?;
        envelope.retry_bar.clone_from(&old.retry_bar);
        ensure!(old == *envelope, "stale envelope attempt");
        ensure!(
            self.state_head()?.open_envelope == Some(envelope.number),
            "attempt requires the current open publication"
        );
        self.validate_envelope(&old, envelope)?;
        let attempt = AttemptRef {
            envelope: envelope.number,
            ordinal: u64::try_from(envelope.attempts.len())?
                .checked_add(1)
                .context("attempt ordinal exhausted")?,
        };
        #[cfg(test)]
        super::header::checkpoint("before_attempt_fence")?;
        self.project_members(&envelope.members, |entry| {
            if let Some(first) = &envelope.retry_bar {
                entry.retry_evidence.bar(first.reason, first.attempt);
            }
            if entry
                .retry_evidence
                .pending
                .is_some_and(|pending| pending != attempt)
            {
                entry
                    .retry_evidence
                    .bar(WitnessReason::UnresolvedPrior, entry.retry_evidence.pending);
            }
            entry.retry_evidence.pending = Some(attempt);
            entry.delivery = Some(DeliveryHeader {
                state: DeliveryState::Dispatching,
                at: Some(now),
                attempt: Some(attempt),
            });
        })?;
        #[cfg(test)]
        super::header::checkpoint("attempt_fenced")?;
        envelope.stages.push(Stage::Dispatching);
        envelope.attempts.push(Attempt {
            at: now,
            outcome: None,
        });
        self.persist_envelope(envelope)?;
        #[cfg(test)]
        super::header::checkpoint("attempt_written")?;
        Ok(())
    }

    pub fn finish_attempt(&self, envelope: &mut Envelope, outcome: SendOutcome) -> Result<()> {
        let _lock = self.lock()?;
        let old = self.envelope(envelope.number)?;
        self.validate_envelope(&old, envelope)?;
        envelope.retry_bar.clone_from(&old.retry_bar);
        let attempt = AttemptRef {
            envelope: envelope.number,
            ordinal: u64::try_from(envelope.attempts.len())?,
        };
        ensure!(attempt.ordinal > 0, "no dispatching attempt");
        let (state, witness) = match &outcome {
            SendOutcome::Rejected { .. } => (DeliveryState::Rejected, None),
            SendOutcome::Accepted => (DeliveryState::Accepted, Some(WitnessReason::Accepted)),
            SendOutcome::Admitted { .. } => {
                (DeliveryState::Admitted, Some(WitnessReason::Admitted))
            }
            SendOutcome::Held { .. } => (DeliveryState::Held, Some(WitnessReason::Held)),
            SendOutcome::Uncertain { .. } => {
                (DeliveryState::Uncertain, Some(WitnessReason::Uncertain))
            }
            SendOutcome::Conflicted { .. } => {
                (DeliveryState::Conflicted, Some(WitnessReason::Conflicted))
            }
        };
        // A stale earlier callback cannot overwrite a later attempt or clear its fence.
        if old.attempts.len() != envelope.attempts.len() {
            if let Some(reason) = witness {
                let mut current = old;
                let first = current
                    .retry_bar
                    .get_or_insert(RetryWitness {
                        reason,
                        attempt: Some(attempt),
                    })
                    .clone();
                let projected = self.project_members(&current.members, |entry| {
                    entry.retry_evidence.bar(first.reason, first.attempt);
                });
                // The keyed proof also fences a completed newer attempt if task saving fails.
                self.retain_retry_bars(&current.members, &first)?;
                self.persist_envelope(&current)?;
                #[cfg(test)]
                super::header::checkpoint("stale_attempt_bar_written")?;
                projected?;
            }
            return Ok(());
        }
        let persisted = old.attempts.last().context("no dispatching attempt")?;
        if let Some(previous) = &persisted.outcome {
            ensure!(previous == &outcome, "attempt outcome already recorded");
            if let Some(first) = &old.retry_bar {
                self.project_members(&old.members, |entry| {
                    entry.retry_evidence.bar(first.reason, first.attempt);
                })?;
            }
            return Ok(());
        }
        ensure!(old == *envelope, "stale envelope outcome");
        envelope
            .attempts
            .last_mut()
            .context("no dispatching attempt")?
            .outcome = Some(outcome.clone());
        envelope.outcome = Some(outcome);
        envelope.stages.push(Stage::Outcome);
        if let Some(reason) = witness {
            envelope.retry_bar.get_or_insert(RetryWitness {
                reason,
                attempt: Some(attempt),
            });
        }
        #[cfg(test)]
        super::header::checkpoint("before_attempt_outcome")?;
        self.persist_envelope(envelope)?;
        #[cfg(test)]
        super::header::checkpoint("attempt_outcome_written")?;
        self.project_members(&envelope.members, |entry| {
            let evidence = &mut entry.retry_evidence;
            if let Some(first) = &envelope.retry_bar {
                evidence.bar(first.reason, first.attempt);
            }
            if evidence.pending == Some(attempt) {
                if let Some(SendOutcome::Rejected {
                    retryable_after_fix,
                    ..
                }) = &envelope.outcome
                    && evidence.coverage == RetryCoverage::Complete
                    && evidence.route == RetryRoute::EnvelopeOnly
                    && !matches!(evidence.history, RetryHistory::Barred { .. })
                {
                    evidence.history = RetryHistory::RejectedOnly {
                        last: attempt,
                        retryable_after_fix: *retryable_after_fix,
                    };
                }
                if matches!(envelope.outcome, Some(SendOutcome::Rejected { .. })) {
                    evidence.pending = None;
                }
                if entry.envelope == Some(envelope.number) {
                    entry.delivery = Some(DeliveryHeader {
                        state,
                        at: Some(persisted.at),
                        attempt: Some(attempt),
                    });
                }
            }
        })?;
        #[cfg(test)]
        super::header::checkpoint("attempt_projected")?;
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
