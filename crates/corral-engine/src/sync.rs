use std::fs::OpenOptions;

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde_json::json;

use crate::{
    Actor, Blocked, Engine, Op, Outcome, Role, Run, SessionRef, SessionStatus, Status, Store, Task,
    as_refusal,
    events::EventKind,
    review::worker_blocked,
    runner::pi::Activity,
    store::TaskFilter,
    tasks::{next_force_approve, next_retry, set_lifecycle},
};

mod attention;
#[cfg(test)]
mod tests;

struct Observation {
    status: Option<SessionStatus>,
    paused: Option<bool>,
    activity: Option<Activity>,
}

impl Engine {
    pub fn sync(&self) -> Result<()> {
        for run in self.store.live_runs(None)? {
            if let Err(error) = self.reconcile(&run) {
                self.sync_error(&run.task, Some(&run.id), "session reconciliation", &error)?;
            }
        }
        // Resume legacy queued repairs before admitting new work.
        for id in self.store.queued_repairs()? {
            let _ = self.advance(&id);
        }
        let mut occupied = self.store.occupied_slots(None)?;
        for task in self.store.list_tasks(&TaskFilter {
            statuses: vec![Status::Ready],
            ..TaskFilter::default()
        })? {
            if occupied >= self.config.limits.running {
                break;
            }
            let Some(project) = self.project_for(&task)? else {
                continue;
            };
            if !project.auto_dispatch {
                continue;
            }
            match self.dispatch_as(&task.id, None, None, false, Actor::Auto) {
                Ok(_) => occupied += 1,
                Err(error) if as_refusal(&error).is_none() => {
                    self.sync_error(&task.id, None, "automatic dispatch", &error)?;
                }
                Err(_) => {}
            }
        }
        for task in self.store.list_tasks(&TaskFilter {
            statuses: vec![Status::Review],
            ..TaskFilter::default()
        })? {
            // advance records any error and always tries the lock without waiting.
            let _ = self.advance(&task.id);
        }
        Ok(())
    }

    pub(crate) fn reconcile(&self, observed: &Run) -> Result<()> {
        // Serialize sampling through commit across CLI sync, daemon sync and
        // relaunch. An older observation must not win by committing last.
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.state_dir.join("locks/reconcile"))
            .context("open reconciliation lock")?;
        lock.lock().context("lock session reconciliation")?;
        // A send-back, re-review request or verdict may commit while we're
        // outside the store reading the socket. Don't use that snapshot for
        // the new obligation, even if the run/session/round were reused.
        let seq = self.store.task_event_seq(&observed.task)?;
        let state = match &observed.session {
            Some(session @ SessionRef::Pi { .. }) => self
                .session_runner()
                .observe_activity(session)
                .map(|(status, paused, activity)| (Some(status), paused, activity)),
            Some(session) => self
                .runner
                .reconciliation_status(session)
                .map(|status| (status, None, None)),
            None => Ok((Some(SessionStatus::Gone), None, None)),
        };
        let (sample, error) = match state {
            Ok((status, paused, activity)) => (
                Observation {
                    status,
                    paused,
                    activity,
                },
                None,
            ),
            Err(error) => (
                Observation {
                    status: None,
                    paused: None,
                    activity: None,
                },
                Some(error),
            ),
        };
        self.reconcile_sample(observed, seq, sample, Timestamp::now())?;
        error
            .map_or(Ok(()), Err)
            .with_context(|| format!("read session {}:{}", observed.task, observed.id))
    }

    fn reconcile_sample(
        &self,
        observed: &Run,
        seq: i64,
        sample: Observation,
        now: Timestamp,
    ) -> Result<()> {
        self.store.transaction(|store| {
            let Some(mut run) = store.get_run(&observed.task, &observed.id)? else {
                return Ok(());
            };
            if run.ended.is_some()
                || run.session != observed.session
                || store.task_event_seq(&run.task)? != seq
                || store
                    .latest_run(&run.task, run.role)?
                    .is_none_or(|latest| latest.id != run.id)
            {
                return Ok(());
            }
            let mut task = store.require_task(&run.task)?;
            let review_requested = run.role == Role::Reviewer
                && task.status == Status::Review
                && store.has_review_request_since_done(&task.id, &run.id, task.round)?
                && !store.has_verdict_since_done(&task.id, task.round)?;
            self.reconcile_attention(store, &mut run, &mut task, review_requested, &sample, now)?;
            let Some(status) = sample.status else {
                return Ok(());
            };
            if run.role == Role::Reviewer {
                if status == SessionStatus::Gone {
                    let waiting = review_requested;
                    let outcome = if !waiting && run.head_sha.is_some() {
                        Outcome::Done
                    } else {
                        Outcome::Failed
                    };
                    self.end_run(store, &mut run, outcome, false)?;
                    if waiting {
                        let next = vec![
                            crate::Next::run(Op::Review {
                                id: task.id.clone(),
                                profile: Some(run.profile.clone()),
                            }),
                            next_force_approve(&task.id),
                        ];
                        self.block_in(
                            store,
                            &mut task,
                            Some(&run.id),
                            Blocked {
                                reason: crate::session_recovery::LOST_REVIEWER.into(),
                                detail: None,
                                next,
                            },
                            Actor::Auto,
                        )?;
                    }
                }
                return Ok(());
            }
            let waiting_for_reply = task
                .blocked
                .as_ref()
                .and_then(|b| b.next.first())
                .is_some_and(|next| matches!(next.op, Op::Reply { .. }));
            if status == SessionStatus::Gone {
                let outcome = if task.status == Status::Review && run.head_sha.is_some() {
                    Outcome::Done
                } else {
                    Outcome::Failed
                };
                self.end_run(store, &mut run, outcome, false)?;
                if task.status == Status::Running || waiting_for_reply {
                    let next = vec![next_retry(&task.id), next_force_approve(&task.id)];
                    self.block_in(
                        store,
                        &mut task,
                        Some(&run.id),
                        Blocked {
                            reason: crate::session_recovery::LOST_WORKER.into(),
                            detail: None,
                            next,
                        },
                        Actor::Auto,
                    )?;
                }
                return Ok(());
            }
            match status {
                SessionStatus::Blocked { excerpt }
                    if task.status == Status::Running || waiting_for_reply =>
                {
                    let blocked = worker_blocked(&task.id, excerpt);
                    self.block_in(store, &mut task, Some(&run.id), blocked, Actor::Auto)?;
                }
                SessionStatus::Working if waiting_for_reply => {
                    set_lifecycle(&mut task, Status::Running);
                    store.update_task(&task)?;
                    self.event(
                        store,
                        Some(&task.id),
                        Some(&run.id),
                        EventKind::Resumed,
                        Actor::Auto,
                        json!({}),
                    )?;
                }
                _ => (),
            }
            Ok(())
        })
    }

    fn reconcile_attention(
        &self,
        store: &Store,
        run: &mut Run,
        task: &mut Task,
        review_requested: bool,
        sample: &Observation,
        now: Timestamp,
    ) -> Result<()> {
        let pi = matches!(run.session, Some(SessionRef::Pi { .. }));
        let quiet = if pi {
            // Pi can report a diagnostic Blocked status after a terminal error
            // without waiting for anyone. Its native facts govern the obligation.
            sample.paused == Some(false)
                && sample
                    .activity
                    .as_ref()
                    .is_some_and(|a| !a.waiting && !a.busy)
        } else {
            sample.status == Some(SessionStatus::Idle)
        };
        let since = run.quiet_since;
        let quiet_since = quiet.then_some(since.unwrap_or(now));
        if quiet_since != since {
            run.quiet_since = quiet_since;
            store.update_run(run)?;
        }

        // Running workers owe a new submission even when head_sha still names
        // an earlier submission. In Review only the asked reviewer owes work.
        if !matches!(
            (run.role, task.status),
            (Role::Worker, Status::Running) | (Role::Reviewer, Status::Review)
        ) {
            return Ok(());
        }
        let owed = run.role == Role::Worker || review_requested;
        let reason = if !quiet || !owed {
            None
        } else if let Some(activity) = &sample.activity {
            let pickup = store.obligation_pickup(run)?;
            attention::pi_reason(run.role, activity, pickup.as_ref(), since, now)
        } else if attention::elapsed(since, now, self.config.sync.quiet_after_secs) {
            Some("Possibly stalled. Open the session to check progress.")
        } else {
            None
        };
        if task.attention.as_deref() != reason {
            task.attention = reason.map(str::to_owned);
            store.update_task(task)?;
        }
        Ok(())
    }

    fn sync_error(
        &self,
        id: &str,
        run: Option<&str>,
        op: &str,
        error: &anyhow::Error,
    ) -> Result<()> {
        self.store.transaction(|store| {
            self.event(
                store,
                Some(id),
                run,
                EventKind::Error,
                Actor::Auto,
                json!({"operation":op,"reason":format!("{error:#}")}),
            )
        })
    }
}
