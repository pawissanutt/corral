use anyhow::{Context, Result};
use serde_json::json;

use crate::{
    Actor, Blocked, CheckKind, CheckResult, Engine, Next, Op, Outcome, Run, Runner, Status, Store,
    Task,
    checkout::Checkout,
    events::EventKind,
    refuse,
    review::{SendBack, gate_notes, next_done},
    tasks::{next_changes, next_retry, set_lifecycle},
};

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
        if task.status != Status::Review {
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
        let target_ref = format!("refs/heads/{target}");
        if !checkout.path.is_dir() {
            let result = corral_git::worktree_add(
                &checkout.repo,
                &checkout.path,
                &checkout.branch,
                &target_ref,
            );
            if let Err(error) = result {
                self.block_landing(&worker, onto, &error, force, &skipped, actor)?;
                return Ok(worker);
            }
        }
        if let Err(error) = checkout.verify_branch() {
            self.block_landing(&worker, onto, &error, force, &skipped, actor)?;
            return Ok(worker);
        }
        let target_sha =
            corral_git::rev_sha(&checkout.repo, &target_ref).context("read landing target SHA")?;
        let source_sha = checkout.merge_base()?;
        match corral_git::rebase_onto(&checkout.path, &source_sha, &target_sha) {
            Ok(corral_git::RebaseOutcome::Rebased) => {
                if worker.base_sha != target_sha {
                    // Remember the actual fork after a retarget, even if the
                    // landing gate or the fast-forward later refuses.
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
            Ok(corral_git::RebaseOutcome::Conflict { message, .. }) => {
                self.landing_send_back(
                    &worker,
                    onto,
                    &format!("rebase onto {target} and resolve conflicts\n\n{message}"),
                    force,
                    &skipped,
                    actor,
                )?;
                return Ok(worker);
            }
            Err(error) => {
                self.block_landing(&worker, onto, &error, force, &skipped, actor)?;
                return Ok(worker);
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
                    let check = self.run_gate(&worker, Some(project), actor)?;
                    if check.key != head {
                        let error =
                            anyhow::anyhow!("checkout HEAD changed before the landing gate");
                        self.block_landing(&worker, onto, &error, false, &[], actor)?;
                        return Ok(worker);
                    }
                    if check.result == CheckResult::Fail {
                        self.landing_send_back(
                            &worker,
                            onto,
                            &gate_notes(&check),
                            false,
                            &[],
                            actor,
                        )?;
                        return Ok(worker);
                    }
                }
            }
        }
        let current_base = corral_git::rev_sha(&checkout.repo, &target_ref)
            .context("read landing target before fast-forward")?;
        let current_branch = corral_git::rev_sha(&checkout.repo, &checkout.branch_ref())
            .context("read branch before fast-forward")?;
        if head != current_branch
            || current_base != target_sha
            || !corral_git::is_ancestor(&checkout.repo, &current_base, &head)
                .context("check that rebased HEAD still contains the base")?
        {
            let error =
                anyhow::anyhow!("base or task branch changed during landing; run approve again");
            self.block_landing(&worker, onto, &error, force, &skipped, actor)?;
            return Ok(worker);
        }
        self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            if !force && task.status != Status::Review {
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
            let landing =
                corral_git::fast_forward_in_worktree(&checkout.repo, &checkout.branch, target)
                    .and_then(|()| {
                        self.stop_landing_sessions(store, id, &worker.id, actor)?;
                        corral_git::worktree_remove(&checkout.repo, &checkout.path, true)
                            .context("remove landed task worktree")?;
                        checkout
                            .delete_branch()
                            .context("delete landed task branch")
                    });
            if let Err(error) = landing {
                self.block_landing_in(
                    store,
                    &mut task,
                    &worker.id,
                    onto,
                    &format!("{error:#}"),
                    actor,
                )?;
                self.record_forced_approve(store, &task, &worker.id, force, &skipped, actor)?;
                return Ok(());
            }
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
            self.record_forced_approve(store, &task, &worker.id, force, &skipped, actor)?;
            Ok(())
        })?;
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
            if force || task.status == Status::Review {
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

    fn block_landing(
        &self,
        worker: &Run,
        onto: Option<&str>,
        error: &anyhow::Error,
        force: bool,
        skipped: &[&str],
        actor: Actor,
    ) -> Result<()> {
        self.store.transaction(|store| {
            let mut task = store.require_task(&worker.task)?;
            self.block_landing_in(
                store,
                &mut task,
                &worker.id,
                onto,
                &format!("{error:#}"),
                actor,
            )?;
            self.record_forced_approve(store, &task, &worker.id, force, skipped, actor)
        })
    }

    fn block_landing_in(
        &self,
        store: &Store,
        task: &mut Task,
        run: &str,
        onto: Option<&str>,
        error: &str,
        actor: Actor,
    ) -> Result<()> {
        self.block_in(
            store,
            task,
            Some(run),
            Blocked {
                reason: "git refused landing".into(),
                detail: Some(error.into()),
                next: vec![
                    Next::force(approve_op(&task.id, onto)),
                    next_changes(&task.id),
                    next_retry(&task.id),
                ],
            },
            actor,
        )
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
