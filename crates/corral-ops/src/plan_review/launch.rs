//! Plan review launch over heads (plan T-253 section 4): the last plan with its current review,
//! the plan's planning run by key and the emitted Previous plan findings and Owner comments.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    HerdrRef, OperatingActionKind, PlanReview, PlanReviewHead, PlanRunState, ProfileChoice,
    TaskHeadMut, TaskMut as _, TaskRef as _, Verdict,
};
use serde_json::json;

use super::{close_plan_review, next_plan_attempt, prepared};
use crate::{Ctx, plan, prompt};

/// The current review of plan `plan`, when `plan` is still the task's last.
fn current_review<'a>(
    task: &'a mut TaskHeadMut,
    plan: &str,
) -> Result<Option<&'a mut PlanReviewHead>> {
    Ok(task
        .last_plan_mut()
        .filter(|p| p.id == plan)
        .context("stale plan verdict: plan was replaced; inspect `corral plan show <id> --json`")?
        .current_review
        .as_mut())
}

pub(super) fn review_with_origin(
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
    let head = ctx.store.head(id)?;
    // The launch reads head fields, the last plan with its current review and the last
    // planning run, which the head's shell holds; counts come from the plan's facts.
    let task = head.shell_task()?;
    let config = plan::config(ctx)?;
    let mut review_ctx = ctx.clone();
    review_ctx.store.config = config.clone();
    let (receipt, receipt_warning) = if reviewer.is_none()
        && head.last_plan.is_some()
        && config.task_review_policy(&task).reviewer == corral_core::Reviewer::Auto
    {
        crate::review_receipts::plan_for_launch(
            &review_ctx,
            &task,
            &next_plan_attempt(ctx, &head)?,
        )?
    } else {
        (None, None)
    };
    let name = reviewer
        .map(str::to_string)
        .or_else(|| receipt.as_ref().map(|r| r.profile.clone()))
        .map(Ok)
        .unwrap_or_else(|| plan::expected_reviewer(&config, &task))?;
    if name == "owner" {
        bail!(
            "owner review is manual: use `corral plan verdict {} approve|changes|escalate --plan <plan-id> --notes-file <path>`",
            task.id
        );
    }
    plan::independent_reviewer(&config, &task, &name)?;
    let mut route = crate::review_contracts::plan_route(
        &config,
        &task,
        &name,
        reviewer.is_some(),
        receipt.as_ref(),
    )?;
    route.warnings.extend(receipt_warning);
    let preferred = config.resolve_profile(
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
    let resolved = crate::availability::plan_reviewer(&review_ctx, &task, name == "pm", preferred)?;
    let selected = task
        .plans
        .last()
        .context("submit a plan before reviewing it")?;
    let reviews = head
        .last_plan
        .as_ref()
        .map_or(Ok(0), |p| usize::try_from(p.facts.children))?;
    plan::fresh(ctx, &task, selected)?;
    let retained = prepared::load(ctx, &task.id)?;
    if let Some(retained) = &retained {
        retained.check(selected)?;
        ensure!(
            retained.review.reviewer == name
                && reviews == retained.previous_reviews
                && retained.review.route.as_deref().is_none_or(|route| {
                    crate::review_contracts::plan_route_changes(&config, &task, route).is_empty()
                }),
            "prepared plan review was superseded"
        );
    }
    if retained.is_none() && selected.reviews.last().is_some_and(|r| r.verdict.is_none()) {
        bail!("plan review already pending");
    }
    let plan_id = selected.id.clone();
    let previous = selected.clone();
    let at = corral_core::clock::now();
    let attempt = retained.as_ref().map_or_else(
        || format!("PR{}", reviews + 1),
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
    let dir = crate::scratch_reclaim::Attempt::PlanReview {
        task: &task.id,
        plan: &plan_id,
        review: &attempt,
    }
    .open(ctx)?;
    let prompt_file = dir.join(format!("{}-{plan_id}-{attempt}-plan-review.md", task.id));
    let notes_file = dir.join(format!("{}-{plan_id}-{attempt}-plan-notes.md", task.id));
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
    text.push_str(&crate::review_contracts::plan_context_head(
        ctx, &head, &task, &route,
    )?);
    text.push_str(&crate::comments::owner_comments_head(&head, &task)?);
    text.push_str(&crate::scratch_reclaim::prompt_section(ctx, &dir));
    // The renderer sizes the prompt to decide how `{prompt_file}` may reach the reviewer.
    std::fs::write(&prompt_file, &text).context("writing plan review prompt")?;
    let native = crate::agent::hooks::is_pi_native(&resolved.harness) && name != "pm";
    let command = prepared::command(ctx, &resolved, &task, &prompt_file, native)?;
    let receipt = crate::launch_exit::receipt(ctx, &task.id, &format!("{plan_id}-{attempt}"))?;
    let command = crate::launch_exit::arm(ctx, &command, &receipt)?;
    let mut preparation = match retained {
        Some(mut retained) => {
            retained.authorization = authorization.clone();
            retained
        }
        None => prepared::Prepared {
            plan: plan_id.clone(),
            artifact_sha256: selected.artifact.sha256.clone(),
            spec_revision: selected.spec_revision.clone(),
            previous_reviews: reviews,
            review: PlanReview {
                agent_host: None,
                delivery: if native {
                    crate::agent::hooks::delivery::launch_mode()
                } else {
                    corral_core::inbox::DeliveryMode::Legacy
                },
                inbox: Default::default(),
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
                route: Some(Box::new(route)),
            },
            authorization,
            prompt: prompt_file.clone(),
            command,
        },
    };
    // Publish before sending: a synchronous reviewer can record its verdict immediately.
    ctx.store.dispatch_update_head(&task.id, |store, t| {
        crate::operating::require_origin(store, &*t, OperatingActionKind::PlanReview, origin)?;
        ensure!(origin == crate::operating::Origin::Owner || !t.dispatch_hold, "automatic plan review is held");
        crate::session_scope::check_launch_in(ctx, store)?;
        if store.config != config {
            bail!("plan review configuration changed before admission; retry");
        }
        if let Some(reason) = crate::resources::read_admission(ctx)? {
            bail!("{reason}");
        }
        let heads = crate::pm_next::heads::open(store)?;
        let usage = crate::slots::SlotUsage::from_heads(&heads, &store.config.limits);
        if usage.reviews + usage.gates >= store.config.limits.review {
            bail!("review process limit reached");
        }
        let shell = t.shell_task()?;
        if store.config.limits.count_reviews
            && let Some(reason) = crate::slots::capacity_reason(store, &shell, 0)? {
                bail!("{reason}");
            }
        let revision = plan::plan_spec_revision(&shell)?;
        let p = t.head().last_plan.as_ref().filter(|p| p.id == plan_id).context(
            "stale plan verdict: plan was replaced; inspect `corral plan show <id> --json`")?;
        if Some(p) != head.last_plan.as_ref() || p.spec_revision != revision {
            bail!("plan changed before review launch");
        }
        let run_id = p.run.clone();
        std::fs::write(&prompt_file, text).context("writing plan review prompt")?;
        t.push_plan_review(&plan_id, preparation.review.clone())?;
        t.last_plan_mut().context("plan disappeared")?.approval = None;
        // The plan's planning run, by key: the last one when current, else its history row.
        let run = match run_id.as_deref() {
            Some(id) => t.plan_run_head(id)?,
            None => None,
        };
        let cancelled = run.as_ref().is_some_and(|r| r.state == PlanRunState::Cancelled);
        if !cancelled && let Some(run) = &run {
            t.edit_plan_run(&run.id, |r| {
                r.state = PlanRunState::Reviewing;
                Ok(())
            })?;
        }
        if !cancelled
            && (run_id.is_some()
                || t.head().current_run.as_ref().is_none_or(|r| r.outcome.is_some()))
        {
            t.status = corral_core::Status::Review;
        }
        ctx.store.log_event(Some(&t.id), "plan_review_requested",
            json!({"target":"plan","plan":plan_id,"attempt":attempt,"reviewer":name,"prompt":prompt_file}))?;
        Ok(())
    })?;
    preparation.save(ctx, &task.id)?;
    close_plan_review(ctx, &previous)?;
    let mut launch = || -> Result<HerdrRef> {
        let herdr = if let Some(herdr) = preparation.live_pane(ctx)? {
            herdr
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
        let attach = ctx.store.update_head(&task.id, |t| {
            let r = current_review(t, &plan_id)?
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
        if native {
            preparation.bind_native(ctx, &head, &resolved, &herdr)?;
        }
        let pressure = crate::resources::read_admission(ctx)?;
        let prepare = |store: &corral_core::Store, task: &mut corral_core::TaskHeadMut| {
            let saved_review = task
                .head()
                .last_plan
                .as_ref()
                .and_then(|plan| plan.current_review.as_ref());
            ensure!(
                saved_review == Some(&PlanReviewHead::of(&preparation.review))
                    && preparation.review.verdict.is_none(),
                "prepared plan review attempt changed"
            );
            let task = &task.shell_task()?;
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
                review.policy_digest == plan::policy_digest(&store.config, task)?
                    && review.route.as_deref().is_none_or(|route| {
                        crate::review_contracts::plan_route_changes(&store.config, task, route)
                            .is_empty()
                    }),
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
            crate::availability::pm_plan_reviewer(ctx, &config, &task)?;
            let delivery = crate::operating::submit_head(
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
            crate::operating::submit_head(ctx, &preparation.authorization, prepare, || {
                ctx.herdr.pane_run(&herdr.pane_id, &preparation.command)
            })?;
        }
        Ok(herdr)
    };
    match launch() {
        Ok(herdr) => {
            let saved = ctx.store.update_head(&task.id, |t| {
                let r = current_review(t, &plan_id)?.filter(|r| r.id == attempt).context("plan review replaced during launch")?;
                r.pending_reply = None;
                ctx.store.log_event(Some(&task.id), "plan_review_started",
                    json!({"target":"plan","plan":plan_id,"attempt":attempt,"reviewer":name,"pane":herdr.pane_id}))?;
                Ok(())
            })?.shell_task()?;
            let p = saved.plans.last().context("plan disappeared")?;
            if p.reviews.last().is_some_and(|r| r.verdict.is_some()) {
                close_plan_review(ctx, p)?;
            }
            if native {
                let _known = corral_core::work::followup("T-626", "plan_reviewer_complete_result");
                return ctx
                    .store
                    .get(&task.id)?
                    .plans
                    .into_iter()
                    .find(|plan| plan.id == plan_id)
                    .and_then(|plan| plan.reviews.into_iter().find(|review| review.id == attempt))
                    .context("plan review disappeared");
            }
            p.reviews.last().cloned().context("plan review disappeared")
        }
        Err(error) => {
            if crate::operating::refused(&error) {
                preparation.suspend(ctx, &task.id)?;
                return Err(error);
            }
            ctx.store.update_head(&task.id, |t| {
                if let Some(review) = current_review(t, &plan_id)?
                    .filter(|r| r.id == attempt && r.verdict.is_none())
                {
                    review.verdict = Some(Verdict::Escalate);
                    review.notes = Some(format!("Review launch failed: {error:#}"));
                    review.pending_reply = None;
                }
                let run_id = t.head().last_plan.as_ref().and_then(|p| p.run.clone());
                if let Some(run) = run_id.as_deref().map(|id| t.plan_run_head(id)).transpose()?.flatten()
                    .filter(|r| r.state != PlanRunState::Cancelled)
                {
                    t.edit_plan_run(&run.id, |r| {
                        r.state = PlanRunState::Failed;
                        Ok(())
                    })?;
                }
                ctx.store.log_event(Some(&task.id), "plan_review_failed",
                    json!({"target":"plan","plan":plan_id,"attempt":attempt,"error":format!("{error:#}")}))?;
                Ok(())
            })?;
            let saved = ctx.store.head(&task.id)?.shell_task()?;
            if let Some(p) = saved.plans.iter().find(|p| p.id == plan_id) {
                close_plan_review(ctx, p)?;
            }
            Err(error)
        }
    }
}
