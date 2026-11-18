use std::{
    fmt::Write as _,
    fs::{self, File},
    io::Write,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde_json::json;

use crate::{
    Actor, Blocked, Check, CheckKind, CheckResult, Engine, Next, Op, Origin, Outcome, Role, Run,
    Status, Store, Task,
    artifacts::ArtifactTarget,
    as_refusal,
    checkout::Checkout,
    config::EffectiveProject,
    dispatch::{LaunchRequest, PendingLaunch},
    events::EventKind,
    prompt::rereview_request,
    refuse,
    tasks::{next_changes, next_force_approve, next_reply, next_retry, set_lifecycle},
};

impl Engine {
    pub fn done(&self, id: &str, summary: &str) -> Result<Option<Run>> {
        let (run, submitted) =
            self.store.transaction(|store| {
                let mut task = store.require_task(id)?;
                if task.status == Status::Review {
                    return Ok((store.latest_run(id, Role::Worker)?, false));
                }
                let mut run = self.last_worker(store, &task)?;
                if !matches!(task.status, Status::Running | Status::Blocked) {
                    return Err(refuse(
                        "done requires a Running or Blocked task",
                        vec![next_retry(id), next_force_approve(id)],
                    ));
                }
                self.require_worktree(&run)?;
                let checkout = Checkout::from_run(&run)?;
                checkout.verify_branch()?;
                corral_git::commit_all(&run.worktree, &format!("{id} {}", task.title))
                    .context("commit task changes")?;
                let head =
                    corral_git::rev_sha(&run.worktree, "HEAD").context("read submitted HEAD")?;
                run.base_sha = checkout.merge_base()?;
                let diff = corral_git::diff_text(&run.worktree, &run.base_sha)
                    .context("read submitted diff")?;
                let target = || ArtifactTarget {
                    task: id,
                    run: Some(&run.id),
                    actor: self.actor,
                };
                self.add_artifact_in(store, target(), diff.as_bytes(), Some("diff"), "diff.patch")?;
                self.add_artifact_in(
                    store,
                    target(),
                    summary.as_bytes(),
                    Some("summary"),
                    "summary.md",
                )?;
                for path in self.scan_artifacts(&run.worktree)? {
                    let result = self.add_artifact_file_in(
                        store,
                        target(),
                        &path,
                        None,
                        None,
                        Some(self.config.artifacts.max_bytes),
                    );
                    if let Err(error) = result {
                        let Some(refusal) = as_refusal(&error) else {
                            return Err(error);
                        };
                        self.event(store, Some(id), Some(&run.id), EventKind::Error, self.actor,
                        json!({"operation":"artifact scan","path":path,"reason":refusal.reason}))?;
                    }
                }
                run.head_sha = Some(head);
                run.summary = Some(summary.to_owned());
                run.quiet_since = None;
                store.update_run(&run)?;
                set_lifecycle(&mut task, Status::Review);
                store.update_task(&task)?;
                self.event(
                    store,
                    Some(id),
                    Some(&run.id),
                    EventKind::Done,
                    self.actor,
                    json!({"sha":run.head_sha,"summary":summary}),
                )?;
                Ok((Some(run), true))
            })?;
        if submitted {
            self.kick_pipeline(id)?;
        }
        Ok(run)
    }

    pub fn gate(&self, id: &str) -> Result<Check> {
        let check = self.with_pipeline_lock(id, || {
            let task = self.store.require_task(id)?;
            let run = self.last_worker(&self.store, &task)?;
            let project = self.project_for(&task)?;
            self.run_gate(&run, project.as_ref(), self.actor)
        })?;
        self.kick_pipeline(id)?;
        Ok(check)
    }

    pub(crate) fn run_gate(
        &self,
        run: &Run,
        project: Option<&EffectiveProject>,
        actor: Actor,
    ) -> Result<Check> {
        self.require_worktree(run)?;
        let head = corral_git::rev_sha(&run.worktree, "HEAD").context("read gate HEAD")?;
        let directory = self.state_dir.join("runs").join(&run.task);
        fs::create_dir_all(&directory).context("create gate log directory")?;
        let prefix = head.get(..8).context("gate HEAD is not a commit SHA")?;
        let log_path = directory.join(format!("gate-{prefix}.log"));
        let mut log = File::create(&log_path)
            .with_context(|| format!("create gate log {}", log_path.display()))?;
        let mut notes = None;
        if let Some(project) = project {
            for line in &project.gate {
                writeln!(log, "$ {line}").context("write gate command to log")?;
                let status = Command::new("sh")
                    .args(["-c", line])
                    .current_dir(&run.worktree)
                    .envs(&project.env)
                    .stdin(Stdio::null())
                    .stdout(log.try_clone().context("clone gate stdout log")?)
                    .stderr(log.try_clone().context("clone gate stderr log")?)
                    .status()
                    .with_context(|| format!("run gate line {line:?}"))?;
                if !status.success() {
                    writeln!(log, "\n{status}").context("write gate failure to log")?;
                    notes = Some(line.clone());
                    break;
                }
            }
        }
        drop(log);
        ensure!(
            corral_git::rev_sha(&run.worktree, "HEAD").context("read HEAD after gate")? == head,
            "HEAD changed while gate was running; run corral gate {} again",
            run.task
        );
        self.store.transaction(|store| {
            let artifact = self.add_artifact_file_in(
                store,
                ArtifactTarget {
                    task: &run.task,
                    run: Some(&run.id),
                    actor,
                },
                &log_path,
                Some("gate"),
                None,
                None,
            )?;
            let check = Check {
                task: run.task.clone(),
                kind: CheckKind::Gate,
                key: head,
                result: if notes.is_none() {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail
                },
                notes,
                log: Some(artifact.reference()),
                by: actor,
                at: Timestamp::now(),
            };
            store.insert_check(&check)?;
            self.event(
                store,
                Some(&run.task),
                Some(&run.id),
                EventKind::Gate,
                actor,
                serde_json::to_value(&check).context("encode gate event")?,
            )?;
            Ok(check)
        })
    }

    pub fn review(&self, id: &str, profile: Option<&str>) -> Result<Run> {
        self.review_as(id, profile, self.actor)
    }

    pub(crate) fn review_as(&self, id: &str, profile: Option<&str>, actor: Actor) -> Result<Run> {
        self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            let recovery = task
                .blocked
                .as_ref()
                .and_then(|b| b.next.first())
                .is_some_and(|next| matches!(next.op, Op::Review { .. }));
            let worker = self.last_worker(store, &task)?;
            if task.status != Status::Review && !recovery {
                return Err(refuse(
                    "review requires a submitted task",
                    vec![next_done(id), next_force_approve(id)],
                ));
            }
            let project = self.project_for(&task)?;
            let name = profile.or(project
                .as_ref()
                .and_then(|project| project.reviewer.as_deref()));
            let profile = name
                .map_or_else(
                    || {
                        Err(refuse(
                            "no reviewer profile resolves; set --profile",
                            vec![],
                        ))
                    },
                    |name| self.config.require_profile(name),
                )
                .map_err(|error| {
                    self.profile_choices(
                        error,
                        |name| Op::Review {
                            id: id.into(),
                            profile: Some(name),
                        },
                        false,
                    )
                })?;
            if let Some(mut reviewer) = store.latest_run(id, Role::Reviewer)?
                && reviewer.ended.is_none()
            {
                let session = self.live_session(&reviewer)?;
                if let Some(session) = session {
                    if store.has_review_request_since_done(id, &reviewer.id, task.round)? {
                        return Ok(reviewer);
                    }
                    let prompt = self
                        .state_dir
                        .join("runs")
                        .join(id)
                        .join(&reviewer.id)
                        .join("prompt.md");
                    let request =
                        rereview_request(&crate::binary_path()?, &task, &reviewer, &prompt)?;
                    match self.runner.send(session, &request, Origin::Auto) {
                        Ok(()) => {
                            self.record_review_request(
                                store, &mut task, &reviewer, actor, recovery,
                            )?;
                            return Ok(reviewer);
                        }
                        Err(error) if as_refusal(&error).is_some() => {}
                        Err(error) => return Err(error).context("request re-review"),
                    }
                }
                let stop = session.is_some();
                let outcome = if stop {
                    Outcome::Superseded
                } else if reviewer.head_sha.is_some() {
                    Outcome::Done
                } else {
                    Outcome::Failed
                };
                self.end_run(store, &mut reviewer, outcome, stop)?;
            }
            self.require_worktree(&worker)?;
            let checkout = Checkout::from_run(&worker)?;
            let history = review_history(store, id)?;
            let run = self.start_run(
                store,
                pending,
                LaunchRequest {
                    task: &task,
                    checkout: &checkout,
                    role: Role::Reviewer,
                    profile: &profile,
                    note: history.as_deref(),
                    project: project.as_ref(),
                },
            )?;
            self.record_review_request(store, &mut task, &run, actor, recovery)?;
            Ok(run)
        })
    }

    fn record_review_request(
        &self,
        store: &Store,
        task: &mut Task,
        run: &Run,
        actor: Actor,
        recovery: bool,
    ) -> Result<()> {
        if recovery {
            set_lifecycle(task, Status::Review);
            store.update_task(task)?;
        }
        self.event(
            store,
            Some(&task.id),
            Some(&run.id),
            EventKind::ReviewStarted,
            actor,
            json!({"profile":run.profile,"round":task.round}),
        )
    }

    pub fn verdict(&self, id: &str, approve: bool, notes: Option<&str>) -> Result<Option<Run>> {
        let (run, close) = self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            if task.status != Status::Review {
                return Err(refuse(
                    "verdict requires a task in Review",
                    vec![next_done(id), next_force_approve(id)],
                ));
            }
            let mut reviewer = store.latest_run(id, Role::Reviewer)?;
            let mut close = false;
            if let Some(run) = &mut reviewer
                && run.ended.is_none()
            {
                run.head_sha = store
                    .latest_run(id, Role::Worker)?
                    .and_then(|worker| worker.head_sha);
                if approve {
                    self.end_run(store, run, Outcome::Done, false)?;
                    close = true;
                } else {
                    store.update_run(run)?;
                }
            }
            let check = Check {
                task: id.into(),
                kind: CheckKind::Review,
                key: task.round.to_string(),
                result: if approve {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail
                },
                notes: notes.map(str::to_owned),
                log: None,
                by: self.actor,
                at: Timestamp::now(),
            };
            store.insert_check(&check)?;
            let review_run = reviewer.as_ref().map(|run| run.id.as_str());
            self.event(
                store,
                Some(id),
                review_run,
                EventKind::Verdict,
                self.actor,
                json!({"approve":approve,"notes":notes,"round":task.round}),
            )?;
            if !approve {
                self.send_back_in(
                    store,
                    pending,
                    &mut task,
                    notes.unwrap_or("Reviewer requested changes."),
                    Actor::Auto,
                    false,
                )?;
            }
            Ok((reviewer, close))
        })?;
        // A reviewer can call verdict from the very session this closes. Publish
        // the verdict and start the next step before touching its process or pane.
        let advanced = self.kick_pipeline(id);
        let stopped =
            if close && let Some(session) = run.as_ref().and_then(|run| run.session.as_ref()) {
                self.runner
                    .stop(session)
                    .context("close reviewer after recording verdict")
            } else {
                Ok(())
            };
        advanced.and(stopped)?;
        Ok(run)
    }

    pub fn changes(&self, id: &str, text: &str) -> Result<Option<Run>> {
        self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            if !matches!(task.status, Status::Review | Status::Blocked) {
                return Err(refuse(
                    "changes requires a task in Review or Blocked",
                    vec![next_done(id), next_retry(id)],
                ));
            }
            self.send_back_in(store, pending, &mut task, text, self.actor, true)
        })
    }

    pub(crate) fn send_back_in(
        &self,
        store: &Store,
        pending: &mut PendingLaunch,
        task: &mut Task,
        notes: &str,
        actor: Actor,
        reset: bool,
    ) -> Result<Option<Run>> {
        let mut worker = self.last_worker(store, task)?;
        let project = self.project_for(task)?;
        let max_rounds = project
            .as_ref()
            .map_or(self.config.defaults.max_rounds, |p| p.max_rounds);
        if !reset && task.round >= max_rounds {
            let next = vec![
                next_changes(&task.id),
                next_retry(&task.id),
                next_force_approve(&task.id),
            ];
            self.block_in(
                store,
                task,
                Some(&worker.id),
                Blocked {
                    reason: "automatic change rounds exhausted".into(),
                    detail: Some(notes.into()),
                    next,
                },
                actor,
            )?;
            return Ok(None);
        }
        let origin = if reset && actor != Actor::Auto {
            Origin::Owner
        } else {
            Origin::Auto
        };
        let live_session = self.live_session(&worker)?;
        let delivered = if let Some(session) = live_session {
            match self.runner.send(session, notes, origin) {
                Ok(()) => true,
                Err(error) if as_refusal(&error).is_some() => false,
                Err(error) => return Err(error).context("send work back to worker"),
            }
        } else {
            false
        };
        if delivered {
            if worker.quiet_since.take().is_some() {
                store.update_run(&worker)?;
            }
        } else {
            self.require_worktree(&worker)?;
            let mut checkout = Checkout::from_run(&worker)?;
            checkout.base_sha = checkout.merge_base().context("find send-back base")?;
            if worker.ended.is_none() {
                let stop = live_session.is_some();
                self.end_run(store, &mut worker, Outcome::Superseded, stop)?;
            }
            worker = self.start_run(
                store,
                pending,
                LaunchRequest {
                    task,
                    checkout: &checkout,
                    role: Role::Worker,
                    profile: &worker.profile,
                    note: Some(notes),
                    project: project.as_ref(),
                },
            )?;
        }
        task.round = if reset { 0 } else { task.round + 1 };
        set_lifecycle(task, Status::Running);
        store.update_task(task)?;
        self.event(
            store,
            Some(&task.id),
            Some(&worker.id),
            EventKind::SentBack,
            actor,
            json!({"notes":notes,"round":task.round}),
        )?;
        Ok(Some(worker))
    }

    pub(crate) fn require_worktree(&self, run: &Run) -> Result<()> {
        if !run.worktree.is_dir() {
            return Err(refuse("task has no worktree", vec![next_retry(&run.task)]));
        }
        Ok(())
    }

    pub(crate) fn send_back(&self, id: &str, notes: &str) -> Result<()> {
        self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            if task.status == Status::Review {
                self.send_back_in(store, pending, &mut task, notes, Actor::Auto, false)?;
            }
            Ok(())
        })
    }
}

fn review_history(store: &Store, id: &str) -> Result<Option<String>> {
    let mut history = None;
    for check in store.list_review_checks(id)? {
        if let Some(notes) = check.notes {
            let history =
                history.get_or_insert_with(|| String::from("Earlier verdicts in recording order:"));
            let verdict = if check.result == CheckResult::Pass {
                "approve"
            } else {
                "changes"
            };
            write!(
                history,
                "\n\n### Round {} ({verdict})\n\n{notes}",
                check.key
            )?;
        }
    }
    Ok(history)
}

pub(crate) fn next_done(id: &str) -> Next {
    Next::run(Op::Done {
        id: id.into(),
        summary: String::new(),
    })
}

pub(crate) fn gate_notes(check: &Check) -> String {
    let line = check.notes.as_deref().unwrap_or("unknown gate line");
    let log = check.log.as_deref().unwrap_or("unavailable");
    format!("Gate failed: {line}\nGate log: {log}")
}

pub(crate) fn worker_blocked(id: &str, excerpt: String) -> Blocked {
    Blocked {
        reason: "worker needs a reply".into(),
        detail: Some(excerpt),
        next: vec![next_reply(id)],
    }
}
