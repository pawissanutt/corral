use std::fmt::Write as _;

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde_json::json;

use crate::{
    Actor, ArtifactKind, Blocked, Check, CheckKind, CheckResult, Engine, Next, Op, Origin, Outcome,
    Role, Run, Runner, Status, Store, Task,
    artifacts::{ArtifactOwner, ArtifactTarget},
    as_refusal,
    checkout::Checkout,
    dispatch::{LaunchRequest, PendingLaunch},
    events::EventKind,
    pipeline::{GATE_FAILED, OperationFailure, PIPELINE_FAILED},
    prompt::{PromptReferences, rereview_request},
    refuse,
    review_verdict::{Decision, is_pi_reviewer, stale_request},
    runner::pi::Pickup,
    tasks::{next_changes, next_force_approve, next_reply, next_retry, set_lifecycle},
};

pub(crate) enum SendBack<'a> {
    Requested,
    Automatic { onto: Option<&'a str> },
}

impl Engine {
    pub fn done(&self, id: &str, summary: &str) -> Result<Option<Run>> {
        let _writer = self.checkout_writer(id)?;
        let mut _exact_writer = None;
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
                _exact_writer = Some(self.checkout_repo_writer(&checkout.repo, id)?);
                checkout.verify_branch()?;
                corral_git::commit_all(&run.worktree, &format!("{id} {}", task.title))
                    .context("commit task changes")?;
                let head =
                    corral_git::rev_sha(&run.worktree, "HEAD").context("read submitted HEAD")?;
                run.base_sha = checkout.merge_base()?;
                let diff = corral_git::diff_text(&run.worktree, &run.base_sha)
                    .context("read submitted diff")?;
                let target = || ArtifactTarget {
                    owner: ArtifactOwner::Task(id),
                    run: Some(&run.id),
                    actor: self.actor,
                    replaces: None,
                };
                self.add_artifact_in(store, target(), diff.as_bytes(), ArtifactKind::Diff, "diff.patch")?;
                self.add_artifact_in(
                    store,
                    target(),
                    summary.as_bytes(),
                    ArtifactKind::Summary,
                    "summary.md",
                )?;
                // The scan and Done share this transaction. Mark the start of
                // that interval so an attachment can recover its exact submission,
                // including when a retained worker submits the same head again.
                let artifact_scan_after_seq = store.latest_seq()?;
                for path in self.scan_artifacts(&run.worktree, &run.base_sha, &head)? {
                    if let Err(error) = self.add_scanned_artifact_in(store, target(), &path) {
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
                    json!({"sha":run.head_sha,"base_sha":run.base_sha,"project":task.project,"summary":summary,"artifact_scan_after_seq":artifact_scan_after_seq}),
                )?;
                Ok((Some(run), true))
            })?;
        if submitted {
            self.kick_pipeline(id)?;
        }
        Ok(run)
    }

    pub fn gate(&self, id: &str) -> Result<Check> {
        let _writer = self.checkout_writer(id)?;
        let check = self.with_pipeline_lock(id, || {
            let task = self.store.require_task(id)?;
            let run = self.last_worker(&self.store, &task)?;
            let project = self.project_for(&task)?;
            let mut effects_started = false;
            match self.run_gate(
                &run,
                task.round,
                project.as_ref(),
                self.actor,
                &mut effects_started,
            ) {
                Ok(check) => {
                    if task.blocked.as_ref().is_some_and(|blocked| {
                        matches!(blocked.reason.as_str(), GATE_FAILED | PIPELINE_FAILED)
                    }) {
                        self.store.transaction(|store| {
                            let mut current = store.require_task(id)?;
                            if current.blocked == task.blocked {
                                let interrupted = store.interrupted_submission(id)?;
                                if let Some(previous) = interrupted.as_deref()
                                    && previous != check.key
                                {
                                    return self.block_review_resubmit(
                                        store,
                                        &mut current,
                                        &run,
                                        self.actor,
                                        Some(previous),
                                        &check.key,
                                    );
                                }
                                let status = if interrupted.is_some() {
                                    Status::Review
                                } else {
                                    Status::Running
                                };
                                set_lifecycle(&mut current, status);
                                store.update_task(&current)?;
                                self.event(
                                    store,
                                    Some(id),
                                    Some(&run.id),
                                    EventKind::Resumed,
                                    self.actor,
                                    json!({"operation": "gate"}),
                                )?;
                            }
                            Ok(())
                        })?;
                    }
                    Ok(check)
                }
                Err(error) => {
                    if effects_started {
                        self.store.transaction(|store| {
                            let mut current = store.require_task(id)?;
                            if current.status == task.status
                                && current.status.holds_slot()
                                && !store.has_queued_repair(id)?
                            {
                                self.record_operation_failure(
                                    store,
                                    &mut current,
                                    Some(&run.id),
                                    OperationFailure {
                                        operation: "gate",
                                        reason: GATE_FAILED,
                                        error: &error,
                                        next: vec![Next::run(Op::Gate { id: id.into() })],
                                    },
                                    self.actor,
                                )?;
                            }
                            Ok(())
                        })?;
                    }
                    Err(error)
                }
            }
        })?;
        self.kick_pipeline(id)?;
        Ok(check)
    }

    pub fn review(&self, id: &str, profile: Option<&str>) -> Result<Run> {
        self.review_selected(id, profile, self.actor, None)
    }

    pub(crate) fn review_after_gate(&self, id: &str, checked: Option<&str>) -> Result<()> {
        self.request_review(id, None, Actor::Auto, None, checked)
            .map(|_| ())
    }

    pub(crate) fn review_selected(
        &self,
        id: &str,
        profile: Option<&str>,
        actor: Actor,
        selected: Option<&str>,
    ) -> Result<Run> {
        self.request_review(id, profile, actor, selected, None)?
            .ok_or_else(|| {
                refuse(
                    "submit the current candidate with done before requesting review",
                    vec![next_done(id)],
                )
            })
    }

    fn request_review(
        &self,
        id: &str,
        profile: Option<&str>,
        actor: Actor,
        selected: Option<&str>,
        checked: Option<&str>,
    ) -> Result<Option<Run>> {
        self.launch_transaction(id, |store, pending| {
            let mut task = store.require_task(id)?;
            if let Some(run) = selected {
                store.require_relaunch_target(&task, Role::Reviewer, run)?;
            }
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
            self.require_worktree(&worker)?;
            let checkout = Checkout::from_run(&worker)?;
            checkout.verify_branch()?;
            let head = corral_git::rev_sha(&checkout.repo, &checkout.branch_ref())
                .context("read current candidate before requesting review")?;
            let submitted = store.review_submission_sha(id)?;
            if submitted.as_deref() != Some(head.as_str())
                || worker.head_sha.as_deref() != Some(head.as_str())
            {
                // Commit the actionable block before returning a refusal to an
                // explicit review/relaunch caller. No session effects occurred.
                self.block_review_resubmit(
                    store,
                    &mut task,
                    &worker,
                    actor,
                    submitted.as_deref(),
                    &head,
                )?;
                return Ok(None);
            }
            if checked.is_some_and(|sha| sha != head) {
                // The branch moved back to the submitted candidate after the
                // pipeline selected a gate check. Give the next advance a chance
                // to consume a check for that candidate, not the other SHA.
                return Ok(None);
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
                // A reviewer started before the bound-verdict protocol can only
                // send the legacy, unbound command. Replace it rather than reusing it.
                if let Some(session) = session
                    && (!is_pi_reviewer(&reviewer) || store.has_bound_review_request(&reviewer)?)
                {
                    if store.has_review_request_since_done(id, &reviewer.id, task.round)? {
                        return Ok(Some(reviewer));
                    }
                    let prompt = self
                        .state_dir
                        .join("runs")
                        .join(id)
                        .join(&reviewer.id)
                        .join("prompt.md");
                    let mut references = PromptReferences::load(self, store, &task, None)?;
                    if is_pi_reviewer(&reviewer) {
                        references.review_request = Some(store.pi_review_request(&task)?);
                    }
                    let request = rereview_request(
                        &crate::binary_path()?,
                        &task,
                        &reviewer,
                        &prompt,
                        &references,
                    )?;
                    match self
                        .session_runner()
                        .send_obligation(session, &request, Origin::Auto)
                    {
                        Ok(pickup) => {
                            if reviewer.quiet_since.take().is_some() {
                                store.update_run(&reviewer)?;
                            }
                            self.record_review_request(
                                store, &mut task, &reviewer, actor, recovery, pickup,
                            )?;
                            return Ok(Some(reviewer));
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
                    fresh: false,
                },
            )?;
            self.record_review_request(store, &mut task, &run, actor, recovery, None)?;
            Ok(Some(run))
        })
    }

    fn block_review_resubmit(
        &self,
        store: &Store,
        task: &mut Task,
        worker: &Run,
        actor: Actor,
        submitted: Option<&str>,
        candidate: &str,
    ) -> Result<()> {
        let detail = if let Some(sha) = submitted.filter(|sha| *sha != candidate) {
            format!(
                "Candidate {candidate} does not match submitted SHA {sha}. Submit the current candidate with done."
            )
        } else {
            format!("No matching submission for candidate {candidate}. Submit it with done.")
        };
        self.block_in(
            store,
            task,
            Some(&worker.id),
            Blocked {
                reason: "candidate needs resubmission".into(),
                detail: Some(detail),
                next: vec![next_done(&task.id)],
            },
            actor,
        )
    }

    fn record_review_request(
        &self,
        store: &Store,
        task: &mut Task,
        run: &Run,
        actor: Actor,
        recovery: bool,
        pickup: Option<Pickup>,
    ) -> Result<()> {
        if recovery {
            set_lifecycle(task, Status::Review);
            store.update_task(task)?;
        } else if task.attention.take().is_some() {
            store.update_task(task)?;
        }
        let mut detail = json!({"profile":run.profile,"round":task.round,"pickup":pickup});
        if is_pi_reviewer(run) {
            detail["request"] = json!(store.pi_review_request(task)?.id);
        }
        self.event(
            store,
            Some(&task.id),
            Some(&run.id),
            EventKind::ReviewStarted,
            actor,
            detail,
        )
    }

    pub fn verdict(&self, id: &str, approve: bool, notes: Option<&str>) -> Result<Option<Run>> {
        self.verdict_selected(id, approve, notes, None)
    }

    pub(crate) fn bound_verdict(&self, id: &str, run: &str, request: &str) -> Result<()> {
        self.verdict_selected(id, false, None, Some((run, request)))
            .map(|_| ())
    }

    fn verdict_selected(
        &self,
        id: &str,
        approve: bool,
        notes: Option<&str>,
        bound: Option<(&str, &str)>,
    ) -> Result<Option<Run>> {
        let _writer = self.checkout_writer(id)?;
        let (run, close, recorded) = self.launch_transaction(id, |store, pending| {
            let mut task = store.require_task(id)?;
            let mut reviewer = store.latest_run(id, Role::Reviewer)?;
            let intent;
            let (approve, notes, actor) = if let Some((run, request)) = bound {
                let target = store.get_run(id, run)?.filter(is_pi_reviewer).ok_or_else(stale_request)?;
                if store.recorded_review_verdict(&target, request)? {
                    return Ok((Some(target), false, false));
                }
                intent = self.read_authorized_verdict(store, &task, &target, request)?;
                reviewer = Some(target);
                // Reconciliation may have blocked the task before an owner could
                // retry the saved verdict. Restore Review only after authorization.
                if task.status == Status::Blocked {
                    set_lifecycle(&mut task, Status::Review);
                    store.update_task(&task)?;
                }
                (intent.verdict == Decision::Approve, Some(intent.notes.as_str()), Actor::Reviewer)
            } else {
                if self.actor == Actor::Reviewer {
                    let caller = std::env::var("CORRAL_RUN").ok();
                    let called_from_pi = caller
                        .as_deref()
                        .map(|run| store.get_run(id, run))
                        .transpose()?
                        .flatten()
                        .as_ref()
                        .is_some_and(is_pi_reviewer);
                    if called_from_pi || reviewer.as_ref().is_some_and(is_pi_reviewer) {
                        return Err(refuse("Pi reviewer verdicts require a bound review request; use corral_verdict in a current Pi reviewer session", vec![]));
                    }
                }
                (approve, notes, self.actor)
            };
            if task.status != Status::Review {
                return Err(refuse(
                    "verdict requires a task in Review",
                    vec![next_done(id), next_force_approve(id)],
                ));
            }
            let mut close = false;
            if let Some(run) = &mut reviewer
                && (run.ended.is_none() || bound.is_some())
            {
                run.head_sha = store
                    .latest_run(id, Role::Worker)?
                    .and_then(|worker| worker.head_sha);
                if approve && run.ended.is_none() {
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
                by: actor,
                at: Timestamp::now(),
                round: Some(task.round),
                run: reviewer.as_ref().map(|run| run.id.clone()),
            };
            store.insert_check(&check)?;
            let mut detail = json!({"approve":approve,"notes":notes,"round":task.round});
            if let Some((_, request)) = bound {
                detail["request"] = json!(request);
                detail["candidate"] = json!(reviewer.as_ref().and_then(|run| run.head_sha.as_ref()));
            }
            self.event(store, Some(id), check.run.as_deref(), EventKind::Verdict, actor, detail)?;
            if approve && task.attention.take().is_some() {
                store.update_task(&task)?;
            }
            if !approve {
                self.send_back_in(
                    store,
                    pending,
                    &mut task,
                    notes.unwrap_or("Reviewer requested changes."),
                    SendBack::Automatic { onto: None },
                )?;
            }
            Ok((reviewer, close, true))
        })?;
        if !recorded {
            return Ok(run);
        }
        // A reviewer can call verdict from the very session this closes. Publish
        // the verdict and start the next step before touching its process or pane.
        let advanced = self.kick_pipeline(id);
        let stopped =
            if close && let Some(session) = run.as_ref().and_then(|run| run.session.as_ref()) {
                self.session_runner()
                    .stop(session)
                    .context("close reviewer after recording verdict")
            } else {
                Ok(())
            };
        advanced.and(stopped)?;
        Ok(run)
    }

    pub fn changes(&self, id: &str, text: &str) -> Result<Option<Run>> {
        self.launch_transaction(id, |store, pending| {
            let mut task = store.require_task(id)?;
            if !matches!(task.status, Status::Review | Status::Blocked) {
                return Err(refuse(
                    "changes requires a task in Review or Blocked",
                    vec![next_done(id), next_retry(id)],
                ));
            }
            self.send_back_in(store, pending, &mut task, text, SendBack::Requested)
        })
    }

    pub(crate) fn send_back_in(
        &self,
        store: &Store,
        pending: &mut PendingLaunch,
        task: &mut Task,
        notes: &str,
        reason: SendBack<'_>,
    ) -> Result<Option<Run>> {
        let mut worker = self.last_worker(store, task)?;
        pending.reserve_checkout(
            self.checkout_repo_writer(&Checkout::from_run(&worker)?.repo, &task.id)?,
        );
        let project = self.project_for(task)?;
        // Ordinary send-backs stay in the task's existing slot. A forced
        // landing of an unadmitted task can also reach this path on conflict.
        if !task.status.holds_slot() {
            self.require_dispatch_slot(store, task.project.as_deref(), project.as_ref())?;
        }
        let max_rounds = project
            .as_ref()
            .map_or(self.config.defaults.max_rounds, |p| p.max_rounds);
        let (actor, reset) = match reason {
            SendBack::Requested => (self.actor, true),
            SendBack::Automatic { .. } => (Actor::Auto, false),
        };
        if let SendBack::Automatic { onto } = reason
            && task.round >= max_rounds
        {
            let next = vec![
                next_changes(&task.id),
                next_retry(&task.id),
                Next::force(Op::Approve {
                    id: task.id.clone(),
                    onto: onto.map(str::to_owned),
                }),
            ];
            self.block_in(
                store,
                task,
                Some(&worker.id),
                Blocked {
                    reason: crate::repair_queue::EXHAUSTED.into(),
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
        let accepted = if let Some(session) = live_session {
            match self
                .session_runner()
                .send_obligation(session, notes, origin)
            {
                Ok(pickup) => Some(pickup),
                Err(error) if as_refusal(&error).is_some() => None,
                Err(error) => return Err(error).context("send work back to worker"),
            }
        } else {
            None
        };
        let pickup = if let Some(pickup) = accepted {
            if worker.quiet_since.take().is_some() {
                store.update_run(&worker)?;
            }
            pickup
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
                    fresh: false,
                },
            )?;
            None
        };
        task.round = if reset { 0 } else { task.round + 1 };
        set_lifecycle(task, Status::Running);
        store.update_task(task)?;
        self.event(
            store,
            Some(&task.id),
            Some(&worker.id),
            EventKind::SentBack,
            actor,
            json!({"notes":notes,"round":task.round,"pickup":pickup}),
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
        self.launch_transaction(id, |store, pending| {
            let mut task = store.require_task(id)?;
            if task.status == Status::Review {
                self.send_back_in(
                    store,
                    pending,
                    &mut task,
                    notes,
                    SendBack::Automatic { onto: None },
                )?;
            }
            Ok(())
        })
    }
}

impl Store {
    fn review_submission_sha(&self, id: &str) -> Result<Option<String>> {
        self.connection.query_row(
            "SELECT (SELECT json_extract(detail,'$.sha') FROM events WHERE task=?1 AND kind='done' ORDER BY seq DESC LIMIT 1)",
            [id], |row| row.get(0),
        ).context("read submitted review candidate")
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
