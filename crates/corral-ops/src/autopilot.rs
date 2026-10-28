//! The review autopilot (docs/DESIGN.md §5): drives a run that `corral done` moved to Review through the effective
//! review policy (gate, then reviewer, then land when accept = auto or wait for the owner when accept = owner),
//! loops failures back to the task agent up to `max_rounds`, and hands the task to the owner (Needs me) when a
//! guard trips. `sync` advances it; every stage transition logs one "autopilot" event.

use anyhow::Result;
use corral_core::{
    Accept, AutoStage, AutopilotState, Config, GateRecord, GateResult, MergeMode,
    OperatingActionKind, ReviewPolicy, ReviewRecord, Reviewer, Run, RunOutcome, Status, Task,
    Verdict,
};
use corral_git::RebaseOutcome;
use jiff::Timestamp;
use serde_json::json;

use crate::acceptance::{Landing, record_acceptance};
use crate::artifacts::glob_match;
use crate::cleanup::{Cleanup, cleanup};
use crate::dispatch::{auto_request_changes, auto_retry};
use crate::gate::{gate_collect, gate_paths, worktree_head};
use crate::integration::{IntegrationPacket, RepairKey};
use crate::prompt::corral_bin;
use crate::review::{approve_landed, has_tracked_changes, main_repo};
use crate::reviewer::review_run_automatic as review_run;
use crate::verification;
use crate::{ApproveStrategy, Change, Ctx, DispatchOutcome};

/// Bound on the steps one task takes in one sync. A step that changes nothing waits, so this only guards
/// against a loop bug.
const MAX_STEPS: usize = 16;

mod infra;
mod output;
mod repair;
mod tiers;
use output::{LOG_TAIL_LINES, first_line, log_tail, short};

enum Step {
    /// Waiting on something outside (a gate, the reviewer, the owner, a dirty main checkout): stop for this sync.
    Wait,
    /// Did something (started a gate) without changing stage: look again.
    Acted,
    /// Changed stage and logged one event.
    Moved(Change),
}

enum GateNow {
    Pending,
    Started,
    Pass,
    Fail(Box<GateRecord>),
    /// The gate pane closed before the gate wrote a result.
    Lost,
    /// The candidate was behind its destination, so it was replayed instead of gated.
    Replayed(Step),
}

/// Advances every Review task as far as it can go now (called by `sync`). Re-reads corral.toml first, so
/// `corral autopilot off` also stops long-running syncs (the board, `sync --watch`); with the kill switch off
/// nothing auto-advances. Transient failures use the diagnostic sink and retry on the next sync.
pub(crate) fn advance_all(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Autopilot);
    let mut store = ctx.store.clone();
    store.reload_config_if_changed()?;
    let ctx = Ctx {
        store,
        ..ctx.clone()
    };
    let mut changes = Vec::new();
    for task in ctx.store.list()? {
        if task.status != Status::Review || crate::plan_pilot::handles(&task) {
            continue;
        }
        // Defer typed markers to advance, which records them under the task lock.
        if task.infra_wait.is_none()
            && let Err(error) = output::record_authority(&ctx, &task)
            && !infra::classified(&error)
        {
            return Err(error);
        }
        match advance(&ctx, &task.id, now) {
            Ok(c) => changes.extend(c),
            Err(e) => {
                let error = format!("{e:#}");
                crate::messages::report(
                    Some(&task.id),
                    "autopilot",
                    format!("autopilot {}: {error}", task.id),
                );
                output::record_error(&ctx, &task, &error)?;
            }
        }
    }
    crate::plan_acceptance::checkpoint::start_due(&ctx);
    Ok(changes)
}

fn advance(ctx: &Ctx, id: &str, now: Timestamp) -> Result<Vec<Change>> {
    // The board worker, the herdr hook and `sync --watch` may sync at once; one of them advances a task.
    let Some(_lock) = ctx.store.try_lock(&format!("autopilot-{id}"))? else {
        return Ok(Vec::new());
    };
    let mut changes = Vec::new();
    for _ in 0..MAX_STEPS {
        let next = match step(ctx, id, now) {
            Ok(next) => next,
            Err(error) => {
                if let Some(wait) = error.downcast_ref::<crate::review_budget::CapacityWait>() {
                    wait.persist(ctx, id, now)?;
                    break;
                }
                return Err(error);
            }
        };
        match next {
            Step::Wait => break,
            Step::Acted => {}
            Step::Moved(c) => changes.push(c),
        }
    }
    Ok(changes)
}

fn step(ctx: &Ctx, id: &str, now: Timestamp) -> Result<Step> {
    let task = ctx.store.get(id)?;
    // Only runs finished with `corral done`; a Review task with an open run was moved there by hand.
    let Some(run) = task
        .current_run()
        .filter(|r| r.outcome == Some(RunOutcome::Done))
    else {
        return Ok(Step::Wait);
    };
    let config = &ctx.store.config;
    let policy = config.task_review_policy(&task);
    let pilot = Pilot {
        ctx,
        config,
        task: &task,
        run,
        policy,
        now,
    };
    // Main movement can stale an approval, but cannot erase a known defect on this candidate.
    // Handle it before integration preflight or a stale gate can restart verification.
    if !matches!(pilot.stage(), Some(AutoStage::Escalated | AutoStage::Done))
        && let Some(review) = run.review.as_ref().filter(|r| {
            r.verdict == Some(Verdict::Changes)
                && worktree_head(run).as_deref() == Some(r.sha.as_str())
        })
    {
        let Some(kind) = crate::automation_status::next_action(ctx, &task)? else {
            return Ok(Step::Wait);
        };
        if crate::operating::require(&ctx.store, &task, kind).is_err() {
            return Ok(Step::Wait);
        }
        if let Err(error) = verification::require_gate(&pilot.policy, &task) {
            return pilot.escalate(&error.to_string());
        }
        return pilot.reviewer_changes(review);
    }
    if task.infra_wait.is_some() {
        match pilot.infra_guard() {
            Ok(Some(step)) => return Ok(step),
            Ok(None) => return step(ctx, id, now),
            Err(error) => {
                return pilot.escalate(&format!("integration preflight error: {error:#}"));
            }
        }
    }
    let kind = match crate::automation_status::next_action(ctx, &task) {
        Ok(Some(kind)) => kind,
        Ok(None) => return Ok(Step::Wait),
        Err(error) if infra::classified(&error) => return pilot.record_infra(&error),
        Err(error) => return Err(error),
    };
    if crate::operating::require(&ctx.store, &task, kind).is_err() {
        return Ok(Step::Wait);
    }
    if let Err(error) = verification::require_gate(&pilot.policy, &task) {
        return pilot.escalate(&error.to_string());
    }
    if pilot.policy.reviewer == Reviewer::CrossVendor {
        match crate::reviewer::implementer_vendor(ctx, &task, run) {
            Ok(_) => {}
            Err(error) if error.is::<corral_core::UnresolvedRunVendor>() => {
                return pilot.escalate(&error.to_string());
            }
            Err(error) => return Err(error),
        }
    }
    if pilot.stage() == Some(AutoStage::Reviewed) {
        let gate_stale =
            !pilot.policy.gate.is_empty() && !verification::gate_passed(ctx, &task, run)?;
        let review_stale = pilot.policy.reviewer != Reviewer::None
            && !verification::review_fresh(ctx, &task, run)?;
        if !gate_stale && !review_stale {
            // Fresh receipts need no integration packet or search-index refresh. Recheck
            // acceptance authority so a newly delegated task can still finish.
            return pilot.accept(worktree_head(run).unwrap_or_default());
        }
    }
    if !matches!(pilot.stage(), Some(AutoStage::Escalated | AutoStage::Done)) {
        match pilot.preflight() {
            Ok(Some(step)) => return Ok(step),
            Ok(None) => {}
            Err(error) if error.is::<crate::integration::StalePacket>() => return Ok(Step::Wait),
            Err(error) if infra::classified(&error) => return pilot.record_infra(&error),
            Err(error) => {
                return pilot.escalate(&format!("integration preflight error: {error:#}"));
            }
        }
    }
    match pilot.stage() {
        None | Some(AutoStage::Changes) => pilot.start(),
        Some(AutoStage::Gating) => pilot.gating(),
        Some(AutoStage::Reviewing) => pilot.reviewing(),
        Some(AutoStage::Landing | AutoStage::Blocked) => pilot.landing(),
        Some(AutoStage::Reviewed) => pilot.start(),
        Some(AutoStage::Escalated | AutoStage::Done) => Ok(Step::Wait),
    }
}

struct Pilot<'a> {
    ctx: &'a Ctx,
    config: &'a Config,
    task: &'a Task,
    run: &'a Run,
    policy: ReviewPolicy,
    now: Timestamp,
}

impl Pilot<'_> {
    fn id(&self) -> &str {
        &self.task.id
    }

    fn state(&self) -> Option<&AutopilotState> {
        self.run.autopilot.as_ref()
    }

    fn stage(&self) -> Option<AutoStage> {
        self.state().map(|a| a.stage)
    }

    fn pane_alive(&self, pane: Option<&str>) -> Result<bool> {
        match pane {
            Some(p) => Ok(self.ctx.herdr.pane_get(p)?.is_some()),
            None => Ok(false),
        }
    }

    fn start(&self) -> Result<Step> {
        let Some(head) = worktree_head(self.run) else {
            return self.escalate("the run's worktree is gone");
        };
        if !corral_git::is_clean(&self.run.worktree)? {
            return self.escalate("the worktree has uncommitted changes after `corral done`");
        }
        if self.policy.gate.is_empty() {
            return self.after_gate(head);
        }
        // A gate that passed (or is running) on this exact commit stands; a failed one is re-run.
        let reuse = match &self.run.gate {
            Some(g) if !matches!(g.result, GateResult::Fail | GateResult::Stale) => {
                self.gate_stands(g)?
            }
            _ => false,
        };
        if !reuse && let GateNow::Replayed(step) = self.start_gate(&head)? {
            return Ok(step);
        }
        let note = if reuse {
            "reusing the gate run on this commit"
        } else {
            "gate started"
        };
        self.moved(AutoStage::Gating, Some(head), None, note)
    }

    fn gating(&self) -> Result<Step> {
        let Some(head) = worktree_head(self.run) else {
            return self.escalate("the run's worktree is gone");
        };
        if self.policy.gate.is_empty() {
            return self.after_gate(head);
        }
        match self.gate_at(&head)? {
            GateNow::Pending => Ok(Step::Wait),
            GateNow::Started => Ok(Step::Acted),
            GateNow::Pass => self.after_gate(head),
            GateNow::Fail(g) => {
                let (summary, details) = self.gate_failure(&g, "")?;
                self.request_changes(&summary, &details)
            }
            GateNow::Lost => self.escalate("the gate pane closed before the gate finished"),
            GateNow::Replayed(step) => Ok(step),
        }
    }

    fn after_gate(&self, head: String) -> Result<Step> {
        let base = corral_git::rev_sha(&self.run.worktree, &self.run.base)?;
        if !corral_git::is_ancestor(&self.run.worktree, &base, &head)? {
            return self.rebase();
        }
        if self.promote()? {
            return Ok(Step::Acted);
        }
        let failures = crate::completion::evidence_failures(self.ctx, self.task, self.run)?;
        if !failures.is_empty() {
            return self.escalate(&format!(
                "required evidence missing or stale: {}",
                failures.join("; ")
            ));
        }
        if self.policy.reviewer == Reviewer::None {
            if self
                .task
                .completion
                .as_ref()
                .is_some_and(|m| m.audited && m.criteria.iter().any(|c| c.required))
            {
                return self.escalate("required semantic criteria need a reviewer; worker evidence is not machine PASS");
            }
            return self.accept(head);
        }
        if crate::verdict_carry::carry(self.ctx, self.id())? {
            return Ok(Step::Acted);
        }
        if self.run.review.as_ref().and_then(|r| r.verdict) == Some(Verdict::Approve)
            && verification::review_fresh(self.ctx, self.task, self.run)?
        {
            return self.accept(head);
        }
        let review = review_run(self.ctx, self.id(), None)?;
        let note = format!("review by {} started", review.reviewer);
        self.moved(AutoStage::Reviewing, Some(head), None, &note)
    }

    /// The gate's result on `head`: collects a finished gate, starts one when none ran on `head`.
    fn gate_at(&self, head: &str) -> Result<GateNow> {
        let gate = match self.run.gate.clone() {
            None => return self.start_gate(head),
            Some(g) if g.result == GateResult::Running => {
                match gate_collect(self.ctx, self.id())? {
                    Some(finished) => finished,
                    None if !g.process_lost => return Ok(GateNow::Pending),
                    None if g.sha == head => return Ok(GateNow::Lost),
                    None => return self.start_gate(head),
                }
            }
            Some(g) => g,
        };
        if gate.sha != head || !self.gate_stands(&gate)? {
            return self.start_gate(head);
        }
        Ok(match gate.result {
            GateResult::Pass => GateNow::Pass,
            GateResult::Fail | GateResult::Stale => GateNow::Fail(Box::new(gate)),
            GateResult::Running => GateNow::Pending,
        })
    }

    /// A failed reduced-tier gate names its tier and re-gate rules from the run's gate contract.
    fn gate_failure(&self, g: &GateRecord, after: &str) -> Result<(String, String)> {
        let cmd = g.failed_cmd.as_deref().unwrap_or("?").trim();
        let gate = format!("{} gate {} --wait", corral_bin()?.display(), self.id());
        let tier = crate::prompt::reduced_send_back(self.ctx, self.task, self.run, &gate)?
            .filter(|(tier, _)| g.tier.as_ref().is_some_and(|t| t.tier == *tier));
        let label = tier.as_ref().map(|(t, _)| format!(" ({t} tier)"));
        let summary = format!(
            "the gate{} failed{after} at {}: `{cmd}`",
            label.unwrap_or_default(),
            short(&g.sha)
        );
        let log = gate_paths(self.ctx, self.id(), &self.run.id).log;
        let mut details = format!(
            "Last {LOG_TAIL_LINES} lines of the gate log ({}):\n\n```\n{}\n```",
            log.display(),
            log_tail(&log, LOG_TAIL_LINES)
        );
        if let Some((_, text)) = tier {
            details.push_str(&format!("\n\n{text}"));
        }
        Ok((summary, details))
    }

    fn reviewing(&self) -> Result<Step> {
        let Some(head) = worktree_head(self.run) else {
            return self.escalate("the run's worktree is gone");
        };
        if self
            .run
            .review
            .as_ref()
            .is_some_and(|r| r.verdict == Some(Verdict::Approve) && r.sha == head)
        {
            let base = corral_git::rev_sha(&self.run.worktree, &self.run.base)?;
            if !corral_git::is_ancestor(&self.run.worktree, &base, &head)? {
                return self.rebase();
            }
        }
        if !self.policy.gate.is_empty()
            && self.run.review.as_ref().is_none_or(|r| r.verdict.is_some())
            && !verification::gate_passed(self.ctx, self.task, self.run)?
        {
            return self.start();
        }
        let Some(review) = self.run.review.as_ref() else {
            review_run(self.ctx, self.id(), None)?;
            return Ok(Step::Acted);
        };
        let who = review.reviewer.as_str();
        let notes = review.notes.as_deref().unwrap_or("").trim();
        match review.verdict {
            None if head != review.sha => self.escalate(&format!(
                "the worktree changed during review (HEAD moved from {} to {}); a reviewer must not edit it",
                short(&review.sha),
                short(&head)
            )),
            None if review.process_lost => {
                self.escalate(&format!("the reviewer ({who}) closed without a verdict"))
            }
            None => Ok(Step::Wait),
            Some(Verdict::Approve) => {
                // HEAD that corral itself replayed from the approved commit (approve or integration) is
                // not a reviewer edit: gate it, then carry the verdict or request a fresh review.
                let replayed = head != review.sha
                    && crate::verdict_carry::clean_replay(
                        self.ctx, self.task, self.run, &review.sha, &head,
                    )?;
                if (head != review.sha && !replayed) || !corral_git::is_clean(&self.run.worktree)? {
                    return self.escalate(
                        "the worktree changed after the reviewer approved it; a reviewer must not edit it",
                    );
                }
                if replayed || !verification::review_fresh(self.ctx, self.task, self.run)? {
                    return self.start();
                }
                self.accept(head)
            }
            Some(Verdict::Changes) => self.reviewer_changes(review),
            Some(Verdict::Escalate) => {
                let why = first_line(notes).trim_start_matches("escalate:").trim();
                self.escalate(&format!("the reviewer ({who}) escalated: {why}"))
            }
        }
    }

    /// Gate and review passed: wait for the owner (accept = owner), or check the guards and start landing.
    /// Diff size is deliberately not a guard: the independent reviewer is the safeguard.
    fn accept(&self, head: String) -> Result<Step> {
        if self.task.review == Some(Accept::Owner)
            && self
                .config
                .review_policy(self.task.project.as_deref(), None)
                .accept
                == Accept::Auto
        {
            return self.escalate("the task asks for the owner's acceptance (review: owner)");
        }
        let base = corral_git::rev_sha(&self.run.worktree, &self.run.base)?;
        let stat = corral_git::diff_stat(&self.run.worktree, &base)?;
        let touched: Vec<&str> = stat
            .files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| self.policy.owner_paths.iter().any(|g| glob_match(g, p)))
            .collect();
        if !touched.is_empty() {
            return self.escalate(&format!(
                "the change touches owner_paths: {}",
                touched.join(", ")
            ));
        }
        if crate::operating::require(&self.ctx.store, self.task, OperatingActionKind::Accept)
            .is_err()
        {
            if self.stage() == Some(AutoStage::Reviewed) {
                return Ok(Step::Wait);
            }
            return self.moved(
                AutoStage::Reviewed,
                Some(head),
                None,
                "passed; waiting for delegated acceptance and independent review",
            );
        }
        self.moved(AutoStage::Landing, Some(head), None, "accepted; landing")
    }

    /// Landing (and Blocked, which retries it): main checkout ready, branch on top of the base (rebasing makes
    /// the gate stale, so it runs again), gate passed on the commit, then fast-forward the base.
    fn landing(&self) -> Result<Step> {
        let Some(head) = worktree_head(self.run) else {
            return self.escalate("the run's worktree is gone");
        };
        let expected = self.state().and_then(|a| a.sha.clone()).unwrap_or_default();
        if head != expected {
            return self.escalate(&format!(
                "the worktree changed while landing (HEAD moved from {} to {})",
                short(&expected),
                short(&head)
            ));
        }
        if !corral_git::is_clean(&self.run.worktree)? {
            return self.escalate("the worktree has uncommitted changes while landing");
        }
        let project_repo = main_repo(self.ctx, self.task, self.run)?;
        let target = crate::plan_target::validate_run(&self.ctx.store, self.task, self.run)?;
        let repo = target
            .as_ref()
            .map_or(project_repo.as_path(), |target| target.checkout.as_path());
        let base = self.run.base.as_str();
        if corral_git::current_branch(repo).ok().as_deref() != Some(base) {
            return self.block(&format!("main checkout is not on {base}"));
        }
        if has_tracked_changes(repo)? {
            return self.block("main checkout has uncommitted changes");
        }
        if self.stage() == Some(AutoStage::Blocked) {
            return self.moved(
                AutoStage::Landing,
                Some(head),
                None,
                "main checkout is ready again",
            );
        }
        let base_head = corral_git::rev_sha(repo, base)?;
        if !corral_git::is_ancestor(&self.run.worktree, &base_head, &head)? {
            return self.rebase();
        }
        if (!self.policy.gate.is_empty()
            && !crate::review::landing_gate_passed(self.ctx, self.task, self.run)?)
            || (self.policy.reviewer != Reviewer::None
                && !verification::review_fresh(self.ctx, self.task, self.run)?)
        {
            return self.start();
        }
        if crate::operating::require(&self.ctx.store, self.task, OperatingActionKind::Accept)
            .is_err()
        {
            return self.accept(head);
        }
        if !self.policy.gate.is_empty() {
            match self.gate_at(&head)? {
                GateNow::Pass => {}
                GateNow::Pending => return Ok(Step::Wait),
                GateNow::Started => return Ok(Step::Acted),
                GateNow::Fail(g) => {
                    let (summary, details) =
                        self.gate_failure(&g, &format!(" after rebasing onto {base}"))?;
                    return self.request_changes(&summary, &details);
                }
                GateNow::Lost => {
                    return self.escalate("the gate pane closed before the gate finished");
                }
                GateNow::Replayed(step) => return Ok(step),
            }
        }
        self.land(&base_head)
    }

    fn land(&self, base_head: &str) -> Result<Step> {
        let stat = corral_git::diff_stat(&self.run.worktree, base_head)?;
        let receipt = self
            .run
            .gate
            .as_ref()
            .and_then(|g| g.identity.as_ref())
            .or_else(|| self.run.review.as_ref().and_then(|r| r.identity.as_ref()));
        let expected = match receipt {
            Some(receipt) => verification::identity(self.ctx, self.task, self.run, receipt)?,
            None => verification::capture_identity(self.ctx, self.task, self.run, "no-checks")?,
        };
        if expected.integration_base != base_head {
            return self.block("stale_retry: integration base moved before landing");
        }
        let landed = approve_landed(
            self.ctx,
            self.id(),
            ApproveStrategy::FastForward,
            self.policy.remove_worktree,
            false,
            Some(&expected),
        );
        let (sha, line) = match landed {
            Ok(v) => v,
            Err(e) => return self.block(&format!("{e:#}")),
        };
        let state = AutopilotState {
            stage: AutoStage::Done,
            reason: None,
            sha: Some(sha.clone()),
            at: self.now,
        };
        let run_id = self.run.id.clone();
        self.ctx.store.update(self.id(), |t| {
            if let Some(r) = t.current_run_mut().filter(|r| r.id == run_id) {
                r.autopilot = Some(state);
            }
            Ok(())
        })?;
        self.log(
            AutoStage::Done,
            self.task.rounds,
            Some(&sha),
            None,
            &format!("landed: {line}"),
        )?;
        let landing = Landing {
            policy: &self.policy,
            sha: &sha,
            line: &line,
            stat: &stat,
            at: self.now,
        };
        record_acceptance(self.ctx, self.id(), &self.run.id, &landing)?;
        Ok(Step::Moved(Change {
            task: self.task.id.clone(),
            from: Status::Review,
            to: Status::Done,
            note: format!("autopilot landed: {line}"),
            stage: Some(AutoStage::Done),
        }))
    }

    /// Sends the run back to its agent with `summary` (one line) and `details`, or escalates when the rounds are
    /// used up. Reuse resumes a missing worker's recorded session; explicit fresh mode or an incompatible
    /// profile escalation starts a retry whose prompt carries the request.
    fn send_request(
        &self,
        summary: &str,
        details: &str,
        key: &RepairKey,
        packet: Option<&str>,
    ) -> Result<Step> {
        key.check(&self.ctx.store.get(self.id())?)?;
        let digest = key.digest()?;
        if packet.is_some()
            && self.ctx.store.events_shared()?.iter().any(|e| {
                e.task.as_deref() == Some(self.id())
                    && e.kind == "integration_request"
                    && e.detail["digest"].as_str() == Some(digest.as_str())
            })
        {
            return Ok(Step::Wait);
        }
        let round = self.task.rounds + 1;
        let max = self.policy.max_rounds;
        if round > max && !self.ctx.store.config.automation.turbo {
            return self.escalate(&format!("{summary} (all {max} change rounds used)"));
        }
        crate::operating::require(&self.ctx.store, self.task, OperatingActionKind::Repair)?;
        let id = self.id();
        let corral = corral_bin()?.display().to_string();
        let mut request = format!("{summary}\n");
        if !details.trim().is_empty() {
            request.push_str(&format!("\n{}\n", details.trim_end()));
        }
        let finish = format!(
            "Fix it, commit, and run `{corral} done {id} --summary \"<what changed>\"` again."
        );
        let document =
            format!("# Changes requested: {id} round {round} of {max}\n\n{request}\n{finish}\n");
        let artifact = self.snapshot(
            &format!("changes-round-{round}.md"),
            &document,
            &format!("Autopilot changes (round {round})"),
        )?;
        let file = self.ctx.store.dir().join(&artifact.path);
        let message = format!(
            "corral autopilot: changes requested on {id} (round {round} of {max}): {}. Details: {}. {finish}",
            summary.trim_end_matches('.'),
            file.display()
        );
        let state = AutopilotState {
            stage: AutoStage::Changes,
            reason: Some(request),
            sha: worktree_head(self.run),
            at: self.now,
        };
        let pane = self.run.herdr.as_ref().map(|h| h.pane_id.as_str());
        let task = self.ctx.store.get(id)?;
        let switch = crate::escalation::escalation(self.config, &task, false)?;
        let reuse = self.config.task_review_policy(&task).worker_session
            == corral_core::SessionPolicy::Reuse;
        let alive = if switch.is_none() {
            self.pane_alive(pane)?
        } else {
            false
        };
        let sent: Result<(Status, String)> = if switch.is_none() && (reuse || alive) {
            auto_request_changes(self.ctx, id, &message, round, state, key).map(|_| {
                (
                    Status::Running,
                    format!("changes requested (round {round}/{max}): {summary}"),
                )
            })
        } else {
            auto_retry(self.ctx, id, round, state, key).map(|outcome| match outcome {
                DispatchOutcome::Started { run_id, .. } => (
                    Status::Running,
                    format!(
                        "fresh retry run {run_id} carries the change request (round {round}/{max}): {summary}"
                    ),
                ),
                DispatchOutcome::Queued { reason, .. } => (
                    Status::Ready,
                    format!(
                        "fresh retry queued ({reason}) with the change request (round {round}/{max}): {summary}"
                    ),
                ),
            })
        };
        // Capacity or a moved snapshot only delays the send-back: the prepared authorization
        // stays claimed, so a later sync retries the same request instead of handing it to the owner.
        let (to, note) = match sent {
            Err(error) if crate::operating::transient_refusal(&error) => return Ok(Step::Wait),
            Err(error) if error.is::<crate::role_session::SessionUnavailable>() => {
                return self.escalate(&error.to_string());
            }
            sent => sent?,
        };
        if let Some(packet) = packet {
            self.ctx.store.log_event(
                Some(id),
                "integration_request",
                json!({ "key": key, "digest": digest, "packet": packet }),
            )?;
        }
        self.log(AutoStage::Changes, round, None, Some(summary), &note)?;
        Ok(Step::Moved(Change {
            task: id.to_string(),
            from: Status::Review,
            to,
            note: format!("autopilot {note}"),
            stage: Some(AutoStage::Changes),
        }))
    }

    fn moved(
        &self,
        to: AutoStage,
        sha: Option<String>,
        reason: Option<&str>,
        note: &str,
    ) -> Result<Step> {
        let from = self.stage();
        let state = AutopilotState {
            stage: to,
            reason: reason.map(str::to_string),
            sha: sha.clone(),
            at: self.now,
        };
        if !self.write(from, state, Status::Review)? {
            return Ok(Step::Wait);
        }
        self.log(to, self.task.rounds, sha.as_deref(), reason, note)?;
        Ok(Step::Moved(Change {
            task: self.task.id.clone(),
            from: Status::Review,
            to: Status::Review,
            note: format!("autopilot {}: {note}", to.as_str()),
            stage: Some(to),
        }))
    }

    fn log(
        &self,
        to: AutoStage,
        round: u32,
        sha: Option<&str>,
        reason: Option<&str>,
        note: &str,
    ) -> Result<()> {
        let current = self.ctx.store.get(self.id())?;
        let released_review_runs: Vec<_> = if to == AutoStage::Escalated {
            self.run
                .review_runs
                .iter()
                .filter(|before| {
                    before.ended.is_none()
                        && current.current_run().is_some_and(|run| {
                            run.review_runs
                                .iter()
                                .any(|after| after.id == before.id && after.ended.is_some())
                        })
                })
                .map(|run| &run.id)
                .collect()
        } else {
            Vec::new()
        };
        self.ctx.store.log_event(
            Some(self.id()),
            "autopilot",
            json!({
                "run": self.run.id,
                "from": self.stage().map_or("review", AutoStage::as_str),
                "to": to.as_str(),
                "round": round,
                "max_rounds": self.policy.max_rounds,
                "sha": sha,
                "reason": reason,
                "note": note,
                "released_review_runs": released_review_runs,
            }),
        )
    }
}
