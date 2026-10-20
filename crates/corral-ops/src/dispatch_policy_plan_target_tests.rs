use super::*;
use corral_core::Origin;

const TARGET: &str = "feature/new-plan";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn consumer(fx: &Fixture, accepted_sha: &str) -> (Task, Task) {
    let dependency = fx.task("External accepted work", Status::Done);
    fx.ctx
        .store
        .update(&dependency.id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"fake", "worktree":"/unused",
                "branch":"external", "base":"main", "base_sha":accepted_sha,
                "started":"2026-10-16T00:00:00Z", "outcome":"done",
            }))?);
            Ok(())
        })
        .unwrap();
    fx.ctx
        .store
        .log_event(
            Some(&dependency.id),
            "approved",
            json!({
                "run":"R1", "sha":accepted_sha, "strategy":"fast_forward", "base":"main",
            }),
        )
        .unwrap();
    let source = fx.ctx.store.root().join("plan.md");
    std::fs::write(&source, format!(
        "---\ntitle: Bootstrap Plan\nproject: app\napproved_scope:\n  goal: Build on accepted external work\n  acceptance: [{{scenario: dispatch, expected: external work included}}]\n  delivery_target: {{kind: integration, branch: {TARGET}}}\ntasks:\n  - {{key: consumer, title: Consumer, profile: fake, depends: [{}]}}\n---\n",
        dependency.id,
    )).unwrap();
    let imported = crate::import_plan(&fx.ctx, &source, false, Origin::Owner).unwrap();
    (imported.tracker, imported.tasks[0].clone())
}

#[test]
fn automatic_backlog_bootstraps_an_unclaimed_plan_with_included_external_work() {
    // Given an authorized Backlog member with a receipt already included in main.
    let mut fx = Fixture::new();
    let repo = fx.ctx.store.project_path("app").unwrap();
    let main = corral_git::rev_sha(&repo, "refs/heads/main").unwrap();
    let (plan, consumer) = consumer(&fx, &main);
    fx.grant(&[&plan]);
    crate::operating::require(&fx.ctx.store, &consumer, OperatingActionKind::Dispatch).unwrap();
    assert!(free_slots(&fx.ctx).unwrap() > 0);
    assert!(fx.ctx.herdr.workspace_list().is_ok());
    assert_eq!(consumer.status, Status::Backlog);
    assert!(corral_git::rev_sha(&repo, &format!("refs/heads/{TARGET}")).is_err());
    let progress = crate::plan_progress(&fx.ctx, &plan.id).unwrap();
    let prerequisite = &progress.work[0].prerequisites[0];
    assert!(prerequisite.accepted && prerequisite.satisfied);
    assert_eq!(prerequisite.proposed_initial_base.as_ref(), Some(&main));
    assert!(prerequisite.target_head.is_none());
    assert!(corral_git::rev_sha(&repo, &format!("refs/heads/{TARGET}")).is_err());
    // When ordinary sync runs automatic Backlog admission.
    crate::sync_at(&fx.ctx, Fixture::now()).unwrap();
    // Then checked initialization is reachable and the actual base includes the receipt.
    let started = fx.ctx.store.get(&consumer.id).unwrap();
    assert_eq!(started.status, Status::Running);
    let run = started.current_run().unwrap();
    assert_eq!(run.base, TARGET);
    assert_eq!(run.base_sha, main);
    assert_eq!(corral_git::rev_sha(&run.worktree, "HEAD").unwrap(), main);
    assert_eq!(corral_git::rev_sha(&repo, TARGET).unwrap(), main);
    assert_eq!(run.plan_target.as_ref().unwrap().plan, plan.id);
    assert_eq!(run.execution.as_ref().unwrap().resolved.profile, "fake");
    assert_eq!(corral_git::current_branch(&repo).unwrap(), "main");
    assert_eq!(corral_git::rev_sha(&repo, "refs/heads/main").unwrap(), main);
    let progress = crate::plan_progress(&fx.ctx, &plan.id).unwrap();
    let prerequisite = &progress.work[0].prerequisites[0];
    assert_eq!(prerequisite.target_head.as_ref(), Some(&main));
    assert!(prerequisite.proposed_initial_base.is_none());
}

#[test]
fn automatic_backlog_does_not_bootstrap_when_external_work_is_absent_from_main() {
    // Given a valid acceptance receipt whose commit is only on an external branch.
    let mut fx = Fixture::new();
    let repo = fx.ctx.store.project_path("app").unwrap();
    let main = corral_git::rev_sha(&repo, "main").unwrap();
    let external = fx.ctx.store.root().join("external-worktree");
    corral_git::worktree_add(&repo, &external, "external", &main).unwrap();
    std::fs::write(external.join("external.txt"), "external work\n").unwrap();
    corral_git::commit_all(&external, "external").unwrap();
    let receipt = corral_git::rev_sha(&external, "HEAD").unwrap();
    let (plan, consumer) = consumer(&fx, &receipt);
    fx.grant(&[&plan]);
    // When ordinary sync evaluates the proposed initial Plan base.
    crate::sync_at(&fx.ctx, Fixture::now()).unwrap();
    // Then accepted-but-absent work cannot create a target or launch a consumer.
    let waiting = fx.ctx.store.get(&consumer.id).unwrap();
    assert_eq!(waiting.status, Status::Backlog);
    assert!(waiting.runs.is_empty() && waiting.launch.is_none());
    assert!(corral_git::rev_sha(&repo, &format!("refs/heads/{TARGET}")).is_err());
    assert_eq!(corral_git::rev_sha(&repo, "main").unwrap(), main);
}

#[test]
fn automatic_backlog_does_not_recreate_a_deleted_initialized_plan_ref() {
    // Given the same eligible prerequisite, but an owned target whose ref was deleted.
    let mut fx = Fixture::new();
    let repo = fx.ctx.store.project_path("app").unwrap();
    let main = corral_git::rev_sha(&repo, "main").unwrap();
    let (plan, consumer) = consumer(&fx, &main);
    let target = {
        let _lock = corral_git::landing_lock(&repo).unwrap();
        crate::plan_target::prepare_dispatch(&fx.ctx, &consumer)
            .unwrap()
            .unwrap()
            .0
    };
    corral_git::worktree_remove(&repo, &target.checkout, false).unwrap();
    git(&repo, &["branch", "-D", TARGET]);
    fx.grant(&[&plan]);
    // When sync inspects the missing owned ref.
    crate::sync_at(&fx.ctx, Fixture::now()).unwrap();
    // Then it retains the ownership journal and refuses to recreate history or launch.
    let waiting = fx.ctx.store.get(&consumer.id).unwrap();
    assert_eq!(waiting.status, Status::Backlog);
    assert!(waiting.runs.is_empty() && waiting.launch.is_none());
    assert!(crate::plan_target::prepare_dispatch(&fx.ctx, &waiting).is_err());
    assert!(corral_git::rev_sha(&repo, &format!("refs/heads/{TARGET}")).is_err());
    assert!(!target.checkout.exists());
    assert_eq!(corral_git::rev_sha(&repo, "main").unwrap(), main);
}

#[test]
fn automatic_backlog_does_not_adopt_an_unowned_existing_plan_ref() {
    // Given a branch at the expected commit, but no controller ownership claim.
    let mut fx = Fixture::new();
    let repo = fx.ctx.store.project_path("app").unwrap();
    let main = corral_git::rev_sha(&repo, "main").unwrap();
    let (plan, consumer) = consumer(&fx, &main);
    git(&repo, &["branch", TARGET, &main]);
    fx.grant(&[&plan]);
    // When automatic admission considers the otherwise eligible consumer.
    crate::sync_at(&fx.ctx, Fixture::now()).unwrap();
    // Then it cannot adopt that branch or launch against it.
    let waiting = fx.ctx.store.get(&consumer.id).unwrap();
    assert_eq!(waiting.status, Status::Backlog);
    assert!(waiting.runs.is_empty() && waiting.launch.is_none());
    assert_eq!(corral_git::rev_sha(&repo, TARGET).unwrap(), main);
    assert!(!repo.join(".worktrees").exists());
}
