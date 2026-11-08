//! Exact plan provenance and code seed adoption; no imported approval or PASS.
use anyhow::{Context, Result, ensure};
use corral_core::{Run, RunOutcome, Status, Task, TaskRef};
use serde_json::json;
use std::path::{Path, PathBuf};

use super::{
    JOURNALS, Prepared, Snapshot,
    git::{self, git},
    lifecycle,
};
use crate::Ctx;

pub(crate) fn prepare_task(ctx: &Ctx, id: &str, prepared: &Prepared) -> Result<()> {
    let journal =
        super::read_journal(&ctx.store, prepared.id())?.context("handoff journal missing")?;
    if journal.transfer.as_ref().is_some_and(|t| t.adoption_proven) {
        return Ok(());
    }
    if super::continuation_seed::adopt(ctx, id, prepared, &journal)? {
        return Ok(());
    }
    let goal = journal.transfer.as_ref().context("transfer missing")?.goal;
    super::attachments::publish(ctx, id, prepared)?;
    if !journal
        .transfer
        .as_ref()
        .is_some_and(|t| t.ownership_committed)
    {
        super::plan_adoption::adopt(ctx, &ctx.store.get(id)?, prepared)?;
    }
    let task = ctx.store.get(id)?;
    let current = super::read_journal(&ctx.store, prepared.id())?.context("transfer missing")?;
    let draft_pending = super::plan_adoption::draft_pending(
        &task,
        current.transfer.as_ref().context("transfer missing")?,
    )?;
    if !lifecycle::plan_only(goal)
        && let Some(snapshot) = &prepared.package().snapshot
    {
        // An active check keeps its pinned checkout and exact attempt. Only its repair
        // destination changes; no duplicate candidate/run/check is created for transfer.
        let existing = task.current_run().is_some_and(|r| {
            let submitted = r
                .autopilot
                .as_ref()
                .and_then(|s| s.sha.as_deref())
                .or_else(|| r.gate.as_ref().map(|g| g.sha.as_str()))
                .or_else(|| r.review.as_ref().map(|review| review.sha.as_str()));
            Some(&r.worktree) == prepared.package().source_path.as_ref()
                && prepared.code_clean() == Some(true)
                && submitted == Some(snapshot.state.head.as_str())
        });
        if !existing {
            ensure!(
                task.current_run().is_none_or(|r| r.outcome.is_some()),
                "handoff_adoption: previous writer must settle"
            );
            let run_id = seed_run(ctx, &task)?;
            let previous_runs = task.runs.len();
            let (path, branch) = seed_path(ctx, &task)?;
            restore_worktree(ctx, &task, snapshot, &path, &branch)?;
            let candidate = prepared.publication.previous_phase
                == corral_core::assistant_workflow::WorkflowPhase::Candidate
                && git(
                    &path,
                    &["status", "--porcelain", "--untracked-files=all"],
                    None,
                )?
                .is_empty();
            let ready = !draft_pending && crate::approved_plan(ctx, &task).is_ok();
            ctx.store.update(id, |task| {
                ensure!(
                    task.runs.len() == previous_runs,
                    "handoff_adoption: task run changed"
                );
                if candidate && ready {
                    candidate_run(ctx, task, prepared, snapshot, &path, &branch, &run_id, goal)?;
                }
                let mut journal =
                    super::read_journal(&ctx.store, prepared.id())?.context("journal missing")?;
                let transfer = journal.transfer.as_mut().context("transfer missing")?;
                transfer.seed_run = Some(run_id.clone());
                transfer.candidate_pending = candidate && !ready;
                if !candidate || !ready {
                    task.status = if task.plans.last().is_some_and(|p| p.approval.is_none())
                        || transfer.candidate_pending
                    {
                        Status::Review
                    } else {
                        Status::Ready
                    };
                }
                ctx.store.save_record(JOURNALS, prepared.id(), &journal)?;
                Ok(())
            })?;
        }
    }
    if !lifecycle::plan_only(goal) && prepared.package().snapshot.is_none() {
        ctx.store.update(id, |task| {
            ensure!(
                task.current_run().is_none_or(|r| r.outcome.is_some()),
                "handoff_adoption: prior writer must settle"
            );
            let mut journal =
                super::read_journal(&ctx.store, prepared.id())?.context("journal missing")?;
            let transfer = journal.transfer.as_mut().context("transfer missing")?;
            transfer.seed_run = Some(format!("R{}", task.runs.len() + 1));
            transfer.fresh_seed = true;
            task.status = if task.plans.last().is_some_and(|p| p.approval.is_none()) {
                Status::Review
            } else {
                Status::Ready
            };
            ctx.store.save_record(JOURNALS, prepared.id(), &journal)?;
            Ok(())
        })?;
    }
    ctx.store.update(id, |task| {
        let mut journal =
            super::read_journal(&ctx.store, prepared.id())?.context("journal missing")?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        ensure!(
            transfer.contract_revision == task.contract_revision()?,
            "handoff_scope_changed: held task changed during adoption; refresh/reconcile"
        );
        transfer.adoption_proven = true;
        if super::plan_adoption::draft_pending(task, transfer)?
            || (transfer.goal == super::Goal::PlanContinue && task.plans.is_empty())
        {
            task.status = Status::Ready;
        }
        if transfer.goal == super::Goal::PlanReviewOnly && transfer.review_target.is_none() {
            transfer.review_target = Some(lifecycle::review_target(task)?);
        }
        task.cowork
            .as_mut()
            .context("handoff fence missing")?
            .completion
            .as_mut()
            .context("completion missing")?
            .contract_revision = transfer.contract_revision.clone();
        let goal = transfer.goal;
        ctx.store.save_record(JOURNALS, prepared.id(), &journal)?;
        ctx.store.log_event(
            Some(&task.id),
            "handoff_adopted",
            json!({"transfer":prepared.id(),"goal":goal}),
        )?;
        Ok(())
    })?;
    Ok(())
}

fn candidate_run(
    ctx: &Ctx,
    task: &mut Task,
    prepared: &Prepared,
    snapshot: &Snapshot,
    path: &Path,
    branch: &str,
    run_id: &str,
    goal: super::Goal,
) -> Result<()> {
    ensure!(
        run_id == format!("R{}", task.runs.len() + 1) && !task.runs.iter().any(|r| r.id == run_id),
        "handoff_adoption: candidate run identity already exists or changed"
    );
    let now = corral_core::clock::now();
    let repo = ctx.store.project_path(&prepared.package().project)?;
    let base = ctx
        .store
        .config
        .projects
        .get(&prepared.package().project)
        .and_then(|p| p.base.clone())
        .map(Ok)
        .unwrap_or_else(|| corral_git::current_branch(&repo))?;
    let mut run: Run = serde_json::from_value(json!({
        "id":run_id, "agent":ctx.store.get_assistant(&prepared.package().assistant)?.profile,
        "worktree":path,"branch":branch,"base":base,"base_sha":snapshot.state.base,"started":now
    }))?;
    run.ended = Some(now);
    run.outcome = Some(RunOutcome::Done);
    run.agent_state = Some("done".into());
    run.summary = Some("Exact transferred candidate; no gate or review result imported.".into());
    let prepare_only = goal == super::Goal::ImplementationPrepare;
    run.autopilot = Some(corral_core::AutopilotState {
        stage: if prepare_only {
            corral_core::AutoStage::Reviewed
        } else {
            corral_core::AutoStage::Gating
        },
        reason: Some(
            if prepare_only {
                "Prepared candidate; checks not requested"
            } else {
                "transferred candidate queued"
            }
            .into(),
        ),
        sha: Some(snapshot.state.head.clone()),
        at: now,
    });
    let policy = ctx.store.config.task_review_policy(task);
    if !policy.gate_tiers.is_empty() {
        run.gate_rules = corral_core::GateRules::Tiered;
    }
    run.completion.manifest = task.completion.clone();
    run.completion.required_checks = Some(crate::gate_tier::required_checks(
        ctx,
        task,
        &corral_core::RunHead::of(&run)?,
    )?);
    task.runs.push(run);
    task.cowork.as_mut().context("handoff fence missing")?.run = run_id.into();
    task.status = Status::Review;
    Ok(())
}

pub(super) fn advance_candidate(ctx: &Ctx, id: &str) -> Result<()> {
    let _pipeline = ctx
        .store
        .try_lock(&format!("autopilot-{id}"))?
        .context("Waiting for current controller")?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .context("Waiting for current launch")?;
    let _gate = ctx
        .store
        .try_lock(&format!("gate-{id}"))?
        .context("Waiting for gate admission")?;
    let task = ctx.store.get(id)?;
    let journal = lifecycle::for_task(&ctx.store, &task)?.context("transfer missing")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    if transfer.disposition != lifecycle::Disposition::Continuing
        || !transfer.candidate_pending
        || super::plan_adoption::draft_pending(&task, transfer)?
        || task.dispatch_hold
        || task.hold.is_some()
        || task.park.is_some()
    {
        return Ok(());
    }
    let config = ctx.store.config_layers()?.config()?;
    if config.automation.paused
        || config.dispatch.paused
        || !config.autopilot_enabled()
        || crate::approved_plan(ctx, &task).is_err()
        || task
            .plans
            .last()
            .is_some_and(|p| !p.document.dependencies.is_empty())
    {
        return Ok(());
    }
    let prepared = super::checkpoint(&ctx.store, &journal.id, false)?;
    let snapshot = prepared
        .package()
        .snapshot
        .as_ref()
        .context("handoff_missing: pending candidate seed missing")?;
    let (path, branch) = seed_path(ctx, &task)?;
    verify_worktree(snapshot, &path, &branch)?;
    let run_id = transfer
        .seed_run
        .clone()
        .context("handoff_missing: candidate run identity missing")?;
    ctx.store.try_update(id, |task| {
        let mut journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        if transfer.disposition != lifecycle::Disposition::Continuing
            || !transfer.candidate_pending
            || super::plan_adoption::draft_pending(task, transfer)?
        {
            return Ok(false);
        }
        let config = ctx.store.config_layers()?.config()?;
        if config.automation.paused
            || config.dispatch.paused
            || !config.autopilot_enabled()
            || task.dispatch_hold
            || task.hold.is_some()
            || task.park.is_some()
        {
            return Ok(false);
        }
        anyhow::ensure!(
            transfer.contract_revision == task.contract_revision()?
                && !task.runs.iter().any(|r| r.id == run_id),
            "handoff_stale_choice: task/spec/run changed before candidate adoption"
        );
        crate::approved_plan(ctx, task)?;
        candidate_run(
            ctx,
            task,
            &prepared,
            snapshot,
            &path,
            &branch,
            &run_id,
            transfer.goal,
        )?;
        transfer.candidate_pending = false;
        journal.waiting = None;
        super::recovery::save(&ctx.store, &journal)?;
        ctx.store.log_event(
            Some(&task.id),
            "handoff_candidate_adopted",
            json!({"transfer":journal.id,"run":run_id,"head":snapshot.state.head}),
        )?;
        Ok(true)
    })?;
    Ok(())
}

pub(crate) fn seed_path(ctx: &Ctx, task: &Task) -> Result<(PathBuf, String)> {
    let repo = ctx
        .store
        .project_path(task.project.as_deref().context("project missing")?)?;
    let run = seed_run(ctx, task)?;
    let journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
    // A returned scope can produce a newer seed before any coding run exists. Never
    // reuse/overwrite that earlier scope's worktree or private Git checkpoint refs.
    let seed_hash = journal
        .transfer
        .as_ref()
        .and_then(|t| t.continuation_seed.as_ref())
        .map_or(journal.package.blob.hash().as_str(), |s| {
            s.selected.state_sha256.as_str()
        });
    let short = seed_hash
        .get(..8)
        .context("handoff_seed_changed: invalid seed identity")?;
    let suffix = format!(
        "{}-handoff-{short}-r{}",
        task.id,
        run.trim_start_matches('R')
    );
    Ok((
        repo.join(".worktrees").join(&suffix),
        format!("corral/{suffix}"),
    ))
}

pub(super) fn seed_run(ctx: &Ctx, task: &Task) -> Result<String> {
    Ok(lifecycle::for_task(&ctx.store, task)?
        .and_then(|j| j.transfer)
        .and_then(|t| t.seed_run)
        .unwrap_or_else(|| format!("R{}", task.runs.len() + 1)))
}

pub(super) fn restore_worktree(
    ctx: &Ctx,
    task: &Task,
    snapshot: &Snapshot,
    path: &Path,
    branch: &str,
) -> Result<()> {
    let repo = ctx
        .store
        .project_path(task.project.as_deref().context("project missing")?)?;
    crate::assistant::scratch::disk_check(ctx)?;
    let temp = tempfile::tempdir().context("verify adoption")?;
    snapshot.restore_new(&ctx.store, &temp.path().join("verified"))?;
    let bundle = temp.path().join("seed.bundle");
    std::fs::write(&bundle, snapshot.bundle.load(&ctx.store)?).context("write adoption bundle")?;
    let prefix = format!(
        "refs/corral/handoff/{}",
        crate::verification::digest(branch.as_bytes())
    );
    let refs: Vec<_> = ["base", "head", "branch", "index"]
        .map(|name| format!("refs/heads/{name}:{prefix}/{name}"))
        .into();
    let mut args = vec![
        "fetch",
        "--no-tags",
        "--no-write-fetch-head",
        bundle.to_str().context("non-UTF-8 bundle path")?,
    ];
    args.extend(refs.iter().map(String::as_str));
    git(&repo, &args, None)?;
    let parent = path.parent().context("seed parent missing")?;
    std::fs::create_dir_all(parent)?;
    let parent_guard = corral_core::fs_guard::DirHandle::open(parent)?;
    if !path.exists() {
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--no-checkout",
                "-b",
                branch,
                path.to_str().context("non-UTF-8 seed path")?,
                &snapshot.state.head,
            ],
            None,
        )?;
    } else {
        ensure!(
            String::from_utf8(git(path, &["branch", "--show-current"], None)?)?.trim() == branch
                && corral_git::rev_sha(path, "HEAD")? == snapshot.state.head,
            "handoff_restore_changed: existing destination is not the reserved seed"
        );
    }
    ensure!(
        path.symlink_metadata()?.file_type().is_dir(),
        "handoff_restore_changed: destination is not a real directory"
    );
    let destination_guard = corral_core::fs_guard::DirHandle::open(path)?;
    let index_path = String::from_utf8(git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        None,
    )?)?;
    publish_file(
        Path::new(index_path.trim()),
        &snapshot.state.index.load(&ctx.store)?,
    )?;
    for file in &snapshot.state.files {
        if let Some(data) = &file.data {
            let destination = path.join(&file.path);
            create_parents(path, Path::new(&file.path))?;
            publish_file(&destination, &data.load(&ctx.store)?)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                corral_core::fs_guard::open_regular_nofollow(&destination)?.set_permissions(
                    std::fs::Permissions::from_mode(file.mode.context("file mode missing")?),
                )?;
            }
        }
    }
    parent_guard.revalidate()?;
    destination_guard.revalidate()?;
    verify_worktree(snapshot, path, branch)?;
    crate::agent::failpoint("handoff_adoption_verified");
    Ok(())
}

fn create_parents(root: &Path, file: &Path) -> Result<()> {
    let mut parent = root.to_path_buf();
    for part in file
        .parent()
        .context("seed file parent missing")?
        .components()
    {
        parent.push(part);
        match parent.symlink_metadata() {
            Ok(meta) => ensure!(
                meta.file_type().is_dir(),
                "handoff_restore_changed: unsafe parent {}",
                parent.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&parent)?,
            Err(e) => return Err(e).context("inspect adoption parent"),
        }
    }
    Ok(())
}

pub(super) fn verify_worktree(snapshot: &Snapshot, path: &Path, branch: &str) -> Result<()> {
    let actual = git::scan(path, &snapshot.state.base, branch)?.state;
    let mut expected = snapshot.state.clone();
    expected.branch = branch.into();
    expected.branch_tip = expected.head.clone();
    expected.detached = false;
    ensure!(
        actual == expected,
        "handoff_restore_changed: exact index/work adoption differs; no empty-source fallback"
    );
    Ok(())
}

fn publish_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.symlink_metadata().is_ok() {
        ensure!(
            super::content::regular(path, super::content::FILE_MAX)?.0 == bytes,
            "handoff_restore_changed: {} differs from reserved content",
            path.display()
        );
    } else {
        use std::io::Write;
        let mut temp =
            tempfile::NamedTempFile::new_in(path.parent().context("file parent missing")?)?;
        temp.write_all(bytes).context("write restored content")?;
        temp.persist_noclobber(path)
            .context("publish restored content without overwrite")?;
    }
    Ok(())
}

pub(super) fn pending_seed(ctx: &Ctx, task: &impl TaskRef) -> Result<bool> {
    has_seed_for(ctx, task, &format!("R{}", task.runs_len()? + 1))
}

pub(crate) fn has_seed_for(ctx: &Ctx, task: &impl TaskRef, run: &str) -> Result<bool> {
    Ok(lifecycle::for_task(&ctx.store, task)?
        .and_then(|j| j.transfer)
        .is_some_and(|t| t.adoption_proven && t.seed_run.as_deref() == Some(run)))
}

pub(crate) fn start_head(
    ctx: &Ctx,
    task: &impl TaskRef,
    run: &corral_core::RunHead,
) -> Result<Option<String>> {
    let Some(journal) = lifecycle::for_task(&ctx.store, task)? else {
        return Ok(None);
    };
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    if transfer.seed_run.as_deref() != Some(&run.id) {
        return Ok(None);
    }
    ensure!(
        transfer.adoption_proven,
        "handoff_reconciling: seed adoption incomplete"
    );
    let (_, package, _) = super::read_package(&ctx.store, &journal.id)?;
    if transfer.fresh_seed {
        ensure!(
            package.snapshot.is_none(),
            "handoff_reconciling: fresh seed cannot replace captured code"
        );
        if run.worktree.exists() {
            ensure!(
                git(
                    &run.worktree,
                    &["status", "--porcelain", "--untracked-files=all"],
                    None
                )?
                .is_empty(),
                "handoff_restore_changed: fresh launch checkout changed before first writer admission"
            );
        }
        return Ok(Some(run.base_sha.clone()));
    }
    let snapshot = super::continuation_seed::snapshot(&journal, &package)
        .context("handoff_missing: reserved code seed is missing")?;
    ensure!(
        run.worktree.is_dir(),
        "handoff_missing: adopted code seed disappeared; no empty-source fallback"
    );
    verify_worktree(&snapshot, &run.worktree, &run.branch)?;
    Ok(Some(snapshot.state.head.clone()))
}

pub(crate) fn prompt(ctx: &Ctx, task: &impl TaskRef) -> Result<String> {
    let Some(journal) = lifecycle::for_task(&ctx.store, task)? else {
        return Ok(String::new());
    };
    let (_, package, preview) = super::read_package(&ctx.store, &journal.id)?;
    let draft = match super::planner_draft::latest(&ctx.store, task, &journal)? {
        Some(bytes) => Some(bytes),
        None => package
            .plan
            .as_ref()
            .map(|p| super::super::publication::load_artifact(&ctx.store, &p.artifact))
            .transpose()?,
    };
    Ok(format!(
        "\n## Preserved continuation\n\n{}\n\nPrior draft (exact bytes; not approval):\n{}\n\nSelected goal: {:?}. Plan-only work must not execute preserved code. If the integration base moved, reconcile the adopted seed rather than starting from an empty checkout. Selected source context and its exact archive index are task artifacts under R0. Archived bytes are not claimed read; checkpoint metadata is not a passing check or approval.\n",
        preview.prompt,
        draft
            .as_deref()
            .map(std::str::from_utf8)
            .transpose()?
            .unwrap_or("No prior draft."),
        journal.transfer.as_ref().context("transfer missing")?.goal
    ))
}
