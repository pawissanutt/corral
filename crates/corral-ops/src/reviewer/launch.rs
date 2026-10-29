//! The reviewer launch (plan T-253 section 4, "reviewer"): the acted task is read as its head,
//! helpers get its fields and current run as a task ([`review_shell`]), earlier review children
//! are reached only by key, and the launch guard compares the parent's `children` fact and its
//! current review record: the full form's child count and record.
use super::*;
use corral_core::{RunHead, TaskHeadMut};

/// The head's fields and current run as a task, with the run's open obligations, its own
/// artifacts and, ahead of the current child, the superseded child that consumed this
/// candidate's stronger review: the rows the review checks and the packet read.
pub(crate) fn review_shell<T: corral_core::TaskRef + ?Sized>(head: &T) -> Result<Task> {
    let mut task = head.shell_task()?;
    let Some(run) = task.runs.last_mut() else {
        return Ok(task);
    };
    run.obligations = head.open_obligations(&run.id)?;
    with_stronger_source(head, run)?;
    let id = run.id.clone();
    task.artifacts = head.artifacts_of(&id)?;
    Ok(task)
}

/// Inserts ahead of `run`'s current child the earlier superseded child that carries a stronger
/// request on the run's head, read by key: `stronger_source` then finds it as over all children.
fn with_stronger_source<T: corral_core::TaskRef + ?Sized>(head: &T, run: &mut Run) -> Result<()> {
    let Ok(sha) = corral_git::rev_sha(&run.worktree, "HEAD") else {
        return Ok(());
    };
    let current = run.review_runs.last().map(|child| child.id.clone());
    if let Some(source) = head.stronger_source(&run.id, &sha, current.as_deref())?
        && let Some(child) = head.review_child(&run.id, &source)?
    {
        run.review_runs
            .insert(0, serde_json::from_value(serde_json::to_value(child)?)?);
    }
    Ok(())
}

fn review_parent_head<'a>(task: &'a mut TaskHeadMut, run_id: &str) -> Result<&'a mut RunHead> {
    task.current_run_mut()
        .filter(|r| r.id == run_id)
        .ok_or_else(|| anyhow!("run {run_id} is no longer current"))
}

/// The accounting child a launch records for its review.
fn review_child(
    ctx: &Ctx,
    run: &Run,
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

fn launch_profile(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    resolved: &ResolvedProfile,
    prompt_file: &Path,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<String> {
    let title = format!("{} review", task.id);
    let command = match crate::agent::hooks::review::command(ctx, task, run, resolved) {
        Some(command) => command?,
        None => render_template(
            resolved,
            &[
                ("prompt_file", prompt_file.display().to_string()),
                ("name", title.clone()),
                ("task", task.id.clone()),
                ("worktree", run.worktree.display().to_string()),
            ],
        )?,
    };
    let herdr = open_run_pane(ctx, task, run, "review")?;
    attach_pane(ctx, task, run, &herdr.pane_id, true, launch)?;
    let mut child = String::new();
    ctx.store.update_head(&task.id, |task| {
        let review = review_parent_head(task, &run.id)?
            .current_child
            .as_deref_mut()
            .context("review child disappeared before launch")?;
        review.herdr = Some(herdr.clone());
        child = review.id.clone();
        Ok(())
    })?;
    let receipt = crate::launch_exit::receipt(ctx, &task.id, &format!("{}-{child}", run.id))?;
    let command = crate::launch_exit::arm(ctx, &command, &receipt)?;
    let pane = herdr.pane_id;
    ctx.herdr
        .pane_report_metadata(&pane, &title, &[("task", task.id.as_str())])?;
    submission::submit(ctx, task, run, launch, None, || {
        ctx.herdr.pane_run(&pane, &command)
    })?;
    Ok(pane)
}

fn attach_pane(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    pane: &str,
    close_pane: bool,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<()> {
    launch.attach(pane, close_pane)?;
    ctx.store.update_head(&task.id, |task| {
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

fn prompt_pm(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
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
    let prepared = ctx.store.head(&task.id)?;
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
        task.id,
        task.title,
        prompt_file.display(),
        task.id,
        attempt,
        notes.display()
    );
    let info = ctx
        .herdr
        .pane_get(&pane)?
        .context("PM pane disappeared before review")?;
    // Save the exact request before contacting the composer, including on transport errors.
    ctx.store.update_head(&task.id, |t| {
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
    .with_context(|| format!("PM review prompt retained for {}", task.id))?;
    ctx.store.log_event(
        Some(&task.id),
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
            task.id,
            delivery
                .error
                .as_deref()
                .unwrap_or("submission was not confirmed")
        );
    }
    ctx.store.update_head(&task.id, |t| {
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
) -> Result<ReviewRecord> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    ctx.store.check_runtime()?;
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    ctx.store.config.validate_review()?;
    let mut launch = crate::review_budget::ReviewLaunch::reserve_origin(
        ctx,
        id,
        crate::review_budget::Kind::Reviewer,
        false,
        origin,
    )?;
    let automatic = matches!(origin, crate::operating::Origin::Automatic);
    if launch.restored() {
        let task = review_shell(&ctx.store.head(id)?)?;
        let run = current_run(&task)?;
        let routed = run.review.as_ref().and_then(|r| r.route.as_deref());
        let profiles: Vec<&str> = routed
            .map(|route| route.profile.as_str())
            .into_iter()
            .chain(run.review_runs.last().map(|child| child.agent.as_str()))
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
    let head = ctx.store.head(id)?;
    let task = review_shell(&head)?;
    if task.status != Status::Review {
        bail!(
            "{} is {}; only Review tasks can be reviewed",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
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
    let mut selected = crate::review_contracts::implementation_route(
        ctx,
        &task,
        &run,
        &reviewer,
        explicit,
        vendor.as_deref(),
        &identity,
    )?;
    selected.route.round = corral_core::implementation_review_round_ref(&head)?;
    selected.route.scope = corral_core::review_scope(selected.route.round);
    let (name, mut resolved) = (selected.name, selected.resolved);
    if reviewer != Reviewer::Pm {
        let exclude = crate::availability::review_exclusion(ctx, &task, &run);
        resolved =
            crate::availability::bind_review(ctx, &task, &run, resolved, exclude.as_deref())?;
    }
    policy.check_reviewer_family(vendor.as_deref(), resolved.vendor.as_deref())?;
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
    let children = head
        .current_run
        .as_ref()
        .map_or(0, |run| run.facts.children);
    let review_id = launch
        .bound_attempt()
        .unwrap_or_else(|| format!("{}-review{}", run.id, children + 1));
    launch.bind(
        record
            .identity
            .as_ref()
            .context("missing reviewer identity")?,
        &review_id,
        Some(&review_id),
    )?;
    crate::watch::capture_review(ctx, &task, &run);
    let captured_head = ctx.store.head(id)?;
    let captured = review_shell(&captured_head)?;
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
    let (packet_digest, packet_section) = crate::review_packet::write_with(
        ctx,
        &captured,
        &current_run(&captured)?,
        &review_id,
        &record,
        route,
        crate::review_packet::findings_of(&captured_head, &review_id)?,
    )?;
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
    let child = review_child(ctx, &run, &resolved, &reviewer, &review_id, &record)?;
    ctx.store.submission_effect_barrier("review_child_observed");
    let saved = ctx.store.update_head(&task.id, |t| {
        let r = review_parent_head(t, &run.id)?;
        if r.facts.children != children || r.review != run.review {
            bail!("review changed before preparing accounting child");
        }
        let finished = r.current_child.as_ref().map(|child| child.id.clone());
        history::finish_review_head(r, record.at);
        r.idle_since = None;
        r.blocked_excerpt = None;
        if let Some(previous) = r.current_child.as_deref_mut() {
            previous.pending_reply = None;
        }
        t.push_review_child(&run.id, child)?;
        if let Some(finished) = &finished {
            history::collect_usage(ctx, t, &run.id, finished)?;
        }
        review_parent_head(t, &run.id)?.review = Some(record.clone());
        Ok(())
    })?;
    let saved_task = review_shell(&saved)?;
    let review_id = saved_task
        .current_run()
        .and_then(|r| r.review_runs.last())
        .context("review snapshot was not saved")?
        .id
        .clone();
    crate::watch::start_review(
        ctx,
        &saved_task,
        saved_task
            .current_run()
            .and_then(|r| r.review_runs.last())
            .context("review accounting child disappeared")?,
    )?;
    let mut prompt_task = saved_task.clone();
    prompt_task.comments = crate::comments::emitted_head(&saved)?;
    std::fs::write(
        &prompt_file,
        review_prompt(
            ctx,
            &prompt_task,
            prompt_task
                .current_run()
                .context("review run disappeared")?,
        )? + &packet_section
            + &crate::scratch_reclaim::prompt_section(ctx, &dir),
    )
    .with_context(|| format!("writing {}", prompt_file.display()))?;
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
