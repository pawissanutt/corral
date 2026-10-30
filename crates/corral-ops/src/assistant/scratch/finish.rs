//! Co-work finish (T-544 cowork-landing). On the owner's instruction a scratch becomes a task:
//! corral materializes task `T-<m>` (origin assistant, label `cowork`) or, with `into`, a new run
//! of an existing task, whose worktree and branch are the scratch's own. It then runs the gate
//! and, when the owner asked for review, the normal reviewer. Landing stays the ordinary
//! `approve`; a landed co-work run is recorded on its session.
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::assistant::{AssistantLanding, AssistantScratch, validate_id};
use corral_core::{
    ArtifactKind, Cowork, CoworkReview, GateRules, Origin, Run, RunOutcome, Status, Task, TaskKind,
    TaskSize, Usage,
};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use super::{COWORK, find, git, holder, lock, repo_of, validate_slug};
use crate::Ctx;
use crate::assistant::owner_action::{self, OwnerAuthority};

/// The label every co-work task carries.
pub const LABEL: &str = "cowork";

/// What the owner's finish instruction names: a new task's title and classification, or the
/// existing task that gets a new run, and the review choice (default none).
#[derive(Debug, Clone, Default)]
pub struct Finish<'a> {
    pub title: Option<&'a str>,
    pub project: Option<&'a str>,
    pub kind: Option<TaskKind>,
    pub size: Option<TaskSize>,
    pub review: Option<CoworkReview>,
    pub into: Option<&'a str>,
    pub note: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finished {
    pub task: String,
    pub run: String,
    pub session: String,
    pub slug: String,
    pub branch: String,
    pub head: String,
    pub review: CoworkReview,
    /// The gate's result (`pass`, `fail`), `none` when the project has no gate, or why it did not run.
    pub gate: String,
    /// For `review: required`: the reviewer started, or why none started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    pub status: Status,
}

const REFUSED: &str = "assistant_refused: corral assistant scratch finish needs the owner; use corral_owner_action or corral pm inbox post";

/// The session a finish acts for: a claimed owner action's own session, else the session the
/// owner names. Finishing is an owner action: an assistant, agent or PM shell is refused.
pub fn session_for(named: Option<&str>) -> Result<String> {
    let session = match owner_action::authority() {
        OwnerAuthority::ViaAssistant(via) => match named {
            Some(named) if named != via.session => bail!(
                "not_own_scratch: {} finishes only its own scratch worktrees, not {named}'s; nothing changed",
                via.session
            ),
            _ => via.session.clone(),
        },
        OwnerAuthority::Shell => {
            if crate::automation_controls::agent_entry()
                || std::env::var("CORRAL_PM").as_deref() == Ok("1")
            {
                bail!("{REFUSED}");
            }
            named
                .context("--session A-<n> is required outside an assistant's owner action")?
                .to_owned()
        }
    };
    validate_id(&session)?;
    Ok(session)
}

/// Refuses a target task that cannot take a new run now; `None` when it can.
fn busy(task: &Task) -> Option<String> {
    if let Some(run) = task.current_run().filter(|run| run.outcome.is_none()) {
        return Some(format!("its run {} is live", run.id));
    }
    let reviewing = task.review_reservation.is_some()
        || task.current_run().is_some_and(|run| {
            run.review_runs
                .last()
                .is_some_and(|review| review.outcome.is_none())
        });
    if reviewing {
        return Some("a review of it is running".into());
    }
    if task.queued || task.launch.is_some() || task.holds_dispatch_slot() {
        return Some("it is queued, launching or holds a slot".into());
    }
    None
}

fn check_into(task: &Task, project: &str) -> Result<()> {
    if let Some(why) = busy(task) {
        bail!(
            "task_busy: {} has a live run ({why}); finish into it once that run ended, or as a new task",
            task.id
        );
    }
    if task.tracker || crate::plan_pilot::handles(task) {
        bail!("{} is a Plan; finish into one of its members", task.id);
    }
    if !matches!(
        task.status,
        Status::Backlog | Status::Ready | Status::NeedsMe | Status::Review
    ) {
        bail!(
            "task_status: {} is {}; a finish adds runs only to Backlog, Ready, Needs me or Review tasks",
            task.id,
            task.status.label()
        );
    }
    if task.project.as_deref() != Some(project) {
        bail!(
            "project_mismatch: {} belongs to {}, the scratch to {project}",
            task.id,
            task.project.as_deref().unwrap_or("no project")
        );
    }
    Ok(())
}

/// The session's usage between `from` and `to`; entries without a response time are left out.
fn usage_between(
    ctx: &Ctx,
    session: &str,
    from: Timestamp,
    to: Timestamp,
) -> (Option<Usage>, Option<String>) {
    let measured = ctx
        .store
        .get_assistant_container(session)
        .and_then(|container| {
            crate::agent::usage::collect_state(&container.state, None, &ctx.store)
        });
    match measured {
        Ok(usage) => {
            let entries: Vec<_> = (usage.entries.into_iter())
                .filter(|entry| entry.at.is_some_and(|at| from <= at && at <= to))
                .collect();
            if entries.is_empty() {
                let note =
                    format!("{session} recorded no usage between scratch creation and finish");
                (None, Some(note))
            } else {
                (Some(Usage::from_entries(entries)), None)
            }
        }
        Err(error) => (
            None,
            Some(format!("{session} usage unavailable: {error:#}")),
        ),
    }
}

/// The scratch's branch, rebased onto the project's base when that moved, and its head.
fn rebased(
    ctx: &Ctx,
    project: &str,
    repo: &Path,
    scratch: &AssistantScratch,
) -> Result<(String, String, String)> {
    let base = match ctx
        .store
        .config
        .projects
        .get(project)
        .and_then(|p| p.base.clone())
    {
        Some(base) => base,
        None => corral_git::current_branch(repo)?,
    };
    let base_sha = corral_git::rev_sha(repo, &base)?;
    let head = corral_git::rev_sha(&scratch.path, "HEAD")?;
    if !corral_git::is_ancestor(repo, &base_sha, &head)?
        && let corral_git::RebaseOutcome::Conflict { paths, .. } =
            corral_git::rebase(&scratch.path, &base_sha)?
    {
        bail!(
            "rebase_conflict: rebasing {} onto {base} conflicts in {}; resolve it in {} and finish again; nothing finished",
            scratch.branch,
            paths.join(", "),
            scratch.path.display()
        );
    }
    let head = corral_git::rev_sha(&scratch.path, "HEAD")?;
    Ok((base, base_sha, head))
}

struct Draft<'a> {
    session: &'a str,
    profile: &'a str,
    scratch: &'a AssistantScratch,
    base: &'a str,
    base_sha: &'a str,
    summary: &'a str,
    review: CoworkReview,
    now: Timestamp,
    usage: (Option<Usage>, Option<String>),
}

impl Draft<'_> {
    fn apply(&self, ctx: &Ctx, task: &mut Task) -> Result<String> {
        let id = format!("R{}", task.runs.len() + 1);
        let mut run: Run = serde_json::from_value(json!({
            "id": id, "agent": self.profile, "worktree": self.scratch.path,
            "branch": self.scratch.branch, "base": self.base, "base_sha": self.base_sha,
            "started": self.scratch.created,
        }))
        .context("building the co-work run")?;
        let policy = (ctx.store.config).review_policy(task.project.as_deref(), task.review);
        if !policy.gate_tiers.is_empty() {
            run.gate_rules = GateRules::Tiered;
        }
        run.completion.manifest = task.completion.clone();
        run.completion.required_checks = Some(crate::gate_tier::required_checks(ctx, task, &run)?);
        run.summary = Some(self.summary.to_owned());
        run.ended = Some(self.now);
        run.outcome = Some(RunOutcome::Done);
        run.agent_state = Some("done".into());
        (run.usage, run.usage_note) = self.usage.clone();
        task.runs.push(run);
        task.cowork = Some(Cowork {
            session: self.session.to_owned(),
            scratch: self.scratch.slug.clone(),
            run: id.clone(),
            review: self.review,
        });
        if !task.labels.iter().any(|label| label == LABEL) {
            task.labels.push(LABEL.into());
        }
        task.status = Status::Review;
        task.queued = false;
        Ok(id)
    }
}

/// Finishes `session`'s scratch `slug` on the owner's instruction. Refusals before the task
/// exists change nothing: the scratch must hold at least one commit, no uncommitted work, and
/// rebase cleanly onto the project's base.
pub fn finish(ctx: &Ctx, session: &str, slug: &str, opts: &Finish<'_>) -> Result<Finished> {
    validate_id(session)?;
    validate_slug(slug)?;
    let _guard = lock(ctx, &format!("scratch-{session}-{slug}"))?;
    let scratch = find(ctx, session, slug)?;
    if let Some(holder) = holder(&ctx.store.list()?, &scratch) {
        bail!("scratch_attached: {session}/{slug} already belongs to {holder}; nothing finished");
    }
    let repo = repo_of(&scratch)?;
    let project = opts.project.unwrap_or(&scratch.project);
    if ctx
        .store
        .project_path(project)?
        .canonicalize()
        .ok()
        .as_deref()
        != Some(repo.as_path())
    {
        bail!("project_mismatch: project {project} is not the repository of {session}/{slug}");
    }
    let work = git::work(&repo, &scratch)?;
    if work.dirty {
        bail!(
            "scratch_dirty: {} has uncommitted changes; commit or remove them, then finish; nothing finished",
            scratch.path.display()
        );
    }
    if work.commits.is_empty() {
        bail!("scratch_empty: {session}/{slug} has no commit beyond its base; nothing finished");
    }
    let into = match opts.into {
        Some(id) => {
            let task = ctx.store.get(id)?;
            check_into(&task, project)?;
            Some(task.id)
        }
        None => None,
    };
    let (base, base_sha, head) = rebased(ctx, project, &repo, &scratch)?;
    let record = ctx.store.get_assistant(session)?;
    let words = match owner_action::authority() {
        OwnerAuthority::ViaAssistant(via) => format!(
            "on the owner's instruction:\n\n> {}",
            via.owner_message.trim().replace('\n', "\n> ")
        ),
        OwnerAuthority::Shell => "by the owner from the shell.".to_owned(),
    };
    let review = opts.review.unwrap_or(CoworkReview::None);
    let mut summary = format!(
        "Co-work finish of assistant scratch {session}/{slug} (branch `{}`, head `{head}`) {words}\n\nReview: {}.",
        scratch.branch,
        match review {
            CoworkReview::None => "none",
            CoworkReview::Required => "required",
        }
    );
    if let Some(note) = opts.note.filter(|note| !note.trim().is_empty()) {
        summary.push_str(&format!("\n\n{}", note.trim()));
    }
    let now = Timestamp::now();
    let draft = Draft {
        session,
        profile: &record.profile,
        scratch: &scratch,
        base: &base,
        base_sha: &base_sha,
        summary: &summary,
        review,
        now,
        usage: usage_between(ctx, session, scratch.created, now),
    };
    let mut run = String::new();
    let task = match &into {
        Some(id) => ctx.store.update(id, |task| {
            check_into(task, project)?;
            run = draft.apply(ctx, task)?;
            Ok(())
        })?,
        None => {
            let title = opts
                .title
                .context("a finish into a new task needs --title")?;
            let mut body = record.body.trim().to_owned();
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(&summary);
            body.push('\n');
            crate::new_task_initialized(
                ctx,
                title,
                Some(project),
                ctx.store.root(),
                Status::Review,
                &body,
                |task| {
                    task.origin = Some(Origin::Assistant);
                    task.kind = opts.kind.map(|kind| kind.to_string());
                    task.size = opts.size;
                    run = draft.apply(ctx, task)?;
                    Ok(())
                },
            )?
        }
    };
    ctx.store.update_assistant(session, |s| {
        if let Some(entry) = s.scratch.iter_mut().find(|entry| entry.slug == slug) {
            entry.kind = COWORK.into();
        }
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "cowork_finished",
        json!({"session": session, "slug": slug, "run": run, "branch": scratch.branch,
            "head": head, "base": base, "review": review, "into": into.is_some()}),
    )?;
    snapshot_diff(ctx, &task.id, &scratch.path, &base_sha)?;
    let gate = gate(ctx, &task.id);
    let reviewer = (review == CoworkReview::Required).then(|| match gate.as_str() {
        "pass" | "none" => match crate::review_run(ctx, &task.id, None) {
            Ok(record) => record.reviewer,
            Err(error) => format!("not started: {error:#}; run `corral review-run {}`", task.id),
        },
        _ => format!("not started: the gate did not pass; fix it in the scratch, then `corral gate {0}` and `corral review-run {0}`", task.id),
    });
    Ok(Finished {
        status: ctx.store.get(&task.id)?.status,
        task: task.id,
        run,
        session: session.to_owned(),
        slug: slug.to_owned(),
        branch: scratch.branch,
        head,
        review,
        gate,
        reviewer,
    })
}

fn snapshot_diff(ctx: &Ctx, task: &str, worktree: &Path, base: &str) -> Result<()> {
    let stat = corral_git::diff_stat(worktree, base)?;
    let diff = corral_git::diff_text(worktree, base)?;
    let temp = tempfile::tempdir().context("creating co-work diff directory")?;
    let file = temp.path().join("final.diff");
    std::fs::write(&file, diff).context("writing co-work diff")?;
    let title = format!("Final diff +{} \u{2212}{}", stat.added, stat.removed);
    crate::artifacts::add_artifact(ctx, task, &file, Some(ArtifactKind::Diff), Some(&title))?;
    Ok(())
}

/// Runs the configured gate to its end. The task exists either way, so a gate that cannot run
/// is reported, as for workers, rather than undoing the finish.
fn gate(ctx: &Ctx, task: &str) -> String {
    let configured = ctx
        .store
        .get(task)
        .and_then(|task| crate::verification::policy(ctx, &task));
    match configured {
        Ok(policy) if policy.gate.is_empty() => "none".into(),
        Ok(_) => match crate::gate::gate_start(ctx, task)
            .and_then(|_| crate::gate::gate_wait(ctx, task))
        {
            Ok(record) => record.result.as_str().into(),
            Err(error) => format!("not run: {error:#}; run `corral gate {task} --wait`"),
        },
        Err(error) => format!("not run: {error:#}"),
    }
}

/// After `approve` landed a co-work run: its session lists the landed task, run and commits,
/// and forgets the scratch whose lifecycle the task run took over. The landing is durable
/// already, so a failure here is reported, never returned.
pub(crate) fn record_landing(ctx: &Ctx, task: &Task, run: &Run, previous: Option<&str>, sha: &str) {
    let Some(cowork) = task.cowork.as_ref().filter(|cowork| cowork.run == run.id) else {
        return;
    };
    let recorded = (|| -> Result<()> {
        let repo = crate::review::main_repo(ctx, task, run)?;
        let commits = match previous {
            Some(previous) => git::range(&repo, previous, sha)?,
            None => Vec::new(),
        };
        let landing = AssistantLanding {
            task: task.id.clone(),
            run: run.id.clone(),
            base: run.base.clone(),
            sha: sha.to_owned(),
            commits,
            at: Timestamp::now(),
        };
        ctx.store.update_assistant(&cowork.session, |session| {
            session.scratch.retain(|entry| entry.branch != run.branch);
            if !session
                .landed
                .iter()
                .any(|l| l.task == landing.task && l.run == landing.run)
            {
                session.landed.push(landing.clone());
            }
            Ok(())
        })?;
        ctx.store.log_event(
            Some(&task.id),
            "cowork_landed",
            json!({"session": cowork.session, "run": run.id, "sha": sha, "commits": landing.commits}),
        )
    })();
    if let Err(error) = recorded {
        crate::messages::report(
            Some(&task.id),
            "cowork",
            format!(
                "landed; recording the landing on {} failed: {error:#}",
                cowork.session
            ),
        );
    }
}

/// A discarded co-work run took its worktree with it, so its session forgets the scratch
/// record, which would otherwise count against `assistant.scratch_max` for good.
pub(crate) fn release_discarded(ctx: &Ctx, task: &Task) {
    let (Some(cowork), Some(run)) = (task.cowork_run(), task.current_run()) else {
        return;
    };
    let released = ctx.store.update_assistant(&cowork.session, |session| {
        session.scratch.retain(|entry| entry.branch != run.branch);
        Ok(())
    });
    if let Err(error) = released {
        crate::messages::report(
            Some(&task.id),
            "cowork",
            format!(
                "discarded; releasing the scratch on {} failed: {error:#}",
                cowork.session
            ),
        );
    }
}
