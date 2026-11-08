//! Exact plan provenance and code seed adoption; no imported approval or PASS.
use anyhow::{Context, Result, ensure};
use corral_core::{PlanRun, PlanRunState, Run, RunOutcome, Status, Task, TaskRef};
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
    let task = ctx.store.get(id)?;
    if lifecycle::plan_only(prepared.package().goal) {
        adopt_plan(ctx, &task, prepared)?;
    } else if let Some(snapshot) = &prepared.package().snapshot {
        // An active check keeps its pinned checkout and exact attempt. Only its repair
        // destination changes; no duplicate candidate/run/check is created for transfer.
        let existing = task
            .current_run()
            .is_some_and(|r| Some(&r.worktree) == prepared.package().source_path.as_ref());
        if !existing {
            ensure!(
                task.runs.is_empty(),
                "handoff_adoption: an unrelated task history needs explicit seed reconciliation"
            );
            let (path, branch) = seed_path(ctx, &task)?;
            restore_worktree(ctx, &task, snapshot, &path, &branch)?;
            let candidate = prepared.publication.previous_phase
                == corral_core::assistant_workflow::WorkflowPhase::Candidate
                && corral_git::is_clean(&path)?;
            ctx.store.update(id, |task| {
                ensure!(task.runs.is_empty(), "handoff_adoption: task run changed");
                if candidate {
                    let now = corral_core::clock::now();
                    let mut run: Run = serde_json::from_value(json!({
                        "id":"R1", "agent":ctx.store.get_assistant(&prepared.package().assistant)?.profile,
                        "worktree":path,"branch":branch,"base":snapshot.state.base,"base_sha":snapshot.state.base,"started":now
                    }))?;
                    run.ended = Some(now);
                    run.outcome = Some(RunOutcome::Done);
                    run.agent_state = Some("done".into());
                    run.summary = Some("Exact transferred candidate; no gate or review result imported.".into());
                    run.autopilot = Some(corral_core::AutopilotState {
                        stage: corral_core::AutoStage::Gating, reason: Some("transferred candidate queued".into()),
                        sha: Some(snapshot.state.head.clone()), at: now,
                    });
                    task.runs.push(run);
                    task.cowork.as_mut().context("handoff fence missing")?.run = "R1".into();
                    task.status = Status::Review;
                }
                let mut journal = super::read_journal(&ctx.store, prepared.id())?.context("journal missing")?;
                journal.transfer.as_mut().context("transfer missing")?.seed_run = Some("R1".into());
                ctx.store.save_record(JOURNALS, prepared.id(), &journal)?;
                Ok(())
            })?;
        }
    }
    ctx.store.update(id, |task| {
        let mut journal =
            super::read_journal(&ctx.store, prepared.id())?.context("journal missing")?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        transfer.adoption_proven = true;
        transfer.contract_revision = task.contract_revision()?;
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

fn adopt_plan(ctx: &Ctx, task: &Task, prepared: &Prepared) -> Result<()> {
    let Some(plan) = &prepared.package().plan else {
        return Ok(());
    };
    let bytes = super::super::publication::load_artifact(&ctx.store, &plan.artifact)?;
    if corral_core::PlanDocument::parse(std::str::from_utf8(&bytes)?).is_err() {
        // An incomplete draft is prompt input, never rewritten into an invented formal plan.
        return Ok(());
    }
    if let Some(existing) = task.plans.last() {
        ensure!(
            existing.artifact.sha256 == plan.artifact.blob.hash().as_str(),
            "handoff_plan_changed: target contains another plan"
        );
        return Ok(());
    }
    let assistant = ctx.store.get_assistant(&prepared.package().assistant)?;
    // This submitted provenance record describes the ACTUAL producer. No planner is
    // launched, and review independence resolves against this profile, not a replacement.
    if task.plan_runs.is_empty() {
        ctx.store.update(&task.id, |task| {
            let now = corral_core::clock::now();
            task.plan_runs.push(PlanRun {
                agent_host: None,
                execution: None,
                route: None,
                delivery: Default::default(),
                inbox: Default::default(),
                id: "D1".into(),
                profile: assistant.profile.clone(),
                harness: "corral-pi".into(),
                model: None,
                vendor: None,
                effort: None,
                spec_revision: crate::plan_spec_revision(task)?,
                started: now,
                ended: Some(now),
                herdr: None,
                output_dir: ctx.store.dir().join("handoff-plan").join(&task.id),
                submitted: None,
                rounds: 0,
                state: PlanRunState::Planning,
                pending_reply: None,
            });
            Ok(())
        })?;
    }
    let temp = tempfile::tempdir().context("prepare exact plan submission")?;
    let path = temp.path().join("plan.md");
    std::fs::write(&path, &bytes).context("write exact plan snapshot")?;
    let submitted = crate::plan::plan_submit_checked(ctx, &task.id, &path, &|task| {
        ensure!(
            task.dispatch_hold,
            "handoff_reconciling: task must remain held during adoption"
        );
        Ok(())
    })?;
    ensure!(
        submitted.artifact.sha256 == plan.artifact.blob.hash().as_str(),
        "handoff_plan_changed: submitted bytes differ"
    );
    Ok(())
}

fn seed_path(ctx: &Ctx, task: &Task) -> Result<(PathBuf, String)> {
    let repo = ctx
        .store
        .project_path(task.project.as_deref().context("project missing")?)?;
    Ok((
        repo.join(".worktrees").join(&task.id),
        format!("corral/{}", task.id),
    ))
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
    if !path.exists() {
        std::fs::create_dir_all(path.parent().context("seed parent missing")?)?;
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
            std::fs::create_dir_all(destination.parent().context("seed file parent missing")?)?;
            publish_file(&destination, &data.load(&ctx.store)?)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    &destination,
                    std::fs::Permissions::from_mode(file.mode.context("file mode missing")?),
                )?;
            }
        }
    }
    let actual = git::scan(path, &snapshot.state.base, branch)?.state;
    let mut expected = snapshot.state.clone();
    expected.branch = branch.into();
    expected.branch_tip = expected.head.clone();
    expected.detached = false;
    ensure!(
        actual == expected,
        "handoff_restore_changed: exact index/work adoption differs; no empty-source fallback"
    );
    crate::agent::failpoint("handoff_adoption_verified");
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
    let snapshot = package
        .snapshot
        .context("handoff_missing: reserved code seed is missing")?;
    ensure!(
        run.worktree.is_dir(),
        "handoff_missing: adopted code seed disappeared; no empty-source fallback"
    );
    Ok(Some(snapshot.state.head))
}

pub(crate) fn prompt(ctx: &Ctx, task: &impl TaskRef) -> Result<String> {
    let Some(journal) = lifecycle::for_task(&ctx.store, task)? else {
        return Ok(String::new());
    };
    let (_, package, preview) = super::read_package(&ctx.store, &journal.id)?;
    let draft = package
        .plan
        .as_ref()
        .map(|p| super::super::publication::load_artifact(&ctx.store, &p.artifact))
        .transpose()?;
    Ok(format!(
        "\n## Preserved continuation\n\n{}\n\nPrior draft (exact bytes; not approval):\n{}\n\nSelected goal: {:?}. Plan-only work must not execute preserved code. If the integration base moved, reconcile the adopted seed rather than starting from an empty checkout.\n",
        preview.prompt,
        draft
            .as_deref()
            .map(std::str::from_utf8)
            .transpose()?
            .unwrap_or("No prior draft."),
        package.goal
    ))
}
