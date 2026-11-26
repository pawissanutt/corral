use anyhow::{Context, Result};
use jiff::Timestamp;
use serde_json::json;

use crate::{
    Actor, Blocked, Engine, Op, Outcome, Role, Run, Runner, SessionStatus, Status, as_refusal,
    events::EventKind,
    review::worker_blocked,
    store::TaskFilter,
    tasks::{next_force_approve, next_retry, set_lifecycle},
};

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
        let status = if let Some(session) = &observed.session {
            self.session_runner()
                .status(session)
                .with_context(|| format!("read session {}:{}", observed.task, observed.id))?
        } else {
            SessionStatus::Gone
        };
        self.store.transaction(|store| {
            let Some(mut run) = store.get_run(&observed.task, &observed.id)? else {
                return Ok(());
            };
            if run.ended.is_some() || run.session != observed.session {
                return Ok(());
            }
            let mut task = store.require_task(&run.task)?;
            if run.role == Role::Reviewer {
                if status == SessionStatus::Gone {
                    let waiting = task.status == Status::Review
                        && store.has_review_request_since_done(&task.id, &run.id, task.round)?
                        && !store.has_verdict_since_done(&task.id, task.round)?;
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
            let quiet_since = if status == SessionStatus::Idle {
                Some(run.quiet_since.unwrap_or_else(Timestamp::now))
            } else {
                None
            };
            if run.quiet_since != quiet_since {
                run.quiet_since = quiet_since;
                store.update_run(&run)?;
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
