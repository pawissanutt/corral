//! The reviewer launch (plan T-253 section 4, "reviewer"): the acted task is read as its head,
//! helpers get it and its current run projection, earlier review children are reached only by
//! key, and the launch guard compares the parent's `children` fact and its current review
//! record: the full form's child count and record.
use super::*;
use corral_core::{RunHead, TaskHead, TaskHeadMut};

/// The current run of any task form, as its projection.
fn current_run_head<T: corral_core::TaskRef + ?Sized>(task: &T) -> Result<RunHead> {
    Ok(task
        .current_run_head()?
        .with_context(|| format!("task {} has no run", task.id()))?
        .into_owned())
}

/// Retry only a read-only launch phase, never the launch's pane or accounting effects.
fn read_step<T>(
    ctx: &Ctx,
    id: &str,
    parent: &str,
    mut read: impl FnMut(&Ctx, &TaskHead, &RunHead) -> Result<T>,
) -> Result<T> {
    ctx.read_step(|ctx| {
        let head = ctx.store.head(id)?;
        let run = current_run_head(&head)?;
        anyhow::ensure!(
            head.status == Status::Review && run.id == parent,
            "review parent changed during preparation"
        );
        read(ctx, &head, &run)
    })
}

fn prepare_prompt(ctx: &Ctx, task: &TaskHead, run: &RunHead) -> Result<(String, Option<String>)> {
    read_step(ctx, &task.id, &run.id, |read, head, current| {
        Ok((
            review_prompt(read, head, current)?,
            crate::round_notice::latest_review(head)?,
        ))
    })
}

fn review_parent_head<'a>(task: &'a mut TaskHeadMut, run_id: &str) -> Result<&'a mut RunHead> {
    task.current_run_mut()
        .filter(|r| r.id == run_id)
        .ok_or_else(|| anyhow!("run {run_id} is no longer current"))
}

/// The accounting child a launch records for its review.
fn review_child(
    ctx: &Ctx,
    run: &RunHead,
    resolved: &ResolvedProfile,
    reviewer: &Reviewer,
    review_id: &str,
    record: &ReviewRecord,
) -> Result<Run> {
    Ok(Run {
        execution: None,
        delivery: crate::delivery::owner::review_launch_mode(&resolved.harness),
        inbox: Default::default(),
        completion: Default::default(),
        changes_round: None,
        session: None,
        id: review_id.to_owned(),
        progress: None,
        checkpoints: vec![],
        checkpoint_required: false,
        agent: resolved.profile.clone(),
        profile: None,
        harness: Some(resolved.harness.clone()),
        model: resolved.model.clone(),
        effort: resolved.effort.clone(),
        route: None,
        vendor: resolved.vendor.clone(),
        failures: 0,
        switches: vec![],
        retry: None,
        mode: RunMode::Interactive,
        worktree: match reviewer {
            Reviewer::Pm => ctx.store.root().to_path_buf(),
            Reviewer::Profile(_) | Reviewer::CrossVendor | Reviewer::Auto => run.worktree.clone(),
            Reviewer::None => bail!("no reviewer configured"),
        },
        branch: run.branch.clone(),
        base: run.base.clone(),
        base_sha: run.base_sha.clone(),
        plan_target: run.plan_target.clone(),
        herdr: None,
        started: record.at,
        ended: None,
        outcome: None,
        summary: None,
        blocked_excerpt: None,
        agent_state: None,
        seen_pane: false,
        seen_working: false,
        gate: None,
        idle_since: None,
        idle_no_report: false,
        herdr_idle_since: None,
        stuck: None,
        pending_reply: None,
        obligations: Default::default(),
        review: Some(record.clone()),
        review_runs: Vec::new(),
        usage: None,
        usage_note: None,
        usage_pending: false,
        autopilot: None,
        gate_rules: corral_core::GateRules::Legacy,
        promotion_origin: None,
        agent_host: None,
    })
}

fn launch_profile<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    resolved: &ResolvedProfile,
    prompt_file: &Path,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<String> {
    let title = format!("{} review", task.id());
    let herdr = open_run_pane(ctx, task, run, "review")?;
    attach_pane(ctx, task, run, &herdr.pane_id, true, launch)?;
    let mut child = String::new();
    ctx.store.update_head(task.id(), |task| {
        let review = review_parent_head(task, &run.id)?
            .current_child
            .as_deref_mut()
            .context("review child disappeared before launch")?;
        review.herdr = Some(herdr.clone());
        child = review.id.clone();
        Ok(())
    })?;
    let saved = ctx.store.head(task.id())?;
    let saved_child = saved
        .current_run
        .as_deref()
        .filter(|parent| parent.id == run.id)
        .and_then(|parent| parent.current_child.as_deref())
        .filter(|current| current.id == child)
        .context("review session disappeared before launch")?;
    let resolved = crate::role_session::command_head(ctx, saved_child, resolved)?;
    let command =
        match crate::agent::hooks::review::command(ctx, task, run, &resolved, &herdr.pane_id) {
            Some(command) => command?,
            None => {
                let mut values = vec![
                    ("prompt_file", prompt_file.display().to_string()),
                    ("name", title.clone()),
                    ("task", task.id().clone()),
                    ("worktree", run.worktree.display().to_string()),
                ];
                if let Some(notice) = crate::round_notice::latest_review(&saved)? {
                    values.push((crate::prompt::NOTICE_KEY, notice));
                }
                render_template(&resolved, &values)?
            }
        };
    let receipt = crate::launch_exit::receipt(ctx, task.id(), &format!("{}-{child}", run.id))?;
    let command = crate::launch_exit::arm(ctx, &command, &receipt)?;
    let pane = herdr.pane_id;
    ctx.herdr
        .pane_report_metadata(&pane, &title, &[("task", task.id().as_str())])?;
    submission::submit(ctx, task, run, launch, None, || {
        ctx.herdr.pane_run(&pane, &command)
    })?;
    Ok(pane)
}

fn attach_pane<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    pane: &str,
    close_pane: bool,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<()> {
    launch.attach(pane, close_pane)?;
    ctx.store.update_head(task.id(), |task| {
        let parent = review_parent_head(task, &run.id)?;
        if let Some(record) = &mut parent.review {
            record.pane = Some(pane.into());
        }
        let record = parent.review.clone();
        if let Some(child) = parent.current_child.as_deref_mut() {
            child.review = record;
        }
        Ok(())
    })?;
    Ok(())
}

fn prompt_pm<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    review_id: &str,
    prompt_file: &Path,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<String> {
    let pane = match live_pm_pane(ctx)? {
        Some(p) => p,
        None => open_pm(ctx)?,
    };
    attach_pane(ctx, task, run, &pane, false, launch)?;
    let corral = corral_bin()?.display().to_string();
    let notes = prompt_file.with_file_name(NOTES);
    let prepared = ctx.store.head(task.id())?;
    let attempt = prepared
        .current_run
        .as_deref()
        .filter(|r| r.id == run.id)
        .and_then(|r| r.review.as_ref())
        .and_then(callback::token)
        .context("PM review attempt disappeared")?;
    let message = format!(
        "Review run {} of {} ({}): read the review prompt at {} and follow it. Do not edit files; finish with \
         `{corral} verdict {} approve|changes|escalate --attempt {} --notes-file {}`.",
        run.id,
        task.id(),
        task.title(),
        prompt_file.display(),
        task.id(),
        attempt,
        notes.display()
    );
    let info = ctx
        .herdr
        .pane_get(&pane)?
        .context("PM pane disappeared before review")?;
    // Save the exact request before contacting the composer, including on transport errors.
    ctx.store.update_head(task.id(), |t| {
        let snapshot = review_parent_head(t, &run.id)?
            .current_child
            .as_deref_mut()
            .filter(|r| r.id == review_id)
            .context("review changed before prompting PM")?;
        snapshot.pending_reply = Some(message.clone());
        snapshot.herdr = Some(HerdrRef {
            workspace_id: info.workspace_id.clone(),
            pane_id: pane.clone(),
            socket: Some(ctx.herdr.socket_path()?),
        });
        Ok(())
    })?;
    let delivery = submission::submit(ctx, task, run, launch, Some(&message), || {
        ctx.herdr.agent_prompt(&pane, &message)
    })
    .with_context(|| format!("PM review prompt retained for {}", task.id()))?;
    ctx.store.log_event(
        Some(task.id()),
        "review_prompt_delivery",
        json!({
            "run": run.id, "review_run": review_id, "pane": pane,
            "via": delivery.via, "submitted": delivery.submitted,
            "attempts": delivery.attempts, "error": delivery.error,
        }),
    )?;
    if !delivery.submitted {
        bail!(
            "PM review prompt was not submitted; retained for review-run {}: {}",
            task.id(),
            delivery
                .error
                .as_deref()
                .unwrap_or("submission was not confirmed")
        );
    }
    ctx.store.update_head(task.id(), |t| {
        if let Some(snapshot) = review_parent_head(t, &run.id)?
            .current_child
            .as_deref_mut()
            .filter(|r| r.id == review_id)
        {
            snapshot.pending_reply = None;
        }
        Ok(())
    })?;
    Ok(pane)
}

pub(super) fn review_run_origin(
    ctx: &Ctx,
    id: &str,
    reviewer: Option<&str>,
    origin: crate::operating::Origin,
    recovery: Option<&corral_core::RecoveryRecord>,
) -> Result<ReviewRecord> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    crate::recovery::require_no_pending(ctx, id, recovery)?;
    let reviewer_recovery = recovery.filter(|r| {
        matches!(
            r.preview.request.action,
            corral_core::RecoveryAction::UseReviewer { .. }
        )
    });
    let force_recovery = recovery.filter(|r| {
        matches!(
            r.preview.request.action,
            corral_core::RecoveryAction::ForceAdmission { .. }
        )
    });
    if let Some(record) = reviewer_recovery {
        crate::recovery::check_code_launch(
            ctx,
            &ctx.store.get(id)?,
            record,
            reviewer.context("recovery reviewer missing")?,
        )?;
    }
    if let Some(record) = force_recovery {
        crate::recovery::check_resource(
            ctx,
            id,
            record,
            corral_core::RecoveryAdmissionStage::CodeReview,
        )?;
    }
    ctx.store.check_runtime()?;
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    ctx.store.config.validate_review()?;
    let mut launch = crate::review_budget::ReviewLaunch::reserve_origin(
        ctx,
        id,
        crate::review_budget::Kind::Reviewer,
        force_recovery.is_some(),
        origin,
    )?;
    let automatic = matches!(origin, crate::operating::Origin::Automatic);
    if launch.restored() {
        let task = ctx.store.head(id)?;
        let run = current_run_head(&task)?;
        let routed = run.review.as_ref().and_then(|r| r.route.as_deref());
        let profiles: Vec<&str> = routed
            .map(|route| route.profile.as_str())
            .into_iter()
            .chain(
                run.current_child
                    .as_deref()
                    .map(|child| child.agent.as_str()),
            )
            .collect();
        instruction_guard::check(ctx, &task, &run, &profiles, automatic)?;
        launch.resume()?;
        let record = ctx
            .store
            .head(id)?
            .current_run
            .as_ref()
            .and_then(|run| run.review.clone())
            .context("resumed review missing")?;
        launch.commit()?;
        return Ok(record);
    }
    let task = ctx.store.head(id)?.detach();
    if task.status != Status::Review {
        bail!(
            "{} is {}; only Review tasks can be reviewed",
            task.id,
            task.status.label()
        );
    }
    let run = current_run_head(&task)?;
    let mut policy = ctx.store.config.task_review_policy(&task);
    let explicit = reviewer.is_some();
    let reviewer = crate::review_routing::choice(&ctx.store.config, &task, reviewer);
    if reviewer == Reviewer::CrossVendor {
        policy.require_cross_family = true;
    }
    let vendor = if policy.cross_family_required() {
        Some(implementer_vendor(ctx, &task, &run)?)
    } else {
        run.vendor.clone()
    };
    if !run.worktree.is_dir() {
        bail!("{}: worktree {} is gone", task.id, run.worktree.display());
    }
    let identity = match launch.bound_candidate() {
        Some(candidate) => candidate,
        None => verification::review_identity(
            ctx,
            &task,
            &run,
            run.gate
                .as_ref()
                .and_then(|g| g.identity.as_ref())
                .map(|i| i.attempt.as_str())
                .unwrap_or(&format!(
                    "review-{}",
                    jiff::Timestamp::now().as_nanosecond()
                )),
        )?,
    };
    let mut selected = read_step(ctx, id, &run.id, |read, task, current| {
        crate::review_contracts::implementation_route(
            read,
            task,
            current,
            &reviewer,
            explicit,
            vendor.as_deref(),
            &identity,
        )
    })?;
    selected.route.round = corral_core::implementation_review_round_ref(&task)?;
    selected.route.scope = corral_core::review_scope(selected.route.round);
    let (name, mut resolved) = (selected.name, selected.resolved);
    if reviewer != Reviewer::Pm && reviewer_recovery.is_none() {
        let exclude = crate::availability::review_exclusion(ctx, &task, &run);
        resolved =
            crate::availability::bind_review(ctx, &task, &run, resolved, exclude.as_deref())?;
    }
    if let Some(record) = reviewer_recovery {
        crate::recovery::check_reviewer(record, &resolved)?;
        if let Some(reason) = crate::routing::known_model_unavailability(ctx, &resolved) {
            bail!("confirmed reviewer unavailable: {reason}");
        }
    } else {
        crate::review_contracts::check_family(&policy, &run, vendor.as_deref(), &resolved)?;
    }
    crate::agent::hooks::review::preflight(ctx, &resolved)?;
    instruction_guard::check(
        ctx,
        &task,
        &run,
        &[selected.route.profile.as_str(), resolved.profile.as_str()],
        automatic,
    )?;
    if !corral_git::is_clean(&run.worktree)? {
        bail!(
            "{}: worktree {} has uncommitted changes; commit them so the review belongs to a commit",
            task.id,
            run.worktree.display()
        );
    }
    let sha = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let mut record = ReviewRecord {
        cancelled: false,
        route: Some(Box::new(selected.route)),
        evidence_digest: Some(verification::evidence_digest(&run)?),
        identity: Some(identity),
        verdict: None,
        process_lost: false,
        reviewer: name.clone(),
        notes: None,
        sha: sha.clone(),
        at: corral_core::clock::now(),
        pane: None,
    };
    // The launch guard's read set: the parent's child count and its current review record.
    let children = task
        .current_run
        .as_ref()
        .map_or(0, |run| run.facts.children);
    // A failed, unrecorded launch may have left a closed or reclaimed registration.
    // Allocate a new identity only if no review child was ever recorded for the old one.
    let bound = launch.bound_attempt();
    let review_id = read_step(ctx, id, &run.id, |read, head, _| {
        let mut number = children + 1;
        let mut candidate = bound
            .clone()
            .unwrap_or_else(|| format!("{}-review{number}", run.id));
        loop {
            let scratch = crate::scratch_reclaim::Attempt::Review {
                task: &task.id,
                run: &run.id,
                child: &candidate,
            };
            let row = read.store.scratch_row_viewed(&scratch.id())?;
            let recorded = head.review_child(&run.id, &candidate)?.is_some();
            if (recorded && bound.as_deref() == Some(&candidate))
                || (!recorded
                    && row.as_ref().is_none_or(|row| {
                        row.state == corral_core::backend::ScratchState::Open
                            && row.kind == corral_core::backend::ScratchKind::ImplementationReview
                            && row.task.as_deref() == Some(&task.id)
                            && row.subject.as_deref() == Some(&run.id)
                            && row.attempt.as_deref() == Some(&candidate)
                            && row.path == format!("reviews/{}-{candidate}", task.id)
                    }))
            {
                return Ok(candidate);
            }
            number += 1;
            candidate = format!("{}-review{number}", run.id);
        }
    })?;
    if let Some(recovery) = reviewer_recovery {
        anyhow::ensure!(
            matches!(&recovery.preview.proposed_effect, corral_core::RecoveryEffect::ReviewAttempt { attempt, .. } if attempt == &review_id),
            "recovery_stale: next review attempt changed"
        );
    }
    launch.bind(
        record
            .identity
            .as_ref()
            .context("missing reviewer identity")?,
        &review_id,
        Some(&review_id),
    )?;
    crate::watch::capture_review(ctx, &task, &run);
    let inherited = callback::inherited(&run, record.identity.as_ref());
    if let Some(route) = record.route.as_mut() {
        route.attempt = Some(review_id.clone());
        route.callback = Some(inherited.unwrap_or_else(|| review_id.clone()));
    }
    let route = record.route.as_deref().context("review route missing")?;
    let key = dir_attempt(Some(&record), &review_id);
    let dir = crate::scratch_reclaim::Attempt::Review {
        task: &task.id,
        run: &run.id,
        child: &key,
    }
    .open(ctx)?;
    let packet = read_step(ctx, id, &run.id, |read, head, current| {
        crate::review_packet::build(
            read,
            head,
            current,
            (&review_id, route),
            &record,
            crate::review_packet::findings_of(head, &review_id)?,
            None,
        )
    })?;
    let (packet_digest, packet_section) =
        crate::review_packet::write_built(ctx, &task, &run, (&review_id, route), packet)?;
    if let Some(route) = record.route.as_mut() {
        route.packet_digest = Some(packet_digest);
    }
    if let Some(prev) = &run.review
        && prev.reviewer != PM
        && let Some(p) = prev.pane.as_deref()
        && pane_alive(ctx, p)?
    {
        ctx.herdr.pane_close(p)?;
    }
    if let Some(p) = stronger::superseded_pane(&run)
        && pane_alive(ctx, p)?
    {
        ctx.herdr.pane_close(p)?;
    }
    let prompt_file = dir.join("review.md");
    let notes = dir.join(NOTES);
    match std::fs::remove_file(&notes) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("clearing replaced reviewer notes"),
    }
    // Persist the window and active record before dispatch: the agent can answer immediately.
    let mut child = review_child(ctx, &run, &resolved, &reviewer, &review_id, &record)?;
    if reviewer != Reviewer::Pm {
        child.session = Some(crate::role_session::reviewer_head(
            ctx,
            &ctx.store.head(id)?,
            &run,
            &child,
        )?);
    }
    let previous_session = run.current_child.as_deref().and_then(|previous| {
        let container = corral_core::agent::ContainerRef::Reviewer {
            task: task.id.clone(),
            run: run.id.clone(),
            review: previous.id.clone(),
        };
        ctx.store
            .head(id)
            .and_then(|head| crate::role_session::capture_head(ctx, &head, previous, &container))
            .ok()
    });
    ctx.store.submission_effect_barrier("review_child_observed");
    let saved = ctx.store.update_head(&task.id, |t| {
        if let Some(recovery) = recovery {
            crate::recovery::validate_contract(ctx, &ctx.store.get(id)?, recovery)?;
        }
        let r = review_parent_head(t, &run.id)?;
        if r.facts.children != children || r.review != run.review {
            bail!("review changed before preparing accounting child");
        }
        // The finished child's usage is owed to the next usage refresh (`usage_pending`).
        history::finish_review_head(r, record.at);
        r.idle_since = None;
        r.blocked_excerpt = None;
        if let Some(previous) = r.current_child.as_deref_mut() {
            previous.pending_reply = None;
            if let Some(session) = &previous_session {
                previous.session = Some(session.clone());
            }
        }
        t.push_review_child(&run.id, child)?;
        review_parent_head(t, &run.id)?.review = Some(record.clone());
        Ok(())
    })?;
    let saved = saved.detach();
    let saved_run = current_run_head(&saved)?;
    let saved_child = saved_run
        .current_child
        .as_deref()
        .context("review snapshot was not saved")?;
    let review_id = saved_child.id.clone();
    crate::watch::start_review(ctx, &saved, saved_child)?;
    let (prompt, notice) = prepare_prompt(ctx, &saved, &saved_run)?;
    let mut text = prompt + &packet_section + &crate::scratch_reclaim::prompt_section(ctx, &dir);
    if let Some(notice) = notice {
        text = format!("{notice}\n\n{text}");
    }
    std::fs::write(&prompt_file, text)
        .with_context(|| format!("writing {}", prompt_file.display()))?;
    if let Some(recovery) = recovery {
        launch.recovery_journal(recovery)?;
    }
    let launched = match &reviewer {
        Reviewer::Profile(_) | Reviewer::CrossVendor | Reviewer::Auto => {
            launch_profile(ctx, &task, &run, &resolved, &prompt_file, &mut launch)
        }
        Reviewer::Pm => prompt_pm(ctx, &task, &run, &review_id, &prompt_file, &mut launch),
        Reviewer::None => bail!("no reviewer configured"),
    };
    if let Err(error) = &launched
        && crate::operating::refused(error)
    {
        return Err(launched.err().context("review refusal disappeared")?);
    }
    let saved = ctx.store.update_head(&task.id, |t| {
        let r = review_parent_head(t, &run.id)?;
        // Never overwrite an immediate verdict, or a replacement review's record.
        if r.current_child.as_deref().is_some_and(|s| s.id == review_id) {
            match &launched {
                Ok(pane) => {
                    if let Some(record) = &mut r.review {
                        record.pane = Some(pane.clone());
                    }
                }
                Err(_) => {
                    history::finish_review_head(r, corral_core::clock::now());
                    if r.review.as_ref().is_some_and(|r| r.verdict.is_none()) {
                        r.review = None;
                    }
                }
            }
        }
        if let Ok(pane) = &launched {
            crate::role_session::publish_reviewer(ctx, t, &run.id)?;
            ctx.store.log_event(
                Some(&task.id),
                "review_started",
                json!({ "run": run.id, "reviewer": name, "pane": pane, "sha": sha, "prompt": prompt_file }),
            )?;
        }
        Ok(())
    })?;
    launched?;
    let record = saved
        .current_run
        .as_ref()
        .and_then(|r| r.review.clone())
        .context("review record was not saved")?;
    launch.commit()?;
    if reviewer == Reviewer::CrossVendor {
        let switch = ProfileSwitch {
            from: run.agent.clone(),
            to: name.clone(),
            reason: "cross-vendor review".into(),
        };
        ctx.store.update_head(&task.id, |t| {
            if t.current_run_head()?.is_none_or(|r| r.id != run.id) {
                bail!("run changed before recording reviewer selection");
            }
            t.push_switch(&run.id, switch.clone())?;
            ctx.store.log_event(
                Some(&task.id),
                "profile_switched",
                json!({
                    "run": run.id, "from": switch.from, "to": switch.to, "reason": switch.reason,
                }),
            )?;
            Ok(())
        })?;
    }
    Ok(record)
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
