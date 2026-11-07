//! Immutable mutation intents and outcomes. No bridge/network I/O in a Store unit.
//! Services hold the delivery lease across prepare, external I/O and settlement;
//! these entry points acquire inbox before task. Authorization belongs to that service.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::*;
use crate::agent::paths::sha256_hex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueReplacement {
    /// Unframed bytes frozen before contacting the host, never a body-file reference.
    pub body: String,
    pub text_sha256: String,
    pub max_payload: usize,
}

impl QueueReplacement {
    pub fn new(body: String, max_payload: usize) -> Self {
        Self {
            text_sha256: sha256_hex(body.as_bytes()),
            body,
            max_payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueOperationKind {
    Withdraw,
    Edit { replacement: QueueReplacement },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueOperationRequest {
    pub members: Vec<u64>,
    pub scope: ExecutionScope,
    pub item: Option<String>,
    pub operation: QueueOperationKind,
    pub actor: String,
    pub reason: String,
    pub expected: Option<ExecutionState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueOperationIntent {
    pub version: u32,
    pub number: u64,
    pub op_key: String,
    pub digest: String,
    pub request: QueueOperationRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum QueueWithdrawalOutcome {
    Withdrawn,
    AlreadyStarted { turn: Option<u64> },
    AlreadyWithdrawn { first_op_key: String },
    UnknownItem,
    Refused { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QueueReplacementOutcome {
    Published { message: String },
    Blocked { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueOperationOutcome {
    pub version: u32,
    pub op_key: String,
    pub withdrawal: QueueWithdrawalOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<QueueReplacementOutcome>,
}

impl Inbox<'_> {
    /// Recipient discovery for sync without decoding terminal message/task/operation history.
    pub fn queue_work_recipients(store: &crate::Store) -> Result<Vec<Recipient>> {
        let mut recipients = std::collections::BTreeMap::new();
        for area in ["inbox-execution-pending", "inbox-queue-operations-open"] {
            for row in store.record_rows(area)? {
                let recipient: Recipient =
                    serde_json::from_value(row.doc).context("decoding queue work recipient")?;
                ensure!(recipient.key()? == row.key, "queue work recipient mismatch");
                recipients.insert(row.key, recipient);
            }
        }
        Ok(recipients.into_values().collect())
    }

    /// Deterministic request identity means a restarted invocation finds its original intent,
    /// including after outcome commit. Changed reason/body cannot repurpose an operation key.
    pub fn prepare_queue_operation(
        &self,
        request: QueueOperationRequest,
    ) -> Result<QueueOperationIntent> {
        ensure!(
            !self.store.has_record_unit(),
            "queue intent must commit before external I/O"
        );
        let _lock = self.lock()?;
        let digest = sha256_hex(&serde_json::to_vec(&request)?);
        let op_key = format!("qo-{digest}");
        if let Some(prior) = self.queue_operation(&op_key)? {
            ensure!(
                prior.request == request,
                "queue operation identity conflict"
            );
            return Ok(prior);
        }
        self.check_queue_request(&request)?;
        let scope_key = request.scope.key()?;
        ensure!(
            self.record::<String>("queue-operation-owner", &scope_key)?
                .is_none(),
            "queue item/envelope already has an open operation"
        );
        self.store.records_transaction(|| {
            let number = self
                .record::<u64>("queue-operation-counter", "next")?
                .unwrap_or(1);
            let intent = QueueOperationIntent {
                version: 1,
                number,
                op_key: op_key.clone(),
                digest,
                request,
            };
            self.save_record(
                "queue-operation-counter",
                "next",
                &number
                    .checked_add(1)
                    .context("queue operation counter exhausted")?,
            )?;
            self.save_record("queue-operation-requests", &op_key, &intent)?;
            self.save_record("queue-operations-open", &op_key, &intent)?;
            self.save_record("queue-operation-owner", &scope_key, &op_key)?;
            self.store
                .save_record("inbox-queue-operations-open", self.key(), &self.recipient)?;
            self.event(
                "queue_operation_prepared",
                json!({"op_key":op_key,"number":number}),
            )?;
            #[cfg(test)]
            super::header::checkpoint("queue_operation_prepare")?;
            Ok(intent)
        })
    }

    pub fn queue_operation(&self, op_key: &str) -> Result<Option<QueueOperationIntent>> {
        let intent = self.record::<QueueOperationIntent>("queue-operation-requests", op_key)?;
        if let Some(intent) = &intent {
            check_intent(op_key, intent)?;
        }
        Ok(intent)
    }

    pub fn queue_operation_outcome(&self, op_key: &str) -> Result<Option<QueueOperationOutcome>> {
        let outcome = self.record::<QueueOperationOutcome>("queue-operation-outcomes", op_key)?;
        if let Some(outcome) = &outcome {
            ensure!(
                outcome.version == 1 && outcome.op_key == op_key,
                "queue outcome identity mismatch"
            );
        }
        Ok(outcome)
    }

    /// Only open work is decoded, never settled operation history.
    pub fn open_queue_operations(&self) -> Result<Vec<QueueOperationIntent>> {
        self.records("queue-operations-open")?
            .into_iter()
            .map(|row| {
                crate::work::record(crate::work::Work::InboxRecordRead, 1);
                let intent: QueueOperationIntent =
                    serde_json::from_value(row.doc).context("decoding open queue operation")?;
                check_intent(&row.key, &intent)?;
                Ok(intent)
            })
            .collect()
    }

    fn check_queue_request(&self, request: &QueueOperationRequest) -> Result<()> {
        ensure!(
            !request.reason.trim().is_empty() && request.reason.len() <= 1024,
            "queue reason must be nonblank and at most 1024 bytes"
        );
        ensure!(
            !request.actor.trim().is_empty() && request.actor.len() <= 128,
            "invalid queue actor"
        );
        let members = self.check_execution_scope(&request.scope)?;
        ensure!(
            members == request.members && !members.is_empty(),
            "queue operation must name the whole envelope"
        );
        if let Some(item) = &request.item {
            request.scope.check_item(item)?;
        }
        for seq in &members {
            let message = self.message(*seq)?;
            ensure!(
                message.binding.decision.is_none()
                    && message.binding.question_request.is_none()
                    && self
                        .record::<Effect>("effects", &format!("{seq:020}"))?
                        .is_none(),
                "effect_bound queue message"
            );
            let execution = self.execution(*seq, &request.scope)?;
            ensure!(
                execution.as_ref().map(|record| record.state) == request.expected,
                "queue execution changed before preparation"
            );
            if let Some(execution) = execution {
                ensure!(
                    Some(&execution.item) == request.item.as_ref(),
                    "queue operation item mismatch"
                );
            }
        }
        if let QueueOperationKind::Edit { replacement } = &request.operation {
            ensure!(members.len() == 1, "shared envelope cannot be edited");
            ensure!(
                replacement.text_sha256 == sha256_hex(replacement.body.as_bytes()),
                "replacement digest mismatch"
            );
            let original = self.message(members[0])?;
            self.check(&replacement_post(&original, &request.actor, replacement))?;
        }
        Ok(())
    }

    /// Canonical non-publication result. A lost/malformed reply is NOT a result: leave open.
    /// Successful edit publication can only be recorded by commit_queue_edit.
    pub fn settle_queue_operation(&self, outcome: &QueueOperationOutcome) -> Result<bool> {
        let _lock = self.lock()?;
        self.store.records_transaction(|| {
            ensure!(
                !matches!(
                    outcome.replacement,
                    Some(QueueReplacementOutcome::Published { .. })
                ),
                "use atomic edit publication"
            );
            self.write_queue_outcome(outcome)
        })
    }

    fn write_queue_outcome(&self, outcome: &QueueOperationOutcome) -> Result<bool> {
        ensure!(outcome.version == 1, "unsupported queue outcome version");
        let intent = self
            .queue_operation(&outcome.op_key)?
            .context("queue operation not prepared")?;
        if let Some(prior) = self.queue_operation_outcome(&outcome.op_key)? {
            ensure!(prior == *outcome, "queue operation outcome conflict");
            return Ok(false);
        }
        let editing = matches!(intent.request.operation, QueueOperationKind::Edit { .. });
        let withdrawn = outcome.withdrawal == QueueWithdrawalOutcome::Withdrawn;
        ensure!(
            outcome.replacement.is_some() == (editing && withdrawn),
            "edit outcome must explain replacement after withdrawal only"
        );
        if let Some(QueueReplacementOutcome::Blocked { reason }) = &outcome.replacement {
            ensure!(
                !reason.trim().is_empty() && reason.len() <= 1024,
                "invalid replacement-blocked reason"
            );
        }
        let owner_key = intent.request.scope.key()?;
        ensure!(
            self.record::<String>("queue-operation-owner", &owner_key)?
                .as_deref()
                == Some(&outcome.op_key),
            "queue operation no longer owns envelope"
        );
        self.save_record("queue-operation-outcomes", &outcome.op_key, outcome)?;
        self.delete_record("queue-operations-open", &outcome.op_key)?;
        self.delete_record("queue-operation-owner", &owner_key)?;
        if self.records("queue-operations-open")?.is_empty() {
            self.store
                .delete_record("inbox-queue-operations-open", self.key())?;
        }
        self.event("queue_operation_settled", json!(outcome))?;
        Ok(true)
    }

    /// After projecting canonical withdrawal evidence for THIS operation, publish exactly
    /// one linked tail message and its outcome in the same Store logical unit. Revalidation
    /// (caller, generation, admission) happens in the service and `admit` before commit.
    /// Standalone recipients use a records unit; task recipients use the current head unit.
    /// Caller holds delivery, not inbox/task. There is never external I/O in these callbacks.
    pub fn commit_queue_edit(
        &self,
        op_key: &str,
        admit: impl FnOnce(&mut crate::TaskHeadMut, &Message) -> Result<()>,
    ) -> Result<Message> {
        ensure!(
            !self.store.has_record_unit(),
            "queue edit owns its publication unit"
        );
        let _lock = self.lock()?;
        let intent = self
            .queue_operation(op_key)?
            .context("queue edit not prepared")?;
        let QueueOperationKind::Edit { replacement } = &intent.request.operation else {
            anyhow::bail!("operation is not an edit");
        };
        if let Some(outcome) = self.queue_operation_outcome(op_key)? {
            let Some(QueueReplacementOutcome::Published { message }) = outcome.replacement else {
                anyhow::bail!("queue edit already settled without publication");
            };
            let seq = message
                .strip_prefix(&format!("{}#", self.key()))
                .context("replacement recipient mismatch")?
                .parse()?;
            return self.message(seq);
        }
        let original = self.message(intent.request.members[0])?;
        let execution = self
            .execution(original.seq, &intent.request.scope)?
            .context("queue edit has no canonical withdrawal evidence")?;
        ensure!(
            execution.state == ExecutionState::Withdrawn
                && execution.op_key.as_deref() == Some(op_key),
            "queue edit does not own the proven withdrawal"
        );
        ensure!(
            original.binding.decision.is_none()
                && original.binding.question_request.is_none()
                && self
                    .record::<Effect>("effects", &format!("{:020}", original.seq))?
                    .is_none(),
            "effect_bound queue message"
        );
        let post = replacement_post(&original, &intent.request.actor, replacement);
        self.check(&post)?;
        let finish = |message: Message| -> Result<Message> {
            self.write_queue_outcome(&QueueOperationOutcome {
                version: 1,
                op_key: op_key.into(),
                withdrawal: QueueWithdrawalOutcome::Withdrawn,
                replacement: Some(QueueReplacementOutcome::Published {
                    message: message.id.clone(),
                }),
            })?;
            self.announce(&message)?;
            #[cfg(test)]
            super::header::checkpoint("queue_edit_commit")?;
            Ok(message)
        };
        match self.recipient.task() {
            None => self.store.records_transaction(|| {
                let mut message = self.update_state(|state| self.build(state, post))?;
                message.supersedes = Some(original.id.clone());
                self.write_message(&message)?;
                finish(message)
            }),
            Some(task) => {
                let mut result = None;
                self.store.update_head(task, |head| {
                    let message =
                        self.reserve_linked_in_head(head, post, Some(original.id.clone()))?;
                    admit(head, &message)?;
                    result = Some(finish(message)?);
                    Ok(())
                })?;
                result.context("queue edit publication did not run")
            }
        }
    }
}

fn check_intent(key: &str, intent: &QueueOperationIntent) -> Result<()> {
    ensure!(
        intent.version == 1
            && intent.number > 0
            && !intent.request.members.is_empty()
            && intent.digest == sha256_hex(&serde_json::to_vec(&intent.request)?)
            && intent.op_key == format!("qo-{}", intent.digest)
            && intent.op_key == key,
        "queue intent identity mismatch"
    );
    Ok(())
}

fn replacement_post<'a>(
    original: &'a Message,
    actor: &'a str,
    replacement: &'a QueueReplacement,
) -> Post<'a> {
    // Kind is borrowed from original, whose lifetime must also cover the transaction.
    Post {
        kind: &original.kind,
        urgency: original.urgency,
        author: actor,
        body: &replacement.body,
        binding: original.binding.clone(),
        subject: original.subject.clone(),
        max_payload: replacement.max_payload,
    }
}

#[cfg(test)]
#[path = "queue_operations_tests.rs"]
mod tests;
