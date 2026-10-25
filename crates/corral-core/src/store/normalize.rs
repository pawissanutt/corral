use anyhow::{Result, bail};

use super::Store;
use crate::{AttentionLedger, Status, Task};

pub(super) fn task(task: &mut Task) -> Result<()> {
    task.attention.validate()?;
    if task.status != Status::Review {
        task.infra_wait = None;
    }
    Ok(())
}

impl Store {
    /// Mutate only attention under the usual board/task locks, preserving `Task.updated`.
    /// A no-op does not rewrite the task. The callback must not call store writers.
    ///
    /// # Errors
    /// Returns callback, accounting-backpressure, locking or persistence errors.
    pub fn update_attention(
        &self,
        id: &str,
        update: impl FnOnce(&mut AttentionLedger) -> Result<()>,
    ) -> Result<Task> {
        if self.frozen.is_some() {
            bail!("a read-only task snapshot cannot update attention");
        }
        self.check_runtime()?;
        let _board = self.lock("dispatch")?;
        let id = Self::normalize_id(id)?;
        let _task = self.lock(&id)?;
        let mut record = self.task_record(&id)?;
        let previous = record.task.attention.clone();
        update(&mut record.task.attention)?;
        record.task.attention.validate()?;
        if record.task.attention != previous {
            crate::schema::persist_record(&record, crate::schema::store_version(self)?)?;
        }
        Ok(record.task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        InfraWait, InfraWaitClass, NoticeClaim, NoticeCoverage, NoticeFact, NoticeOutcome,
    };
    use jiff::Timestamp;
    use serde_json::json;

    fn wait() -> InfraWait {
        InfraWait {
            version: 1,
            class: InfraWaitClass::Contention,
            marker: "index.lock".into(),
            path: "/repo/.git/index.lock".into(),
            first_seen: Timestamp::from_second(1).unwrap(),
            first_mtime: Some(Timestamp::from_second(0).unwrap()),
            held_at: Some(Timestamp::from_second(301).unwrap()),
        }
    }

    fn notice() -> NoticeClaim {
        NoticeClaim {
            version: 1,
            batch_id: "infra/T-001/R1/1".into(),
            producer: "infra_hold".into(),
            source: "T-001/R1/1".into(),
            claimed_at: Timestamp::from_second(301).unwrap(),
            claim_ids: vec!["infra/T-001/R1/1".into()],
            coverage: vec![NoticeCoverage::Exact {
                condition_key: "T-001/infra_hold/1".into(),
            }],
            exemption_basis: None,
            outcome: None,
            pending: vec![NoticeFact {
                fact_id: "infra/T-001/R1/1/claimed".into(),
                kind: "notification_claimed".into(),
                detail: json!({"batch_id": "infra/T-001/R1/1"}),
            }],
        }
    }

    #[test]
    fn save_clears_infra_wait_on_every_non_review_status() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let task = store.create("Infra", None, Status::Review, "spec").unwrap();
        for status in [
            Status::Idea,
            Status::Backlog,
            Status::Ready,
            Status::Running,
            Status::NeedsMe,
            Status::Done,
            Status::Dropped,
        ] {
            let review = store
                .update(&task.id, |task| {
                    task.status = Status::Review;
                    task.infra_wait = Some(wait());
                    if task.attention.notice_outbox.is_empty() {
                        task.attention.push_notice(notice())?;
                    }
                    Ok(())
                })
                .unwrap();
            assert_eq!(review.infra_wait, Some(wait()));
            let changed = store
                .update(&task.id, |task| {
                    task.status = status;
                    Ok(())
                })
                .unwrap();
            assert!(changed.infra_wait.is_none(), "{status:?}");
            assert_eq!(changed.attention.notice_outbox, vec![notice()]);
            assert!(store.get(&task.id).unwrap().infra_wait.is_none());
        }
    }

    #[test]
    fn attention_writes_preserve_every_other_field_and_replay_after_source_clear() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let task = store
            .create_initialized("Infra", None, Status::Review, "spec", |task| {
                task.infra_wait = Some(wait());
                task.attention.push_notice(notice())
            })
            .unwrap();
        let task = store
            .update(&task.id, |task| {
                task.status = Status::Done;
                Ok(())
            })
            .unwrap();
        let claimed = store
            .update_attention(&task.id, |ledger| {
                ledger.notice_outbox[0].outcome = Some(NoticeOutcome::Notified {
                    at: Timestamp::from_second(302).unwrap(),
                });
                ledger.notice_outbox[0].pending.push(NoticeFact {
                    fact_id: "infra/T-001/R1/1/outcome".into(),
                    kind: "owner_notified".into(),
                    detail: json!({"batch_id": "infra/T-001/R1/1"}),
                });
                Ok(())
            })
            .unwrap();
        let mut expected = task.clone();
        expected.attention = claimed.attention.clone();
        assert_eq!(claimed, expected);
        let reopened = Store::open(root.path()).unwrap();
        assert_eq!(reopened.get(&task.id).unwrap(), claimed);
        let drained = reopened
            .update_attention(&task.id, |ledger| {
                ledger.notice_outbox[0].pending.clear();
                Ok(())
            })
            .unwrap();
        assert_eq!(drained.updated, task.updated);
        assert!(drained.attention.notice_outbox[0].outcome.is_some());
        assert_eq!(
            drained.attention.notice_outbox[0].coverage,
            notice().coverage
        );
    }

    #[test]
    fn invalid_or_failed_attention_update_does_not_publish() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let task = store.create("Infra", None, Status::Ready, "").unwrap();
        let result = store.update_attention(&task.id, |ledger| {
            ledger.notice_outbox = vec![notice(); 33];
            Ok(())
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("notification_accounting_backpressure")
        );
        assert_eq!(store.get(&task.id).unwrap(), task);
        let result = store.update_attention(&task.id, |ledger| {
            ledger.push_notice(notice())?;
            anyhow::bail!("callback failed")
        });
        assert!(result.is_err());
        assert_eq!(store.get(&task.id).unwrap(), task);
        let no_op = store.update_attention(&task.id, |_| Ok(())).unwrap();
        assert_eq!(no_op, task);
    }
}
