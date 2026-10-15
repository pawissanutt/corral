//! Separate plan review attempts. These never populate implementation review receipts.
use anyhow::{Context, Result, bail};
use corral_core::{HerdrRef, PlanRecord, PlanReview, ProfileChoice, Task, Verdict};
use jiff::Timestamp;
use serde_json::json;

use crate::{Ctx, plan, prompt};

fn current<'a>(task: &'a mut Task, id: &str) -> Result<&'a mut PlanRecord> {
    task.plans
        .last_mut()
        .filter(|p| p.id == id)
        .context("stale plan verdict: plan was replaced; inspect `corral plan show <id> --json`")
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
    let plan_id = selected.id.clone();
    let previous = selected.clone();
    let at = Timestamp::now();
    let attempt = format!("PR{}", selected.reviews.len() + 1);
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
         Read the complete snapshot and relevant repository contracts. Review goal/non-goals, decisions, \
         paths/contracts, dependencies/write scopes, acceptance scenarios and verification commands. \
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
    // Publish before sending: a synchronous reviewer can record its verdict immediately.
    ctx.store.update(&task.id, |t| {
        crate::session_scope::check_launch(ctx)?;
        let revision = plan::plan_spec_revision(t)?;
        let p = current(t, &plan_id)?;
        if p != selected || p.spec_revision != revision {
            bail!("plan changed before review launch");
        }
        std::fs::write(&prompt_file, text).context("writing plan review prompt")?;
        p.reviews.push(PlanReview {
            id: attempt.clone(),
            reviewer: name.clone(),
            policy_digest: plan::policy_digest(&config, &task)?,
            verdict: None,
            notes: None,
            at,
            herdr: None,
            pending_reply: Some(format!("Review the plan prompt at {}. Follow its exact verdict command.", prompt_file.display())),
        });
        ctx.store.log_event(Some(&t.id), "plan_review_requested",
            json!({"target":"plan","plan":plan_id,"attempt":attempt,"reviewer":name,"prompt":prompt_file}))?;
        Ok(())
    })?;
    close_plan_review(ctx, &previous)?;
    let launch = || -> Result<HerdrRef> {
        let herdr = if name == "pm" {
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
        let submitted = if name == "pm" {
            let delivery = ctx.herdr.agent_prompt(&herdr.pane_id, &format!(
                "Review the plan prompt at {}. Follow its exact plan verdict command, including --plan and --attempt.",
                prompt_file.display()
            ))?;
            ctx.store.log_event(
                Some(&task.id),
                "plan_review_delivery",
                json!({"target":"plan","plan":plan_id,"attempt":attempt,"via":delivery.via,
                    "submitted":delivery.submitted,"error":delivery.error}),
            )?;
            if !delivery.submitted {
                bail!("PM plan prompt was not submitted; retained for inspection");
            }
            Ok(())
        } else {
            let command = prompt::render_template(
                &resolved,
                &[
                    ("prompt_file", prompt_file.display().to_string()),
                    ("name", format!("{} plan review", task.id)),
                    ("task", task.id.clone()),
                    ("worktree", ctx.store.root().display().to_string()),
                ],
            )?;
            ctx.herdr.pane_run(&herdr.pane_id, &command)
        };
        submitted?;
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
        let mut notes = input.notes.to_string();
        let verdict = match input.verdict {
            Verdict::Approve if reviewer != plan::expected_reviewer(&config, task)?
                || plan::owner_decisions(task, selected) => {
                notes.push_str("\nEscalated: unauthorized reviewer or unresolved owner decision.");
                Verdict::Escalate
            }
            Verdict::Approve => Verdict::Approve,
            Verdict::Changes => Verdict::Changes,
            Verdict::Escalate => Verdict::Escalate,
        };
        // An old queued intent must not turn approval into an automatic implementation launch.
        if required {
            task.queued = false;
        }
        let p = current(task, input.plan)?;
        match input.attempt {
            Some(_) => {
                let review = p.reviews.last_mut().context("plan review missing")?;
                review.verdict = Some(verdict);
                review.notes = Some(notes);
                review.at = Timestamp::now();
            }
            None => p.reviews.push(PlanReview {
                id: format!("PR{}", p.reviews.len() + 1),
                reviewer,
                policy_digest: digest,
                verdict: Some(verdict),
                notes: Some(notes),
                at: Timestamp::now(),
                herdr: None,
                pending_reply: None,
            }),
        }
        ctx.store.log_event(Some(&task.id), "plan_verdict",
            json!({"target":"plan","plan":input.plan,"review":task.plans.last().and_then(|p|p.reviews.last())}))?;
        Ok(())
    })?;
    let p = saved.plans.last().context("plan disappeared")?;
    if let Some(previous) = superseded {
        close_plan_review(ctx, &previous)?;
    }
    close_plan_review(ctx, p)?;
    p.reviews.last().cloned().context("plan verdict missing")
}
