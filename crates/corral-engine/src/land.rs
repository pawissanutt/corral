use anyhow::{Context, Result};
use serde_json::json;

use crate::{
    Actor, Blocked, CheckKind, CheckResult, Engine, Op, Outcome, Run, Status, Store, Task,
    checkout::Checkout,
    events::EventKind,
    refuse,
    review::{gate_notes, next_done},
    tasks::{next_changes, next_force_approve, next_retry, set_lifecycle},
};

impl Engine {
    pub fn approve(&self, id: &str, force: bool) -> Result<Run> {
        self.with_pipeline_lock(id, || self.approve_as(id, force, self.actor))
    }

    pub(crate) fn approve_as(&self, id: &str, force: bool, actor: Actor) -> Result<Run> {
        self.check_force(force, || Op::Approve { id: id.into() })?;
        let task = self.store.require_task(id)?;
        let worker = self.last_worker(&self.store, &task)?;
        let checkout = Checkout::from_run(&worker)?;
        let _lock =
            corral_git::landing_lock(&checkout.repo).context("acquire repository landing lock")?;
        let task = self.store.require_task(id)?;
        let mut worker = self.last_worker(&self.store, &task)?;
        let checkout = Checkout::from_run(&worker)?;
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
                    vec![next, next_force_approve(id)],
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
        if !checkout.path.is_dir() {
            let result = corral_git::worktree_add(
                &checkout.repo,
                &checkout.path,
                &checkout.branch,
                &checkout.base_ref(),
            );
            if let Err(error) = result {
                self.block_landing(id, &worker.id, &error, force, &skipped, actor)?;
                return Ok(worker);
            }
        }
        if let Err(error) = checkout.verify_branch() {
            self.block_landing(id, &worker.id, &error, force, &skipped, actor)?;
            return Ok(worker);
        }
        match corral_git::rebase(&checkout.path, &checkout.base_ref()) {
            Ok(corral_git::RebaseOutcome::Rebased) => {}
            Ok(corral_git::RebaseOutcome::Conflict { message, .. }) => {
                self.landing_send_back(
                    id,
                    &worker.id,
                    &format!(
                        "rebase onto {} and resolve conflicts\n\n{message}",
                        checkout.base
                    ),
                    force,
                    &skipped,
                    actor,
                )?;
                return Ok(worker);
            }
            Err(error) => {
                self.block_landing(id, &worker.id, &error, force, &skipped, actor)?;
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
                        self.block_landing(id, &worker.id, &error, false, &[], actor)?;
                        return Ok(worker);
                    }
                    if check.result == CheckResult::Fail {
                        self.landing_send_back(
                            id,
                            &worker.id,
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
        let current_base = corral_git::rev_sha(&checkout.repo, &checkout.base_ref())
            .context("read base before fast-forward")?;
        let current_branch = corral_git::rev_sha(&checkout.repo, &checkout.branch_ref())
            .context("read branch before fast-forward")?;
        if head != current_branch
            || !corral_git::is_ancestor(&checkout.repo, &current_base, &head)
                .context("check that rebased HEAD still contains the base")?
        {
            let error =
                anyhow::anyhow!("base or task branch changed during landing; run approve again");
            self.block_landing(id, &worker.id, &error, force, &skipped, actor)?;
            return Ok(worker);
        }
        self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            if !force && task.status != Status::Review {
                return Err(refuse(
                    "task left Review before landing",
                    vec![next_done(id), next_force_approve(id)],
                ));
            }
            if self.last_worker(store, &task)?.id != worker.id {
                return Err(refuse(
                    "the task acquired a different worker run before landing",
                    vec![next_done(id), next_force_approve(id)],
                ));
            }
            let landing =
                corral_git::fast_forward(&checkout.repo, &checkout.branch, &checkout.base)
                    .and_then(|()| {
                        self.stop_landing_sessions(store, id, &worker.id, actor)?;
                        corral_git::worktree_remove(&checkout.repo, &checkout.path, true)
                            .context("remove landed task worktree")?;
                        checkout
                            .delete_branch()
                            .context("delete landed task branch")
                    });
            if let Err(error) = landing {
                self.block_landing_in(store, &mut task, &worker.id, &format!("{error:#}"), actor)?;
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
                json!({"sha":worker.head_sha,"base":worker.base}),
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
                    self.runner
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
        id: &str,
        run: &str,
        notes: &str,
        force: bool,
        skipped: &[&str],
        actor: Actor,
    ) -> Result<()> {
        self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            if force || task.status == Status::Review {
                self.send_back_in(store, pending, &mut task, notes, Actor::Auto, false)?;
                self.record_forced_approve(store, &task, run, force, skipped, actor)?;
            }
            Ok(())
        })
    }

    fn block_landing(
        &self,
        id: &str,
        run: &str,
        error: &anyhow::Error,
        force: bool,
        skipped: &[&str],
        actor: Actor,
    ) -> Result<()> {
        self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            self.block_landing_in(store, &mut task, run, &format!("{error:#}"), actor)?;
            self.record_forced_approve(store, &task, run, force, skipped, actor)
        })
    }

    fn block_landing_in(
        &self,
        store: &Store,
        task: &mut Task,
        run: &str,
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
                    next_force_approve(&task.id),
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
