//! Queue evidence is independent of delivery receipts and turn completion.
//! Callers verify host interval coverage/provenance before projecting evidence here.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Inbox, SessionBinding};
use crate::agent::paths::sha256_hex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionScope {
    pub envelope: u64,
    pub generation: u32,
    pub binding: SessionBinding,
    /// Full committed corral_host_bound nonce digest, not its display prefix.
    pub bind: String,
    pub idempotency_key: String,
    pub text_sha256: String,
    pub text_bytes: u64,
}

impl ExecutionScope {
    pub(super) fn key(&self) -> Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(self)?))
    }

    pub(super) fn check_item(&self, item: &str) -> Result<()> {
        let prefix = format!("q-g{}-{}-", self.generation, &self.bind[..12]);
        let counter = item
            .strip_prefix(&prefix)
            .context("queue item binding mismatch")?;
        ensure!(
            counter
                .parse::<u64>()
                .is_ok_and(|n| n > 0 && n.to_string() == counter),
            "invalid queue item counter"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Queued,
    Started,
    Withdrawn,
    Rejected,
    Handled,
    Lost,
    Unknown,
}

impl ExecutionState {
    pub fn definite(self) -> bool {
        !matches!(self, Self::Queued | Self::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub seq: u64,
    pub scope: ExecutionScope,
    pub item: String,
    pub state: ExecutionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingExecution {
    pub seq: u64,
    pub scope: ExecutionScope,
}

impl Inbox<'_> {
    pub(super) fn check_execution_scope(&self, scope: &ExecutionScope) -> Result<Vec<u64>> {
        ensure!(scope.generation > 0, "invalid queue generation");
        ensure!(
            scope.bind.len() == 64
                && scope
                    .bind
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid full queue bind digest"
        );
        let envelope = self.envelope(scope.envelope)?;
        ensure!(
            envelope.binding == scope.binding
                && envelope.idempotency_key == scope.idempotency_key
                && envelope.payload_sha256 == scope.text_sha256
                && self.payload(&envelope)?.len() as u64 == scope.text_bytes,
            "queue envelope binding or payload mismatch"
        );
        Ok(envelope.members)
    }

    /// Register eligibility BEFORE delivery observation can retire its observation index.
    /// Only call for a generation that actually supports the queue ledger; no legacy backfill.
    pub fn track_execution(&self, scope: &ExecutionScope) -> Result<()> {
        let _lock = self.lock()?;
        let members = self.check_execution_scope(scope)?;
        self.store.records_transaction(|| {
            for seq in members {
                self.message(seq)?;
                let key = execution_key(seq, scope)?;
                if self
                    .execution(seq, scope)?
                    .is_some_and(|record| record.state.definite())
                {
                    continue;
                }
                let pending = PendingExecution {
                    seq,
                    scope: scope.clone(),
                };
                self.save_record("execution-pending", &key, &pending)?;
                self.save_record(
                    &format!("execution-pending/{}", scope.key()?),
                    &format!("{seq:020}"),
                    &pending,
                )?;
                self.save_record("execution-current", &format!("{seq:020}"), scope)?;
            }
            self.refresh_execution_recipient()?;
            Ok(())
        })
    }

    pub(super) fn refresh_execution_recipient(&self) -> Result<()> {
        if self.records("execution-pending")?.is_empty() {
            self.store
                .delete_record("inbox-execution-pending", self.key())
        } else {
            self.store
                .save_record("inbox-execution-pending", self.key(), &self.recipient)
        }
    }

    pub fn execution(&self, seq: u64, scope: &ExecutionScope) -> Result<Option<ExecutionRecord>> {
        self.record("execution", &execution_key(seq, scope)?)
    }

    /// Legacy messages have no current scope, hence no invented execution view.
    pub fn execution_view(&self, seq: u64) -> Result<Option<ExecutionRecord>> {
        self.record::<ExecutionScope>("execution-current", &format!("{seq:020}"))?
            .map(|scope| self.execution(seq, &scope))
            .transpose()
            .map(Option::flatten)
    }

    /// Output-sized eligibility, optionally restricted to one exact binding/envelope.
    pub fn pending_executions(
        &self,
        scope: Option<&ExecutionScope>,
    ) -> Result<Vec<PendingExecution>> {
        let area = scope
            .map(|scope| scope.key().map(|key| format!("execution-pending/{key}")))
            .transpose()?
            .unwrap_or_else(|| "execution-pending".into());
        self.records(&area)?
            .into_iter()
            .map(|row| {
                crate::work::record(crate::work::Work::InboxRecordRead, 1);
                serde_json::from_value(row.doc).context("decoding pending execution")
            })
            .collect()
    }

    /// Immutable evidence plus a monotonic projection, in one records unit. Matching replay
    /// is a no-op, including queued replay after a terminal. Conflicts are logged/refused.
    /// `lost` requires stopped-producer + complete-coverage proof at the observation layer.
    pub fn record_execution(&self, record: &ExecutionRecord) -> Result<bool> {
        let _lock = self.lock()?;
        let members = self.check_execution_scope(&record.scope)?;
        ensure!(
            members.contains(&record.seq),
            "execution is not an envelope member"
        );
        record.scope.check_item(&record.item)?;
        ensure!(
            record.reason.as_ref().is_none_or(|s| s.len() <= 1024),
            "queue reason too large"
        );
        let key = execution_key(record.seq, &record.scope)?;
        let prior = self.execution(record.seq, &record.scope)?;
        if let Some(prior) = &prior {
            ensure!(
                prior.item == record.item,
                "execution item identity mismatch"
            );
            if prior == record || record.state == ExecutionState::Queued && prior.state.definite() {
                return Ok(false);
            }
            if prior.state.definite() || prior.state == record.state {
                self.event(
                    "queue_execution_conflict",
                    json!({"prior":prior,"incoming":record}),
                )?;
                anyhow::bail!("conflicting queue execution evidence");
            }
            // Insufficient coverage must not erase known queue membership.
            if record.state == ExecutionState::Unknown {
                return Ok(false);
            }
        }
        self.store.records_transaction(|| {
            let event_key = sha256_hex(&serde_json::to_vec(record)?);
            self.save_record("execution-events", &event_key, record)?;
            self.save_record("execution", &key, record)?;
            // An older scope's replay must never select itself over a newer scope.
            let current =
                self.record::<ExecutionScope>("execution-current", &format!("{:020}", record.seq))?;
            if current.is_none() {
                self.save_record(
                    "execution-current",
                    &format!("{:020}", record.seq),
                    &record.scope,
                )?;
            }
            let scoped_area = format!("execution-pending/{}", record.scope.key()?);
            if record.state.definite() {
                self.delete_record("execution-pending", &key)?;
                self.delete_record(&scoped_area, &format!("{:020}", record.seq))?;
            } else {
                let pending = PendingExecution {
                    seq: record.seq,
                    scope: record.scope.clone(),
                };
                self.save_record("execution-pending", &key, &pending)?;
                self.save_record(&scoped_area, &format!("{:020}", record.seq), &pending)?;
            }
            self.refresh_execution_recipient()?;
            self.event("queue_execution", json!(record))?;
            Ok(true)
        })
    }
}

fn execution_key(seq: u64, scope: &ExecutionScope) -> Result<String> {
    Ok(format!("{}:{seq:020}", scope.key()?))
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
