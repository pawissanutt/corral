use anyhow::{Context, Result};
use serde_json::json;

use crate::{
    Actor, CheckKind, CheckResult, Engine, Next, Op, Outcome, Run, Runner, Status, Store, Task,
    checkout::Checkout,
    events::EventKind,
    pipeline::OperationFailure,
    refuse,
    review::{SendBack, gate_notes, next_done},
    tasks::{next_retry, set_lifecycle},
};

const APPROVE_FAILED: &str = "approve failed";

fn reviewable(task: &Task) -> bool {
    task.status == Status::Review
        || task
            .blocked
            .as_ref()
            .is_some_and(|blocked| blocked.reason == APPROVE_FAILED)
}

impl Engine {
    pub fn approve(&self, id: &str, onto: Option<&str>, force: bool) -> Result<Run> {
        self.with_pipeline_lock(id, || self.approve_as(id, onto, force, self.actor))
    }

    pub(crate) fn approve_as(
        &self,
        id: &str,
        onto: Option<&str>,
        force: bool,
        actor: Actor,
    ) -> Result<Run> {
        self.check_force(force, || approve_op(id, onto))?;
        let task = self.store.require_task(id)?;
        let worker = self.last_worker(&self.store, &task)?;
        let checkout = Checkout::from_run(&worker)?;
        let _lock =
            corral_git::landing_lock(&checkout.repo).context("acquire repository landing lock")?;
        let task = self.store.require_task(id)?;
        let mut worker = self.last_worker(&self.store, &task)?;
        let checkout = Checkout::from_run(&worker)?;
        let target = onto.unwrap_or(&checkout.base);
        let reason = if target == checkout.branch {
            Some(format!(
                "cannot land a task branch onto itself ({target:?})"
            ))
        } else if !corral_git::local_branch_exists(&checkout.repo, target)
            .context("check landing target branch")?
        {
            Some(format!("landing target branch {target:?} does not exist"))
        } else {
            None
        };
        if let Some(reason) = reason {
            let next = corral_git::local_branches(&checkout.repo)?
                .into_iter()
                .filter(|branch| branch != &checkout.branch)
                .map(|branch| Next {
                    op: approve_op(id, Some(&branch)),
                    force,
                })
                .collect();
            return Err(refuse(reason, next));
        }
        let next_force = || Next::force(approve_op(id, onto));
        let mut skipped = Vec::new();
        if !reviewable(&task) {
            if !force {
                let next = if matches!(task.status, Status::Running | Status::Blocked) {
                    next_done(id)
                } else {
                    next_retry(id)
                };
                return Err(refuse(
                    "approve requires a task in Review",
                    vec![next, next_force()],
                ));
            }
            skipped.push("status rule");
        }
        if !checkout.branch_exists()? {
            return Err(refuse(
                "cannot land a missing task branch, even with --force",
                vec![next_retry(id)],
            ));
        }
        let mut effects_started = false;
        let result = (|| -> Result<()> {
            let target_ref = format!("refs/heads/{target}");
            if !checkout.path.is_dir() {
                effects_started = true;
                corral_git::worktree_add(
                    &checkout.repo,
                    &checkout.path,
                    &checkout.branch,
                    &target_ref,
                )
                .context("recreate landing worktree")?;
            }
            checkout.verify_branch()?;
            let target_sha = corral_git::rev_sha(&checkout.repo, &target_ref)
                .context("read landing target SHA")?;
            let source_sha = checkout.merge_base()?;
            effects_started = true;
            match corral_git::rebase_onto(&checkout.path, &source_sha, &target_sha)
                .context("rebase landing worktree")?
            {
                corral_git::RebaseOutcome::Rebased => {
                    if worker.base_sha != target_sha {
                        // Keep the actual fork even if the subsequent gate or merge refuses.
                        self.store.transaction(|store| {
                            let mut current = store
                                .get_run(id, &worker.id)?
                                .context("landing worker run disappeared")?;
                            current.base_sha.clone_from(&target_sha);
                            store.update_run(&current)
                        })?;
                        worker.base_sha.clone_from(&target_sha);
                    }
                }
                corral_git::RebaseOutcome::Conflict { message, .. } => {
                    return self.landing_send_back(
                        &worker,
                        onto,
                        &format!("rebase onto {target} and resolve conflicts\n\n{message}"),
                        force,
                        &skipped,
                        actor,
                    );
                }
            }
            let head = corral_git::rev_sha(&checkout.repo, &checkout.branch_ref())
                .context("read rebased branch HEAD")?;
            let project = self.project_for(&task)?;
            if let Some(project) = project.as_ref().filter(|project| !project.gate.is_empty()) {
                let green = self
                    .store
                    .latest_check(id, CheckKind::Gate, &head)?
                    .is_some_and(|check| check.result == CheckResult::Pass);
                if !green {
                    if force {
                        skipped.push("gate");
                    } else {
                        let check =
                            self.run_gate(&worker, Some(project), actor, &mut effects_started)?;
                        anyhow::ensure!(
                            check.key == head,
                            "checkout HEAD changed before the landing gate"
                        );
                        if check.result == CheckResult::Fail {
                            return self.landing_send_back(
                                &worker,
                                onto,
                                &gate_notes(&check),
                                false,
                                &[],
                                actor,
                            );
                        }
                    }
                }
            }
            let current_base = corral_git::rev_sha(&checkout.repo, &target_ref)
                .context("read landing target before fast-forward")?;
            let current_branch = corral_git::rev_sha(&checkout.repo, &checkout.branch_ref())
                .context("read branch before fast-forward")?;
            anyhow::ensure!(
                head == current_branch
                    && current_base == target_sha
                    && corral_git::is_ancestor(&checkout.repo, &current_base, &head)
                        .context("check that rebased HEAD still contains the base")?,
                "base or task branch changed during landing; run approve again"
            );
            self.store.transaction(|store| {
                let mut task = store.require_task(id)?;
                if !force && !reviewable(&task) {
                    return Err(refuse(
                        "task left Review before landing",
                        vec![next_done(id), next_force()],
                    ));
                }
                if self.last_worker(store, &task)?.id != worker.id {
                    return Err(refuse(
                        "the task acquired a different worker run before landing",
                        vec![next_done(id), next_force()],
                    ));
                }
                corral_git::fast_forward_in_worktree(&checkout.repo, &checkout.branch, target)
                    .context("fast-forward landing target")?;
                self.stop_landing_sessions(store, id, &worker.id, actor)?;
                corral_git::worktree_remove(&checkout.repo, &checkout.path, true)
                    .context("remove landed task worktree")?;
                checkout
                    .delete_branch()
                    .context("delete landed task branch")?;
                worker.head_sha = Some(head);
                self.end_run(store, &mut worker, Outcome::Landed, false)?;
                set_lifecycle(&mut task, Status::Done);
                if actor == Actor::Owner {
                    task.round = 0;
                }
                store.update_task(&task)?;
                self.event(
                    store,
                    Some(id),
                    Some(&worker.id),
                    EventKind::Landed,
                    actor,
                    json!({"sha":worker.head_sha,"base":target}),
                )?;
                self.record_forced_approve(store, &task, &worker.id, force, &skipped, actor)
            })
        })();
        if let Err(error) = result {
            if !effects_started {
                return Err(error);
            }
            let recorded = self.store.transaction(|store| {
                let mut current = store.require_task(id)?;
                if current.status != task.status
                    || self.last_worker(store, &current)?.id != worker.id
                {
                    return Ok(false);
                }
                self.record_operation_failure(
                    store,
                    &mut current,
                    Some(&worker.id),
                    OperationFailure {
                        operation: "approve",
                        reason: APPROVE_FAILED,
                        error: &error,
                        next: vec![Next::run(approve_op(id, onto)), next_force()],
                    },
                    actor,
                )?;
                self.record_forced_approve(store, &current, &worker.id, force, &skipped, actor)?;
                Ok(true)
            })?;
            if !recorded {
                return Err(error);
            }
        }
        Ok(worker)
    }

    fn stop_landing_sessions(
        &self,
        store: &Store,
        task: &str,
        landing: &str,
        actor: Actor,
    ) -> Result<()> {
        for mut run in store.live_runs_for_task(task)? {
            let stop = self.live_session(&run)?.is_some();
            if landing == run.id {
                if stop && let Some(session) = &run.session {
                    self.session_runner()
                        .stop(session)
                        .context("stop landed worker session")?;
                }
            } else {
                self.end_run(store, &mut run, Outcome::Stopped, stop)?;
            }
            if stop {
                self.event(
                    store,
                    Some(task),
                    Some(&run.id),
                    EventKind::Stopped,
                    actor,
                    json!({"reason":"landing"}),
                )?;
            }
        }
        Ok(())
    }

    fn landing_send_back(
        &self,
        worker: &Run,
        onto: Option<&str>,
        notes: &str,
        force: bool,
        skipped: &[&str],
        actor: Actor,
    ) -> Result<()> {
        self.launch_transaction(|store, pending| {
            let mut task = store.require_task(&worker.task)?;
            if force || reviewable(&task) {
                self.send_back_in(
                    store,
                    pending,
                    &mut task,
                    notes,
                    SendBack::Automatic { onto },
                )?;
                self.record_forced_approve(store, &task, &worker.id, force, skipped, actor)?;
            }
            Ok(())
        })
    }

    fn record_forced_approve(
        &self,
        store: &Store,
        task: &Task,
        run: &str,
        force: bool,
        skipped: &[&str],
        actor: Actor,
    ) -> Result<()> {
        if force {
            self.event(
                store,
                Some(&task.id),
                Some(run),
                EventKind::Forced,
                actor,
                json!({"operation":"approve","skipped":skipped}),
            )?;
        }
        Ok(())
    }
}

fn approve_op(id: &str, onto: Option<&str>) -> Op {
    Op::Approve {
        id: id.to_owned(),
        onto: onto.map(str::to_owned),
    }
}
