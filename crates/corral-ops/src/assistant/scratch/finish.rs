//! Co-work finish (T-544 cowork-landing). On the owner's instruction a scratch becomes a task:
//! corral materializes task `T-<m>` (origin assistant, label `cowork`) or, with `into`, a new run
//! of an existing task, whose worktree and branch are the scratch's own. Confirmed owner
//! finishes require gate and independent review, then autopilot can land without another
//! owner acceptance. A landed co-work run is recorded on its session.
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::assistant::{AssistantLanding, AssistantScratch, validate_id};
use corral_core::{
    ArtifactKind, CompletionDriver, CompletionIntent, CompletionScope, Cowork, CoworkReview,
    GateRules, Origin, Run, RunOutcome, Status, Task, TaskKind, TaskRef as _, TaskSize, Usage,
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
/// existing task that gets a new run. Confirmed finishes always require independent review.
#[derive(Debug, Clone, Default)]
pub struct Finish<'a> {
    pub title: Option<&'a str>,
    pub project: Option<&'a str>,
    pub kind: Option<TaskKind>,
    pub size: Option<TaskSize>,
    pub review: Option<CoworkReview>,
    /// Verify runs the same checks/repairs but never lands. Omitted repairs retain their intent.
    pub intent: Option<CompletionIntent>,
    pub into: Option<&'a str>,
    pub note: Option<&'a str>,
    /// The candidate commit the owner was shown: the finish refuses `stale_candidate` unless the
    /// scratch HEAD is still that commit (a hex prefix of at least 7 digits).
    pub expected_head: Option<&'a str>,
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
    /// Durable admission state (`queued`); reconciliation owns the eventual gate receipt.
    pub gate: String,
    /// Reviewer readback, when already known; submission itself never starts/waits for review.
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
    if task
        .current_run()
        .and_then(|run| run.gate.as_ref())
        .is_some_and(|gate| gate.result == corral_core::GateResult::Running)
    {
        return Some("its gate attempt has not settled (collect/cancel/recover it first)".into());
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
    if let Some(run) = task.current_run()
        && task.open_obligations(&run.id)?.pending_decision().is_some()
    {
        bail!(
            "unresolved owner decision; preserve and resolve the current run's question before resubmitting"
        );
    }
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

fn check_completion(task: &Task, session: &str, slug: &str) -> Result<()> {
    if let Some(cowork) = task.cowork_run()
        && let Some(scope) = cowork.completion.as_ref()
    {
        if matches!(owner_action::authority(), OwnerAuthority::ViaAssistant(via) if !via.confirmed)
        {
            bail!(
                "completion_client_unavailable: upgrade the assistant to submit a confirmed current-scope instruction"
            );
        }
        if scope.version != 1 || scope.contract_revision != task.contract_revision()? {
            bail!(
                "completion_scope_changed: refresh the current-revision instruction or upgrade Corral; nothing submitted"
            );
        }
        if cowork.assistant_driver() != Some((session, slug)) {
            bail!(
                "completion_driver_changed: this scope belongs to another driver; use an explicit take-back before submitting"
            );
        }
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
            crate::agent::usage::collect_state_complete(&container.state, &ctx.store)
        });
    match measured {
        Ok(usage) => window_usage(session, usage, from, to),
        Err(error) => (
            None,
            Some(format!("{session} usage unavailable: {error:#}")),
        ),
    }
}

fn window_usage(
    session: &str,
    usage: Usage,
    from: Timestamp,
    to: Timestamp,
) -> (Option<Usage>, Option<String>) {
    let entries: Vec<_> = usage
        .entries
        .into_iter()
        .filter(|entry| entry.at.is_some_and(|at| from <= at && at <= to))
        .collect();
    let usage = Usage::from_entries(entries);
    if usage.total_tokens() == 0 {
        (
            None,
            Some(format!(
                "{session} recorded no nonzero usage between scratch creation and finish"
            )),
        )
    } else {
        (Some(usage), None)
    }
}

#[cfg(test)]
#[path = "finish_tests.rs"]
mod tests;

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
    head: &'a str,
    summary: &'a str,
    review: CoworkReview,
    intent: CompletionIntent,
    owner_confirmed: bool,
    now: Timestamp,
    usage: (Option<Usage>, Option<String>),
}

impl Draft<'_> {
    fn apply(&self, ctx: &Ctx, task: &mut Task) -> Result<String> {
        crate::repair_cycle::submitted(&ctx.store, task)?;
        if self.owner_confirmed {
            task.kind
                .get_or_insert_with(|| TaskKind::Backend.to_string());
            task.size.get_or_insert(TaskSize::M);
        }
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
        run.completion.required_checks = Some(crate::gate_tier::required_checks(
            ctx,
            task,
            &corral_core::RunHead::of(&run)?,
        )?);
        run.summary = Some(self.summary.to_owned());
        run.ended = Some(self.now);
        run.outcome = Some(RunOutcome::Done);
        run.agent_state = Some("done".into());
        (run.usage, run.usage_note) = self.usage.clone();
        if self.owner_confirmed {
            run.autopilot = Some(corral_core::AutopilotState {
                stage: corral_core::AutoStage::Gating,
                reason: Some("completion queued".into()),
                sha: Some(self.head.to_owned()),
                at: self.now,
            });
        }
        task.runs.push(run);
        let completion = if self.owner_confirmed {
            Some(CompletionScope {
                version: 1,
                contract_revision: task.contract_revision()?,
                intent: self.intent,
                driver: CompletionDriver::Assistant {
                    session: self.session.to_owned(),
                    scratch: self.scratch.slug.clone(),
                },
                instruction: self.summary.to_owned(),
            })
        } else {
            None
        };
        let transfer_fenced = task
            .cowork
            .as_ref()
            .is_some_and(|c| c.review == CoworkReview::TransferV1);
        task.cowork = Some(Cowork {
            completion,
            session: self.session.to_owned(),
            scratch: self.scratch.slug.clone(),
            run: id.clone(),
            review: if transfer_fenced {
                CoworkReview::TransferV1
            } else {
                self.review
            },
            owner_confirmed: self.owner_confirmed,
        });
        if transfer_fenced {
            crate::assistant::workflow::handoff::renew::submitted(
                ctx,
                task,
                self.session,
                &self.scratch.slug,
            )?;
        }
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
    if opts.intent.is_some()
        && matches!(owner_action::authority(), OwnerAuthority::ViaAssistant(via) if !via.confirmed)
    {
        bail!(
            "completion_client_unavailable: explicit Verify/Ship intent requires a confirmed owner instruction; upgrade the assistant"
        );
    }
    // Same per-task lease/order as controller advancement and gate admission. No board
    // writer lock is held while Git runs, and an active check is refused below, not replayed.
    let target_id = opts
        .into
        .map(corral_core::Store::normalize_id)
        .transpose()?;
    let _pipeline = target_id
        .as_ref()
        .map(|id| {
            ctx.store
                .try_lock(&format!("autopilot-{id}"))?
                .context("completion pipeline is advancing; retry submission")
        })
        .transpose()?;
    let _gate = target_id
        .as_ref()
        .map(|id| {
            ctx.store
                .try_lock(&format!("gate-{id}"))?
                .context("gate admission is in progress; retry submission")
        })
        .transpose()?;
    let _guard = lock(ctx, &format!("scratch-{session}-{slug}"))?;
    let scratch = find(ctx, session, slug)?;
    if let Some(holder) = holder(&ctx.store.list()?, &scratch) {
        let target = opts.into.map(|id| ctx.store.get(id)).transpose()?;
        let repairing = target.as_ref().is_some_and(|task| {
            matches!(task.status, Status::Review | Status::NeedsMe)
                && task
                    .cowork_run()
                    .is_some_and(|cowork| cowork.assistant_driver() == Some((session, slug)))
                && task.current_run().is_some_and(|run| {
                    run.worktree == scratch.path
                        && run.autopilot.as_ref().is_some_and(|state| {
                            state.stage == corral_core::AutoStage::Changes
                                || (state.stage == corral_core::AutoStage::Reviewed
                                    && opts.intent.is_some())
                                || (state.stage == corral_core::AutoStage::Escalated
                                    && opts.expected_head.is_some()
                                    && state.reason.as_deref().is_some_and(|reason| {
                                        reason.starts_with("submitted candidate changed")
                                    }))
                        })
                })
        });
        if !repairing {
            bail!(
                "scratch_attached: {session}/{slug} already belongs to {holder}; nothing finished"
            );
        }
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
    let candidate = corral_git::rev_sha(&scratch.path, "HEAD")?;
    if let Some(expected) = opts.expected_head {
        let hex =
            (7..=64).contains(&expected.len()) && expected.bytes().all(|b| b.is_ascii_hexdigit());
        if !hex {
            bail!("--expected-head takes a commit SHA of 7 to 64 hex digits, not {expected:?}");
        }
        if !candidate.starts_with(&expected.to_ascii_lowercase()) {
            bail!(
                "stale_candidate: {session}/{slug} is at {}, not the candidate {expected} the owner was shown; show the owner the current candidate and ask again; nothing finished",
                candidate.get(..12).unwrap_or(&candidate)
            );
        }
    }
    let into = match opts.into {
        Some(id) => {
            let task = ctx.store.get(id)?;
            check_into(&task, project)?;
            check_completion(&task, session, slug)?;
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
    let owner_confirmed = match owner_action::authority() {
        OwnerAuthority::Shell => true,
        OwnerAuthority::ViaAssistant(via) => via.confirmed,
    };
    let review = if owner_confirmed {
        CoworkReview::CompletionV1
    } else {
        opts.review.unwrap_or(CoworkReview::None)
    };
    let mut summary = format!(
        "Co-work finish of assistant scratch {session}/{slug} (branch `{}`, head `{head}`) {words}\n\nReview: {}.",
        scratch.branch,
        match review {
            CoworkReview::None => "none",
            CoworkReview::Required | CoworkReview::CompletionV1 | CoworkReview::TransferV1 =>
                "required",
        }
    );
    if let Some(note) = opts.note.filter(|note| !note.trim().is_empty()) {
        summary.push_str(&format!("\n\n{}", note.trim()));
    }
    let now = corral_core::clock::now();
    let previous = into.as_ref().map(|id| ctx.store.get(id)).transpose()?;
    let intent = opts
        .intent
        .or_else(|| {
            previous
                .as_ref()?
                .cowork
                .as_ref()?
                .completion
                .as_ref()
                .map(|scope| scope.intent)
        })
        .unwrap_or(CompletionIntent::Ship);
    let draft = Draft {
        session,
        profile: &record.profile,
        scratch: &scratch,
        base: &base,
        base_sha: &base_sha,
        head: &head,
        summary: &summary,
        review,
        intent,
        owner_confirmed,
        now,
        usage: usage_between(ctx, session, scratch.created, now),
    };
    let log_finished = |task: &Task, run: &str| {
        ctx.store.log_event(
            Some(&task.id),
            "cowork_finished",
            json!({
                "session": session, "slug": slug, "run": run, "branch": scratch.branch,
                "head": head, "candidate": candidate, "base": base, "review": review,
                "owner_confirmed": owner_confirmed, "into": into.is_some(),
            }),
        )
    };
    let mut run = String::new();
    let task = match &into {
        Some(id) => ctx.store.update(id, |task| {
            check_into(task, project)?;
            check_completion(task, session, slug)?;
            run = draft.apply(ctx, task)?;
            log_finished(task, &run)?;
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
                    log_finished(task, &run)?;
                    Ok(())
                },
            )?
        }
    };
    ctx.store.update_assistant(session, |s| {
        if let Some(entry) = s.scratch.iter_mut().find(|entry| entry.slug == slug) {
            entry.kind = COWORK.into();
        }
        if s.linked_scratch.as_deref() == Some(slug) {
            s.linked_scratch = None;
        }
        Ok(())
    })?;
    snapshot_diff(ctx, &task.id, &scratch.path, &base_sha)?;
    // Task/Run publication is the durable queue. Sync owns gate/review/landing, not the
    // assistant host or a detached request. No gate-length work occurs in this call.
    let gate = "queued".to_owned();
    let reviewer = None;
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

fn scratch_cleaned(repo: &Path, run: &corral_core::RunHead) -> Result<bool> {
    Ok(!run.worktree.exists() && !git::branch_exists(repo, &run.branch)?)
}

/// After `approve` landed a co-work run: its session lists the landed task, run and commits,
/// and forgets a scratch only after both its directory and branch are gone. The landing is durable
/// already, so a failure here is reported, never returned.
pub(crate) fn record_landing<T: corral_core::TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &corral_core::RunHead,
    previous: Option<&str>,
    sha: &str,
) {
    let Some(cowork) = task
        .cowork()
        .as_ref()
        .filter(|cowork| cowork.applies_to(&run.id))
    else {
        return;
    };
    let recorded = (|| -> Result<()> {
        let repo = crate::review::main_repo(ctx, task, run)?;
        let commits = match previous {
            Some(previous) => git::range(&repo, previous, sha)?,
            None => Vec::new(),
        };
        let landing = AssistantLanding {
            task: task.id().clone(),
            run: run.id.clone(),
            base: run.base.clone(),
            sha: sha.to_owned(),
            commits,
            at: Timestamp::now(),
        };
        let cleaned = scratch_cleaned(&repo, run)?;
        ctx.store.update_assistant(&cowork.session, |session| {
            // Retained/failed-cleanup branches remain discoverable for preserving GC/discard.
            session
                .scratch
                .retain(|entry| entry.branch != run.branch || !cleaned);
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
            Some(task.id()),
            "cowork_landed",
            json!({"session": cowork.session, "run": run.id, "sha": sha, "commits": landing.commits}),
        )
    })();
    if let Err(error) = recorded {
        crate::messages::report(
            Some(task.id()),
            "cowork",
            format!(
                "landed; recording the landing on {} failed: {error:#}",
                cowork.session
            ),
        );
    }
}

/// Forget a discarded co-work scratch only after its directory and branch cleanup succeeded.
/// A retained branch or a failed cleanup stays discoverable for preserving GC/discard.
pub(crate) fn release_discarded(ctx: &Ctx, task: &Task) {
    let (Some(cowork), Some(run)) = (task.cowork_run(), task.current_run()) else {
        return;
    };
    let released = (|| -> Result<()> {
        let run = corral_core::RunHead::of(run)?;
        let repo = crate::review::main_repo(ctx, task, &run)?;
        let cleaned = scratch_cleaned(&repo, &run)?;
        ctx.store.update_assistant(&cowork.session, |session| {
            session
                .scratch
                .retain(|entry| entry.branch != run.branch || !cleaned);
            Ok(())
        })?;
        Ok(())
    })();
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
