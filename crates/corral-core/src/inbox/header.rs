//! Bounded owner-message identities and cumulative delivery proof, never a history scan.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::storage::read_optional;
use super::*;
use crate::work::{self, Work};

#[derive(Deserialize)]
struct RetryClaim {
    at: Timestamp,
    from: Option<String>,
    to: Option<String>,
}

pub const PRIMARY_CAPACITY: usize = 8;
pub const OWNER_CAPACITY: usize = 16;

#[cfg(test)]
thread_local! {
    pub(super) static CRASH_AT: std::cell::Cell<Option<&'static str>> = const {std::cell::Cell::new(None)};
}

#[cfg(test)]
pub(super) fn checkpoint(point: &'static str) -> Result<()> {
    ensure!(
        !CRASH_AT.with(|selected| selected.get() == Some(point)),
        "injected_crash: {point}"
    );
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerHeader {
    pub entries: Vec<OwnerMessage>,
    pub overflow_entries: Vec<OwnerMessage>,
    pub overflow_since: Option<Timestamp>,
    pub full_since: Option<Timestamp>,
    pub legacy_debt: Option<LegacyDebt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerMessage {
    pub seq: u64,
    pub kind: String,
    pub created: Timestamp,
    pub envelope: Option<u64>,
    pub delivery: Option<DeliveryHeader>,
    #[serde(default)]
    pub retry_evidence: RetryEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyDebt {
    pub phase: IndexPhase,
    pub next_seq: u64,
    pub end_seq: u64,
    pub latest_observed_owner: Option<u64>,
    pub started: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexPhase {
    Watermark,
    Collect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRef {
    pub envelope: u64,
    pub ordinal: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryCoverage {
    Complete,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryRoute {
    #[default]
    EnvelopeOnly,
    AlternateUnknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RetryHistory {
    #[default]
    NeverAttempted,
    RejectedOnly {
        last: AttemptRef,
        retryable_after_fix: bool,
    },
    Barred {
        first: RetryWitness,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryWitness {
    pub reason: WitnessReason,
    pub attempt: Option<AttemptRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WitnessReason {
    Accepted,
    Admitted,
    Held,
    Uncertain,
    Conflicted,
    Observed,
    UnresolvedPrior,
    AlternateSubmission,
    UntrackedMutation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryEvidence {
    pub coverage: RetryCoverage,
    pub route: RetryRoute,
    pub history: RetryHistory,
    pub pending: Option<AttemptRef>,
}

impl RetryEvidence {
    pub fn permits(&self, terminal: TerminalState) -> bool {
        self.coverage == RetryCoverage::Complete
            && self.route == RetryRoute::EnvelopeOnly
            && self.pending.is_none()
            && match self.history {
                RetryHistory::NeverAttempted => true,
                RetryHistory::RejectedOnly {
                    retryable_after_fix,
                    ..
                } => terminal == TerminalState::Failed && retryable_after_fix,
                RetryHistory::Barred { .. } => false,
            }
    }

    pub(super) fn bar(&mut self, reason: WitnessReason, attempt: Option<AttemptRef>) {
        if !matches!(self.history, RetryHistory::Barred { .. }) {
            self.history = RetryHistory::Barred {
                first: RetryWitness { reason, attempt },
            };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Published,
    Dispatching,
    Rejected,
    Accepted,
    Admitted,
    Held,
    Uncertain,
    Conflicted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryHeader {
    pub state: DeliveryState,
    pub at: Option<Timestamp>,
    pub attempt: Option<AttemptRef>,
}

/// Five keyed receipt/terminal probes; the retry claim is mirrored in the failed-record slot.
/// No body/envelope reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerEntry {
    pub message: OwnerMessage,
    pub admitted: Option<Receipt>,
    pub observed: Option<Receipt>,
    pub terminal: Option<TerminalState>,
    pub publication_gap: bool,
    #[serde(default)]
    pub retry_claimed_at: Option<Timestamp>,
    #[serde(default)]
    pub retry_successor: Option<String>,
}

impl OwnerEntry {
    /// Bounded evidence eligibility; callers additionally check live recipient and capacity.
    pub fn retry_eligible(&self) -> bool {
        !self.publication_gap
            && self.retry_claimed_at.is_none()
            && self.retry_successor.is_none()
            && self.observed.is_none()
            && self.admitted.is_none()
            && (self.message.envelope.is_none() || self.message.delivery.is_some())
            && self
                .terminal
                .is_some_and(|terminal| self.message.retry_evidence.permits(terminal))
    }
}

impl OwnerHeader {
    pub fn iter(&self) -> impl Iterator<Item = &OwnerMessage> {
        self.entries.iter().chain(&self.overflow_entries)
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = &mut OwnerMessage> {
        self.entries.iter_mut().chain(&mut self.overflow_entries)
    }

    pub fn len(&self) -> usize {
        self.entries.len() + self.overflow_entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn insert(&mut self, entry: OwnerMessage) {
        if self.entries.len() < PRIMARY_CAPACITY && self.overflow_entries.is_empty() {
            self.entries.push(entry);
        } else {
            self.overflow_entries.push(entry);
        }
        self.timestamps();
    }

    fn timestamps(&mut self) {
        if self.overflow_entries.is_empty() {
            self.overflow_since = None;
        } else if self.overflow_since.is_none() {
            self.overflow_since = self.overflow_entries.first().map(|entry| entry.created);
        }
        if self.len() < OWNER_CAPACITY {
            self.full_since = None;
        } else if self.full_since.is_none() {
            self.full_since = self.iter().map(|entry| entry.created).max();
        }
    }
}

impl Inbox<'_> {
    /// Container closure is semantic completion, not an observation or terminal delivery label.
    pub fn recipient_ended(&self, task: &crate::Task) -> bool {
        use crate::agent::{ContainerRef, container_closure};
        match &self.recipient {
            Recipient::Run { task: id, run } => {
                let current = task.current_run();
                if current.is_some_and(|record| {
                    record.agent_host.is_some()
                        || record.harness.as_deref().is_some_and(crate::is_pi_native)
                }) {
                    container_closure(
                        task,
                        &ContainerRef::Worker {
                            task: id.clone(),
                            run: run.clone(),
                        },
                    )
                    .is_some()
                } else {
                    current.is_none_or(|record| {
                        record.id != *run || record.outcome.is_some() || record.ended.is_some()
                    })
                }
            }
            Recipient::ReviewRun {
                task: id,
                run,
                review,
            } => container_closure(
                task,
                &ContainerRef::Reviewer {
                    task: id.clone(),
                    run: run.clone(),
                    review: review.clone(),
                },
            )
            .is_some(),
            Recipient::PlanRun { plan_run, .. } => task
                .plan_runs
                .last()
                .is_none_or(|record| record.id != *plan_run || !record.holds_slot()),
            Recipient::PlanReview { plan, review, .. } => task
                .plans
                .iter()
                .find(|record| record.id == *plan)
                .and_then(|record| record.reviews.last())
                .is_none_or(|record| record.id != *review || record.verdict.is_some()),
            Recipient::Pm => false,
        }
    }

    pub(super) fn annotate_owner(state: &mut InboxState, envelope: &Envelope, recovered: bool) {
        if let Some(header) = &mut state.owner_header {
            for entry in header
                .iter_mut()
                .filter(|entry| envelope.members.contains(&entry.seq))
            {
                if recovered
                    && entry.envelope != Some(envelope.number)
                    && (!envelope.attempts.is_empty()
                        || envelope.outcome.is_some()
                        || envelope
                            .stages
                            .iter()
                            .any(|stage| !matches!(stage, Stage::Blocked)))
                {
                    entry.retry_evidence.coverage = RetryCoverage::Unknown;
                    entry
                        .retry_evidence
                        .bar(WitnessReason::UntrackedMutation, None);
                }
                if entry.envelope != Some(envelope.number) {
                    entry.envelope = Some(envelope.number);
                    entry.delivery = Some(DeliveryHeader {
                        state: if recovered && !envelope.attempts.is_empty() {
                            DeliveryState::Unknown
                        } else {
                            DeliveryState::Published
                        },
                        at: None,
                        attempt: None,
                    });
                }
            }
        }
    }

    pub(super) fn repair_owner_annotation(&self, envelope: &Envelope) -> Result<()> {
        let mut next = self.state()?;
        let old = next.owner_header.clone();
        Self::annotate_owner(&mut next, envelope, true);
        if old != next.owner_header {
            self.update_state(|state| {
                Self::annotate_owner(state, envelope, true);
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Initialize debt in constant work; this never indexes historical messages.
    pub fn initialize_owner_header(state: &mut InboxState, now: Timestamp) {
        if state.owner_header.is_some() {
            return;
        }
        state.owner_header = Some(OwnerHeader {
            legacy_debt: (state.next_seq > 1).then_some(LegacyDebt {
                phase: IndexPhase::Watermark,
                next_seq: 1,
                end_seq: state.next_seq.saturating_sub(1),
                latest_observed_owner: None,
                started: now,
            }),
            ..Default::default()
        });
    }

    /// Read a caller's task snapshot, without listing directories or decoding envelopes.
    pub fn owner_entries(&self, state: &InboxState) -> Result<Vec<OwnerEntry>> {
        Ok(self
            .owner_entries_with_records(state)?
            .into_iter()
            .map(|(entry, _)| entry)
            .collect())
    }

    fn owner_entries_with_records(
        &self,
        state: &InboxState,
    ) -> Result<Vec<(OwnerEntry, Option<Value>)>> {
        let Some(header) = &state.owner_header else {
            return Ok(Vec::new());
        };
        ensure!(
            header.entries.len() <= PRIMARY_CAPACITY
                && header.overflow_entries.len() <= PRIMARY_CAPACITY,
            "owner header exceeds bounded capacity"
        );
        header.iter().map(|entry| self.owner_entry(entry)).collect()
    }

    fn owner_entry(&self, entry: &OwnerMessage) -> Result<(OwnerEntry, Option<Value>)> {
        let admitted = self.receipt(entry.seq, ReceiptLevel::Admitted)?;
        let observed = self.receipt(entry.seq, ReceiptLevel::Observed)?;
        let mut terminal = None;
        let mut failed_record = None;
        let mut message = entry.clone();
        for state in [
            TerminalState::Cancelled,
            TerminalState::Failed,
            TerminalState::Abandoned,
        ] {
            work::record(Work::InboxRecordRead, 1);
            let record = read_optional::<Value>(&self.record_path(entry.seq, state.suffix()))?;
            if let Some(record) = &record {
                if let Some(first) = super::storage::retry_bar(record)? {
                    // This durable first witness can predate a failed task projection.
                    message.retry_evidence.history = RetryHistory::Barred { first };
                }
                if !super::storage::proof_only(record) && terminal.is_none() && observed.is_none() {
                    terminal = Some(state);
                }
            }
            if state == TerminalState::Failed {
                failed_record = record;
            }
        }
        // The exact `retried` marker stays authoritative for retry; the hot read uses its
        // mirror in the failed-record slot already probed above.
        let retry = failed_record
            .as_ref()
            .and_then(|record| record.get("retry_claim"))
            .filter(|claim| !claim.is_null())
            .map(|claim| {
                serde_json::from_value::<RetryClaim>(claim.clone())
                    .context("parsing mirrored retry claim")
            })
            .transpose()?;
        if let Some(from) = retry.as_ref().and_then(|claim| claim.from.as_ref()) {
            ensure!(
                *from == format!("{}#{}", self.key(), entry.seq),
                "retry claim does not match its exact message"
            );
        }
        Ok((
            OwnerEntry {
                message,
                admitted,
                observed,
                terminal,
                publication_gap: !self.message_path(entry.seq).is_file(),
                retry_claimed_at: retry.as_ref().map(|claim| claim.at),
                retry_successor: retry.and_then(|claim| claim.to),
            },
            failed_record,
        ))
    }

    /// Caller holds inbox -> task locks. Prune only settled, superseded obligations.
    pub(super) fn maintain_header(&self, state: &mut InboxState, now: Timestamp) -> Result<()> {
        let (header, handoffs) = self.preview_header(state, now)?;
        for (seq, proof) in handoffs {
            // Handoff proof before pruning. Failure leaves the task's entry intact.
            super::storage::write_json(
                &self.record_path(seq, TerminalState::Failed.suffix()),
                &proof,
            )?;
        }
        state.owner_header = Some(header);
        Ok(())
    }

    /// Compute the projected header and required handoffs without writing recipient evidence.
    fn preview_header(
        &self,
        state: &InboxState,
        now: Timestamp,
    ) -> Result<(OwnerHeader, Vec<(u64, Value)>)> {
        // Maintenance needs only bounded identities and the legacy sequence watermark,
        // never a copy of historical effects or transcript cursors.
        let mut projection = InboxState {
            owner_header: state.owner_header.clone(),
            next_seq: state.next_seq,
            ..Default::default()
        };
        Self::initialize_owner_header(&mut projection, now);
        let reads = self.owner_entries_with_records(&projection)?;
        let entries = reads.iter().map(|(entry, _)| entry);
        let header = projection
            .owner_header
            .as_mut()
            .context("initialized owner header")?;
        let watermark = entries
            .clone()
            .filter(|entry| entry.observed.is_some())
            .map(|entry| entry.message.seq)
            .chain(
                header
                    .legacy_debt
                    .as_ref()
                    .and_then(|debt| debt.latest_observed_owner),
            )
            .max();
        let mut resolved = BTreeSet::new();
        let mut handoffs = Vec::new();
        for (entry, record) in reads {
            if watermark.is_some_and(|seq| entry.message.seq < seq)
                && (entry.observed.is_some() || entry.terminal.is_some())
            {
                if entry.observed.is_none() {
                    let first = match &entry.message.retry_evidence.history {
                        RetryHistory::Barred { first } => Some(first.clone()),
                        RetryHistory::NeverAttempted | RetryHistory::RejectedOnly { .. } => entry
                            .message
                            .retry_evidence
                            .pending
                            .map(|attempt| RetryWitness {
                                reason: WitnessReason::UnresolvedPrior,
                                attempt: Some(attempt),
                            }),
                    };
                    if let Some(first) = first {
                        handoffs.push((
                            entry.message.seq,
                            super::storage::retry_bar_record(record, &first)?,
                        ));
                    }
                }
                resolved.insert(entry.message.seq);
            }
        }
        header
            .entries
            .retain(|entry| !resolved.contains(&entry.seq));
        header
            .overflow_entries
            .retain(|entry| !resolved.contains(&entry.seq));
        while header.entries.len() < PRIMARY_CAPACITY && !header.overflow_entries.is_empty() {
            header.entries.push(header.overflow_entries.remove(0));
        }
        if let Some(debt) = &mut header.legacy_debt {
            debt.latest_observed_owner = watermark;
        }
        header.timestamps();
        Ok((
            projection
                .owner_header
                .context("initialized owner header")?,
            handoffs,
        ))
    }

    /// Bounded maintenance, at most one task write; owning sync calls once per recipient.
    pub fn maintain_owner_header(&self, now: Timestamp) -> Result<bool> {
        let _lock = self.lock()?;
        let old = self.state()?;
        let mut next = old.clone();
        self.maintain_header(&mut next, now)?;
        if let Some(id) = self.recipient.task()
            && self.recipient_ended(&self.store.get(id)?)
        {
            next.owner_header = Some(OwnerHeader::default());
        }
        if next == old {
            return Ok(false);
        }
        #[cfg(test)]
        checkpoint("before_promotion")?;
        self.update_state(|state| {
            state.owner_header = next.owner_header;
            Ok(())
        })?;
        #[cfg(test)]
        checkpoint("promotion_committed")?;
        work::record(Work::InboxHeaderWrite, 1);
        Ok(true)
    }

    pub(super) fn admit_owner(&self, state: &mut InboxState, kind: &str) -> Result<()> {
        // Fresh provenance starts at the first reservation, even when it is not owner-kind.
        // Otherwise a later owner message would mistake this upgraded writer for legacy debt.
        if self.recipient != Recipient::Pm && state.next_seq == 1 {
            Self::initialize_owner_header(state, Timestamp::now());
        }
        if self.recipient == Recipient::Pm || !owner_kind(kind) {
            return Ok(());
        }
        self.maintain_header(state, Timestamp::now())?;
        self.check_owner_capacity(
            state
                .owner_header
                .as_ref()
                .context("initialized owner header")?,
        )
    }

    pub(super) fn preview_owner_admission(&self, state: &InboxState, kind: &str) -> Result<()> {
        if self.recipient == Recipient::Pm || !owner_kind(kind) {
            return Ok(());
        }
        let (header, _) = self.preview_header(state, Timestamp::now())?;
        self.check_owner_capacity(&header)
    }

    fn check_owner_capacity(&self, header: &OwnerHeader) -> Result<()> {
        ensure!(
            header.legacy_debt.is_none(),
            "owner_header_index_pending: {}: run `corral inbox index-owner {} --limit 64`",
            self.key(),
            self.key()
        );
        ensure!(
            header.len() < OWNER_CAPACITY,
            "owner_header_full: {} occupancy {}/16 retained {:?}; inspect retained IDs with `corral inbox show <id>`; resume after settled supersession or recipient completion",
            self.key(),
            header.len(),
            header
                .iter()
                .map(|entry| format!("{}#{}", self.key(), entry.seq))
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    pub(super) fn retain_owner(state: &mut InboxState, message: &Message) {
        if !message.owner_kind() {
            return;
        }
        if let Some(header) = &mut state.owner_header {
            let alternate = message.binding.question_request.is_some();
            header.insert(OwnerMessage {
                seq: message.seq,
                kind: message.kind.clone(),
                created: message.created,
                envelope: None,
                delivery: None,
                retry_evidence: RetryEvidence {
                    coverage: if alternate {
                        RetryCoverage::Unknown
                    } else {
                        RetryCoverage::Complete
                    },
                    route: if alternate {
                        RetryRoute::AlternateUnknown
                    } else {
                        RetryRoute::EnvelopeOnly
                    },
                    ..Default::default()
                },
            });
        }
    }

    /// Explicit, budgeted two-phase legacy identity indexing. Never reads envelopes.
    pub fn index_owner(&self, limit: usize) -> Result<OwnerHeader> {
        ensure!((1..=64).contains(&limit), "index-owner limit must be 1..64");
        let _lock = self.lock()?;
        self.update_state(|state| {
            self.maintain_header(state, Timestamp::now())?;
            let header = state
                .owner_header
                .as_mut()
                .context("initialized owner header")?;
            let Some(mut debt) = header.legacy_debt.take() else {
                return Ok(header.clone());
            };
            let mut read = 0;
            while read < limit {
                if debt.next_seq > debt.end_seq {
                    match debt.phase {
                        IndexPhase::Watermark => {
                            debt.phase = IndexPhase::Collect;
                            debt.next_seq = 1;
                        }
                        IndexPhase::Collect => break,
                    }
                }
                work::record(Work::InboxIndexRead, 1);
                let message = self.message(debt.next_seq)?;
                read += 1;
                if message.owner_kind() {
                    let observed = self.receipt(message.seq, ReceiptLevel::Observed)?.is_some();
                    match debt.phase {
                        IndexPhase::Watermark => {
                            if observed {
                                debt.latest_observed_owner = Some(message.seq);
                            }
                        }
                        IndexPhase::Collect => {
                            let superseded = debt
                                .latest_observed_owner
                                .is_some_and(|seq| message.seq < seq);
                            if !(superseded && (observed || self.terminal(message.seq)?.is_some()))
                                && !header.iter().any(|entry| entry.seq == message.seq)
                            {
                                if header.len() == OWNER_CAPACITY {
                                    break;
                                }
                                header.insert(OwnerMessage {
                                    seq: message.seq,
                                    kind: message.kind,
                                    created: message.created,
                                    envelope: None,
                                    delivery: None,
                                    retry_evidence: RetryEvidence {
                                        route: if message.binding.question_request.is_some() {
                                            RetryRoute::AlternateUnknown
                                        } else {
                                            RetryRoute::EnvelopeOnly
                                        },
                                        ..Default::default()
                                    },
                                });
                            }
                        }
                    }
                }
                debt.next_seq = debt
                    .next_seq
                    .checked_add(1)
                    .context("owner index exhausted")?;
            }
            if debt.phase != IndexPhase::Collect || debt.next_seq <= debt.end_seq {
                header.legacy_debt = Some(debt);
            }
            Ok(header.clone())
        })
    }

    /// Projection updates merge with the current task under its lock, never a cached header.
    pub(super) fn project_members(
        &self,
        members: &[u64],
        mut update: impl FnMut(&mut OwnerMessage),
    ) -> Result<()> {
        let mut state = self.state()?;
        let old = state.owner_header.clone();
        if let Some(header) = &mut state.owner_header {
            for entry in header
                .iter_mut()
                .filter(|entry| members.contains(&entry.seq))
            {
                update(entry);
            }
        }
        if old != state.owner_header {
            self.update_state(|current| {
                if let Some(header) = &mut current.owner_header {
                    for entry in header
                        .iter_mut()
                        .filter(|entry| members.contains(&entry.seq))
                    {
                        update(entry);
                    }
                }
                Ok(())
            })?;
            work::record(Work::InboxHeaderWrite, 1);
        }
        Ok(())
    }
}

pub(super) fn owner_kind(kind: &str) -> bool {
    matches!(
        kind,
        "owner_reply" | "owner_note" | "changes_requested" | "control_request"
    )
}
