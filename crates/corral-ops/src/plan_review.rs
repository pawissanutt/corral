//! Separate plan review attempts. These never populate implementation review receipts.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    HerdrRef, OperatingActionKind, OperatingMode, PlanApproval, PlanRecord, PlanReview,
    PlanRunState, ProfileChoice, Task, Verdict,
};
use jiff::Timestamp;
use serde_json::json;

use crate::{Ctx, plan, prompt};

mod prepared;

fn current<'a>(task: &'a mut Task, id: &str) -> Result<&'a mut PlanRecord> {
    task.plans
        .last_mut()
        .filter(|p| p.id == id)
        .context("stale plan verdict: plan was replaced; inspect `corral plan show <id> --json`")
}

/// End admission and reject callbacks before a pending attempt is abandoned.
pub(crate) fn terminalize_pending(plan: &mut PlanRecord, reason: &str) {
    for review in plan.reviews.iter_mut().filter(|r| r.verdict.is_none()) {
        review.verdict = Some(Verdict::Escalate);
        review.notes = Some(reason.into());
        review.pending_reply = None;
        review.at = Timestamp::now();
    }
}

/// Close only a dedicated reviewer workspace, never the shared PM.
pub(crate) fn close_plan_review(ctx: &Ctx, plan: &PlanRecord) -> Result<()> {
    let Some(review) = plan.reviews.last().filter(|r| r.reviewer != "pm") else {
        return Ok(());
    };
    let Some(herdr) = &review.herdr else {
        return Ok(());
    };
    if ctx.herdr.pane_get(&herdr.pane_id)?.is_some() {
        ctx.herdr.pane_close(&herdr.pane_id)?;
    }
    if !ctx
        .herdr
        .pane_list()?
        .iter()
        .any(|p| p.workspace_id == herdr.workspace_id)
        && ctx
            .herdr
            .workspace_list()?
            .iter()
            .any(|(id, _)| *id == herdr.workspace_id)
    {
        ctx.herdr.workspace_close(&herdr.workspace_id)?;
    }
    Ok(())
}

pub fn plan_review(ctx: &Ctx, id: &str, reviewer: Option<&str>) -> Result<PlanReview> {
    review_with_origin(ctx, id, reviewer, crate::operating::Origin::Owner)
}

pub(crate) fn review_auto(ctx: &Ctx, id: &str) -> Result<PlanReview> {
    review_with_origin(ctx, id, None, crate::operating::Origin::Automatic)
}

/// A suspended attempt owns its next action even when an older verdict remains
/// on the task. Never let that older verdict trigger revision or acceptance.
pub(crate) fn resume_prepared(ctx: &Ctx, task: &Task) -> Result<bool> {
    let Some(preparation) = prepared::load(ctx, &task.id)? else {
        return Ok(false);
    };
    preparation.check(task.plans.last().context("prepared plan disappeared")?)?;
    if crate::operating::is_automatic(&preparation.authorization) {
        review_auto(ctx, &task.id)?;
    }
    Ok(true)
}

fn review_with_origin(
    ctx: &Ctx,
    id: &str,
    reviewer: Option<&str>,
    origin: crate::operating::Origin,
) -> Result<PlanReview> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    crate::session_scope::check_launch(ctx)?;
    // Unlike implementation reviews, plans have no run-backed launch journal.
    // Retain the exclusive claim until the real pane reference has been saved.
    let _ownership = ctx
        .store
        .try_lock("session-ownership")?
        .context("pane inventory is being reconciled; retry plan review")?;
    crate::session_scope::check_launch(ctx)?;
    let task = ctx.store.get(id)?;
    let config = plan::config(ctx)?;
    let name = reviewer
        .map(str::to_string)
        .map(Ok)
        .unwrap_or_else(|| plan::expected_reviewer(&config, &task))?;
    if name == "owner" {
        bail!(
            "owner review is manual: use `corral plan verdict {} approve|changes|escalate --plan <plan-id> --notes-file <path>`",
            task.id
        );
    }
    plan::independent_reviewer(&config, &task, &name)?;
    let resolved = config.resolve_profile(
        &ProfileChoice {
            profile: Some(if name == "pm" {
                config.pm.agent.clone()
            } else {
                name.clone()
            }),
            ..Default::default()
        },
        None,
        None,
    )?;
    let selected = task
        .plans
        .last()
        .context("submit a plan before reviewing it")?;
    plan::fresh(ctx, &task, selected)?;
    let retained = prepared::load(ctx, &task.id)?;
    if let Some(retained) = &retained {
        retained.check(selected)?;
        ensure!(
            retained.review.reviewer == name && selected.reviews.len() == retained.previous_reviews,
            "prepared plan review was superseded"
        );
    }
    if retained.is_none() && selected.reviews.last().is_some_and(|r| r.verdict.is_none()) {
        bail!("plan review already pending");
    }
    let plan_id = selected.id.clone();
    let previous = selected.clone();
    let at = Timestamp::now();
    let attempt = retained.as_ref().map_or_else(
        || format!("PR{}", selected.reviews.len() + 1),
        |retained| retained.review.id.clone(),
    );
    let mut action = crate::operating::identity(
        &task.id,
        selected.run.as_deref().unwrap_or(&plan_id),
        OperatingActionKind::PlanReview,
        &attempt,
    );
    action.plan = Some(plan_id.clone());
    let authorization = crate::operating::authorize(ctx, action, origin)?;
    let prompt_file = ctx
        .store
        .prompts_dir()
        .join(format!("{}-{plan_id}-{attempt}-plan-review.md", task.id));
    let notes_file = ctx
        .store
        .state_dir()
        .join("reviews")
        .join(format!("{}-{plan_id}-{attempt}-plan-notes.md", task.id));
    std::fs::create_dir_all(
        notes_file
            .parent()
            .context("plan notes directory missing")?,
    )
    .context("creating plan notes directory")?;
    std::fs::create_dir_all(ctx.store.prompts_dir()).context("creating plan prompt directory")?;
    let corral = prompt::corral_bin()?;
    let verdict_cmd = format!(
        "{} --root '{}' plan verdict {} approve|changes|escalate --plan {plan_id} --attempt {attempt} --notes-file '{}'",
        corral.display(),
        ctx.store
            .root()
            .display()
            .to_string()
            .replace('\'', "'\\''"),
        task.id,
        notes_file.display().to_string().replace('\'', "'\\''"),
    );
    let mut text = format!(
        "# Plan review: {} {}\n\nTarget: plan {plan_id}, attempt {attempt}. This is not implementation review.\n\
         Plan: {}\nSHA-256: {}\nTask-spec revision: {}\n\n## Task spec\n\n{}\n\n\
         ## Plan metadata\n\n```json\n{}\n```\n\n\
         Read the complete snapshot and relevant repository contracts. Review goal/non-goals, alternatives \
         considered, decisions and risks, paths/contracts touched, scope split into tasks with \
         dependencies/write scopes, acceptance scenarios, explicit owner decisions and verification commands. \
         Reuse registered criterion IDs. Escalate unresolved owner decisions or a reviewer-policy mismatch. \
         Do not edit project code, merge, accept implementation, dispatch, or create tasks. \
         Write concrete notes to {} and finish with:\n\n    {verdict_cmd}\n",
        task.id,
        task.title,
        ctx.store.dir().join(&selected.artifact.path).display(),
        selected.artifact.sha256,
        selected.spec_revision,
        task.body,
        serde_json::to_string_pretty(&selected.document)?,
        notes_file.display(),
    );
    text.push_str(&crate::comments::owner_comments(&task));
    let command = prompt::render_template(
        &resolved,
        &[
            ("prompt_file", prompt_file.display().to_string()),
            ("name", format!("{} plan review", task.id)),
            ("task", task.id.clone()),
            ("worktree", ctx.store.root().display().to_string()),
        ],
    )?;
    let mut preparation = match retained {
        Some(mut retained) => {
            retained.authorization = authorization.clone();
            retained
        }
        None => prepared::Prepared {
            plan: plan_id.clone(),
            artifact_sha256: selected.artifact.sha256.clone(),
            spec_revision: selected.spec_revision.clone(),
            previous_reviews: selected.reviews.len(),
            review: PlanReview {
                id: attempt.clone(),
                reviewer: name.clone(),
                policy_digest: plan::policy_digest(&config, &task)?,
                verdict: None,
                notes: None,
                at,
                herdr: None,
                pending_reply: Some(format!(
                    "Review the plan prompt at {}. Follow its exact verdict command.",
                    prompt_file.display()
                )),
            },
            authorization,
            prompt: prompt_file.clone(),
            command,
        },
    };
    // Publish before sending: a synchronous reviewer can record its verdict immediately.
    ctx.store.dispatch_update(&task.id, |store, t| {
        crate::operating::require_origin(store, t, OperatingActionKind::PlanReview, origin)?;
        ensure!(origin == crate::operating::Origin::Owner || !t.dispatch_hold, "automatic plan review is held");
        crate::session_scope::check_launch_in(ctx, store)?;
        if store.config != config {
            bail!("plan review configuration changed before admission; retry");
        }
        if let Some(reason) = crate::resources::read_admission(ctx)? {
            bail!("{reason}");
        }
        let tasks = store.list()?;
        let usage = crate::slots::SlotUsage::from_tasks(&tasks, &store.config.limits);
        if usage.reviews + usage.gates >= store.config.limits.review {
            bail!("review process limit reached");
        }
        if store.config.limits.count_reviews
            && let Some(reason) = crate::slots::capacity_reason(store, t, 0)? {
                bail!("{reason}");
            }
        let revision = plan::plan_spec_revision(t)?;
        let p = current(t, &plan_id)?;
        if p != selected || p.spec_revision != revision {
            bail!("plan changed before review launch");
        }
        std::fs::write(&prompt_file, text).context("writing plan review prompt")?;
        p.reviews.push(preparation.review.clone());
        p.approval = None;
        let run_id = p.run.clone();
        let cancelled = t.plan_runs.iter().any(|r|
            Some(&r.id) == run_id.as_ref() && r.state == PlanRunState::Cancelled);
        if !cancelled
            && let Some(run) = t.plan_runs.iter_mut().find(|r| Some(&r.id) == run_id.as_ref()) {
            run.state = PlanRunState::Reviewing;
        }
        if !cancelled && (run_id.is_some() || t.current_run().is_none_or(|r| r.outcome.is_some())) {
            t.status = corral_core::Status::Review;
        }
        ctx.store.log_event(Some(&t.id), "plan_review_requested",
            json!({"target":"plan","plan":plan_id,"attempt":attempt,"reviewer":name,"prompt":prompt_file}))?;
        Ok(())
    })?;
    preparation.save(ctx, &task.id)?;
    close_plan_review(ctx, &previous)?;
    let mut launch = || -> Result<HerdrRef> {
        let herdr = if let Some(herdr) = &preparation.review.herdr {
            ensure!(
                herdr.socket.as_ref() == Some(&ctx.herdr.socket_path()?),
                "prepared reviewer belongs to another session"
            );
            ensure!(
                ctx.herdr.pane_get(&herdr.pane_id)?.is_some(),
                "prepared reviewer pane disappeared"
            );
            herdr.clone()
        } else if name == "pm" {
            let pane_id = match crate::pm::live_pm_pane(ctx)? {
                Some(pane) => pane,
                None => crate::pm::open_pm(ctx)?,
            };
            let info = ctx
                .herdr
                .pane_get(&pane_id)?
                .context("PM pane disappeared")?;
            HerdrRef {
                workspace_id: info.workspace_id,
                pane_id,
                socket: Some(ctx.herdr.socket_path()?),
            }
        } else {
            let cwd = match task.project.as_deref() {
                Some(project) => ctx.store.project_path(project)?,
                None => ctx.store.root().to_path_buf(),
            };
            let created =
                ctx.herdr
                    .workspace_create(&cwd, &format!("{} plan review", task.id), false)?;
            HerdrRef {
                workspace_id: created.workspace_id,
                pane_id: created.pane_id,
                socket: Some(ctx.herdr.socket_path()?),
            }
        };
        preparation.review.herdr = Some(herdr.clone());
        preparation.save(ctx, &task.id)?;
        let attach = ctx.store.update(&task.id, |t| {
            let r = current(t, &plan_id)?
                .reviews
                .last_mut()
                .filter(|r| r.id == attempt)
                .context("plan review replaced before launch")?;
            r.herdr = Some(herdr.clone());
            Ok(())
        });
        if let Err(error) = attach {
            if name != "pm" {
                ctx.herdr.workspace_close(&herdr.workspace_id)?;
            }
            return Err(error);
        }
        let pressure = crate::resources::read_admission(ctx)?;
        let prepare = |store: &corral_core::Store, task: &mut Task| {
            crate::session_scope::check_launch_in(ctx, store)?;
            ensure!(
                !crate::operating::is_automatic(&preparation.authorization) || !task.dispatch_hold,
                "automatic plan review is held"
            );
            ensure!(
                pressure.is_none(),
                "{}",
                pressure.as_deref().unwrap_or_default()
            );
            let plan = task.plans.last().context("plan review missing")?;
            preparation.check(plan)?;
            plan::fresh(ctx, task, plan)?;
            let review = plan.reviews.last().context("plan review missing")?;
            ensure!(
                review == &preparation.review && review.verdict.is_none(),
                "prepared plan review attempt changed"
            );
            ensure!(
                review.policy_digest == plan::policy_digest(&store.config, task)?,
                "prepared plan reviewer policy changed"
            );
            Ok(())
        };
        ctx.herdr.pane_report_metadata(
            &herdr.pane_id,
            &format!("{} plan review", task.id),
            &[
                ("task", &task.id),
                ("run", &preparation.authorization.action.run),
                ("plan", &plan_id),
                ("attempt", &attempt),
                ("target", "plan-review"),
            ],
        )?;
        if name == "pm" {
            let delivery = crate::operating::submit(
                ctx,
                &preparation.authorization,
                prepare,
                || {
                    ctx.herdr.agent_prompt(&herdr.pane_id, &format!(
                "Review the plan prompt at {}. Follow its exact plan verdict command, including --plan and --attempt.",
                prompt_file.display()
                ))
                },
            )?;
            ctx.store.log_event(
                Some(&task.id),
                "plan_review_delivery",
                json!({"target":"plan","plan":plan_id,"attempt":attempt,"via":delivery.via,
                    "submitted":delivery.submitted,"error":delivery.error}),
            )?;
            if !delivery.submitted {
                bail!("PM plan prompt was not submitted; retained for inspection");
            }
        } else {
            crate::operating::submit(ctx, &preparation.authorization, prepare, || {
                ctx.herdr.pane_run(&herdr.pane_id, &preparation.command)
            })?;
        }
        Ok(herdr)
    };
    match launch() {
        Ok(herdr) => {
            let saved = ctx.store.update(&task.id, |t| {
                let p = current(t, &plan_id)?;
                let r = p.reviews.last_mut().filter(|r| r.id == attempt).context("plan review replaced during launch")?;
                r.pending_reply = None;
                ctx.store.log_event(Some(&task.id), "plan_review_started",
                    json!({"target":"plan","plan":plan_id,"attempt":attempt,"reviewer":name,"pane":herdr.pane_id}))?;
                Ok(())
            })?;
            let p = saved.plans.last().context("plan disappeared")?;
            if p.reviews.last().is_some_and(|r| r.verdict.is_some()) {
                close_plan_review(ctx, p)?;
            }
            p.reviews.last().cloned().context("plan review disappeared")
        }
        Err(error) => {
            if crate::operating::refused(&error) {
                preparation.suspend(ctx, &task.id)?;
                return Err(error);
            }
            ctx.store.update(&task.id, |t| {
                let p = current(t, &plan_id)?;
                if let Some(review) = p
                    .reviews
                    .last_mut()
                    .filter(|r| r.id == attempt && r.verdict.is_none())
                {
                    review.verdict = Some(Verdict::Escalate);
                    review.notes = Some(format!("Review launch failed: {error:#}"));
                    review.pending_reply = None;
                }
                let run_id = p.run.clone();
                if let Some(run) = t
                    .plan_runs
                    .iter_mut()
                    .find(|r| Some(&r.id) == run_id.as_ref() && r.state != PlanRunState::Cancelled)
                {
                    run.state = PlanRunState::Failed;
                }
                Ok(())
            })?;
            let saved = ctx.store.get(&task.id)?;
            if let Some(p) = saved.plans.iter().find(|p| p.id == plan_id) {
                close_plan_review(ctx, p)?;
            }
            ctx.store.log_event(Some(&task.id), "plan_review_failed",
                json!({"target":"plan","plan":plan_id,"attempt":attempt,"error":format!("{error:#}")}))?;
            Err(error)
        }
    }
}

pub struct PlanVerdict<'a> {
    pub plan: &'a str,
    /// None is an explicit owner verdict, never attributed to a launched agent.
    pub attempt: Option<&'a str>,
    pub verdict: Verdict,
    pub notes: &'a str,
}

pub fn plan_verdict(ctx: &Ctx, id: &str, input: &PlanVerdict<'_>) -> Result<PlanReview> {
    let _attribution = input
        .attempt
        .map(|_| ctx.store.attribute(corral_core::Actor::Reviewer));
    if input.notes.trim().is_empty() {
        bail!("plan verdict needs nonempty notes");
    }
    let mut superseded = None;
    let saved = ctx.store.update(id, |task| {
        let config = plan::config(ctx)?;
        let selected = task.plans.last().filter(|p| p.id == input.plan).context(
            "stale plan verdict: plan was replaced; inspect `corral plan show <id> --json`",
        )?;
        plan::fresh(ctx, task, selected)?;
        let digest = plan::policy_digest(&config, task)?;
        let required = config.plan_required(task.project.as_deref(), task.plan_required);
        let reviewer = match input.attempt {
            Some(attempt) => {
                let review = selected.reviews.last().filter(|r| r.id == attempt && r.verdict.is_none())
                    .context("stale plan verdict: review attempt was replaced or already has a verdict")?;
                if review.policy_digest != digest {
                    bail!("stale plan verdict: reviewer policy changed; review again");
                }
                review.reviewer.clone()
            }
            None => {
                superseded = Some(selected.clone());
                "owner".into()
            }
        };
        let owner = input.attempt.is_none();
        let expected = plan::expected_reviewer(&config, task)?;
        let reviewed = selected.reviews.last().is_some_and(|r|
            r.verdict == Some(Verdict::Approve) && r.reviewer == expected
                && r.policy_digest == digest);
        let decisions = plan::owner_decisions(task, selected);
        let mut notes = input.notes.to_string();
        let verdict = match input.verdict {
            Verdict::Approve if owner && plan::expected_reviewer(&config, task)? != "owner" && !reviewed => {
                notes.push_str("\nEscalated: independent review must approve before owner acceptance.");
                Verdict::Escalate
            }
            Verdict::Approve if !owner && reviewer != plan::expected_reviewer(&config, task)? => {
                notes.push_str("\nEscalated: unauthorized reviewer.");
                Verdict::Escalate
            }
            Verdict::Approve if decisions && owner && !plan::decisions_resolved(task, selected, input.notes) => {
                notes.push_str("\nEscalated: owner must resolve decisions explicitly in notes as `Resolved: <decision> => <resolution>`.");
                Verdict::Escalate
            }
            Verdict::Approve => Verdict::Approve,
            Verdict::Changes => Verdict::Changes,
            Verdict::Escalate => Verdict::Escalate,
        };
        if decisions && !owner {
            notes.push_str("\nOwner decisions remain; owner acceptance must record each as `Resolved: <decision> => <resolution>`.");
        }
        // Keep the snapshot's verdict without restarting its cancelled execution.
        let cancelled = task.plan_runs.iter().any(|r|
            Some(&r.id) == selected.run.as_ref() && r.state == PlanRunState::Cancelled);
        // Recording a review is always allowed, including while automatic work
        // is paused. Automatic acceptance is a separate fresh, scoped action.
        let accepted = verdict == Verdict::Approve && owner;
        if accepted {
            crate::plan_decompose::check_attachment(&ctx.store, task, input.plan)?;
        }
        // An old queued intent must not turn approval into an automatic implementation launch.
        if required {
            task.queued = false;
        }
        let p = current(task, input.plan)?;
        p.approval = accepted.then(|| PlanApproval {
            by: reviewer.clone(), at: Timestamp::now(), policy_digest: digest.clone(),
            authorization: None,
        });
        match input.attempt {
            Some(_) => {
                let review = p.reviews.last_mut().context("plan review missing")?;
                review.verdict = Some(verdict);
                review.notes = Some(notes);
                review.at = Timestamp::now();
            }
            None => {
                terminalize_pending(p, "Superseded by an owner decision.");
                p.reviews.push(PlanReview {
                id: format!("PR{}", p.reviews.len() + 1),
                reviewer,
                policy_digest: digest,
                verdict: Some(verdict),
                notes: Some(notes),
                at: Timestamp::now(),
                herdr: None,
                pending_reply: None,
                });
            }
        }
        let run_id = p.run.clone();
        if let Some(run) = task.plan_runs.iter_mut().find(|r|
            Some(&r.id) == run_id.as_ref() && r.state != PlanRunState::Cancelled) {
            run.state = match verdict {
                Verdict::Approve if accepted => PlanRunState::Approved,
                Verdict::Approve => PlanRunState::AwaitingOwner,
                Verdict::Changes => PlanRunState::Changes,
                Verdict::Escalate => PlanRunState::Escalated,
            };
        }
        if !cancelled && verdict == Verdict::Approve && !accepted {
            let effective = ctx.store.effective_operating_policy()?;
            task.status = if decisions || config.explicit_owner_acceptance(task, true)
                || !config.automation.authority.plan_accept
                || effective.mode == OperatingMode::Assist {
                corral_core::Status::NeedsMe
            } else {
                corral_core::Status::Review
            };
        } else if !cancelled && verdict == Verdict::Approve && accepted && run_id.is_some() {
            task.status = corral_core::Status::Ready;
        }
        ctx.store.log_event(Some(&task.id), "plan_verdict",
            json!({"target":"plan","plan":input.plan,"review":task.plans.last().and_then(|p|p.reviews.last())}))?;
        if verdict == Verdict::Approve && !accepted {
            ctx.store.log_event(Some(&task.id), "plan_awaiting_owner",
                json!({"target":"plan","plan":input.plan,"reason":"awaiting owner approval"}))?;
        }
        Ok(())
    })?;
    let p = saved.plans.last().context("plan disappeared")?;
    if let Some(previous) = superseded {
        close_plan_review(ctx, &previous)?;
    }
    close_plan_review(ctx, p)?;
    if p.approval.is_some() {
        crate::plan_decompose::apply(ctx, id, input.plan)?;
    } else if input.attempt.is_some()
        && p.reviews
            .last()
            .is_some_and(|r| r.verdict == Some(Verdict::Approve))
        && crate::operating::require(
            &ctx.store,
            &ctx.store.get(id)?,
            OperatingActionKind::AcceptPlan,
        )
        .is_ok()
    {
        // A refusal must not discard an already collected independent verdict.
        if let Err(error) = accept_auto(ctx, id, input.plan) {
            crate::messages::report(Some(id), "automatic plan acceptance", format!("{error:#}"));
        }
    }
    p.reviews.last().cloned().context("plan verdict missing")
}

/// Reconcile an already completed independent review under explicit automatic policy.
pub(crate) fn accept_auto(ctx: &Ctx, id: &str, plan_id: &str) -> Result<()> {
    let snapshot = ctx.store.get(id)?;
    let selected = snapshot
        .plans
        .last()
        .filter(|p| p.id == plan_id)
        .context("stale plan acceptance")?;
    let review = selected
        .reviews
        .last()
        .context("independent review required")?;
    let mut action = crate::operating::identity(
        &snapshot.id,
        selected.run.as_deref().unwrap_or(plan_id),
        OperatingActionKind::AcceptPlan,
        &format!("accept-{}", review.id),
    );
    action.plan = Some(plan_id.into());
    let authorization =
        crate::operating::authorize(ctx, action, crate::operating::Origin::Automatic)?;
    crate::operating::submit(
        ctx,
        &authorization,
        |store, task| {
            let config = &store.config;
            let selected = task
                .plans
                .last()
                .filter(|p| p.id == plan_id)
                .context("stale plan acceptance")?;
            if task
                .plan_runs
                .iter()
                .any(|r| Some(&r.id) == selected.run.as_ref() && r.state == PlanRunState::Cancelled)
            {
                bail!("cancelled plan run requires explicit replacement dispatch");
            }
            plan::fresh(ctx, task, selected)?;
            let digest = plan::policy_digest(config, task)?;
            let review = selected
                .reviews
                .last()
                .context("independent review required")?;
            if review.verdict != Some(Verdict::Approve)
                || review.id != authorization.action.attempt.trim_start_matches("accept-")
                || review.reviewer != plan::expected_reviewer(config, task)?
                || matches!(review.reviewer.as_str(), "owner" | "pm")
                || review.policy_digest != digest
                || plan::owner_decisions(task, selected)
            {
                bail!(
                    "automatic plan acceptance requires current independent approval without owner decisions"
                );
            }
            let by = review.reviewer.clone();
            crate::plan_decompose::check_attachment(store, task, plan_id)?;
            crate::plan_decompose::check_scopes(store, task, plan_id)?;
            let selected = current(task, plan_id)?;
            selected.approval = Some(PlanApproval {
                by,
                at: Timestamp::now(),
                policy_digest: digest,
                authorization: Some(authorization.id.clone()),
            });
            let run_id = selected.run.clone();
            if let Some(run) = task
                .plan_runs
                .iter_mut()
                .find(|r| Some(&r.id) == run_id.as_ref())
            {
                run.state = PlanRunState::Approved;
            }
            task.queued = false;
            if run_id.is_some() {
                task.status = corral_core::Status::Ready;
            }
            Ok(())
        },
        || Ok(()),
    )?;
    crate::plan_decompose::apply_auto(ctx, id, plan_id)?;
    Ok(())
}
