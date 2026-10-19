use super::*;
use corral_core::{OperatingAuthority, OperatingSelection, Origin, Run};
use std::path::Path;
use std::process::Stdio;

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

fn fixture() -> (tempfile::TempDir, Ctx, Task, Vec<Task>) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let source = dir.path().join("plan.md");
    std::fs::write(
        &source,
        "---\ntitle: Plan\nproject: app\nready: true\napproved_scope:\n  goal: Ship feature\n  acceptance:\n    - {criterion: C1, scenario: check, expected: pass}\n    - {criterion: C2, scenario: end-to-end, expected: pass together}\ntasks:\n  - {key: accepted, title: Accepted}\n  - {key: running, title: Running}\n  - {key: review, title: Review}\n  - {key: waiting, title: Waiting, depends: [accepted]}\n  - {key: missing, title: Missing}\n---\n",
    )
    .unwrap();
    let imported = crate::import_plan(&ctx, &source, false, Origin::Agent).unwrap();
    for (child, status) in imported.tasks.iter().zip([
        Status::Done,
        Status::Running,
        Status::Review,
        Status::Ready,
        Status::Dropped,
    ]) {
        ctx.store
            .update(&child.id, |task| {
                task.status = status;
                Ok(())
            })
            .unwrap();
    }
    (dir, ctx, imported.tracker, imported.tasks)
}

fn grant(ctx: &Ctx, plan: &Task) {
    let selection = OperatingSelection {
        tasks: [plan.id.clone()].into(),
        authority: OperatingAuthority {
            dispatch: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let preview = ctx.store.preview_operating_policy(&selection).unwrap();
    ctx.store
        .confirm_operating_policy(
            &preview,
            "fixture owner",
            "2026-10-19T00:00:00Z".parse().unwrap(),
        )
        .unwrap();
}

fn accepted(ctx: &Ctx, child: &Task, sha: &str, strategy: &str) {
    ctx.store
        .update(&child.id, |task| {
            task.status = Status::Done;
            let run: Run = serde_json::from_value(json!({
                "id":"R1", "agent":"fixture", "worktree":"/unused",
                "branch":"corral/fixture", "base":"main", "base_sha":sha,
                "started":"2026-10-19T00:00:00Z", "outcome":"done",
            }))
            .unwrap();
            task.runs = vec![run];
            Ok(())
        })
        .unwrap();
    ctx.store
        .log_event(
            Some(&child.id),
            "approved",
            json!({
                "run":"R1", "sha":sha, "strategy":strategy, "base":"main",
            }),
        )
        .unwrap();
}

#[test]
fn projection_names_every_bucket_and_unmapped_criteria_without_writing() {
    // Given one required member in each lifecycle and one exact criterion mapping.
    let (_dir, ctx, plan, children) = fixture();
    ctx.store
        .update(&children[0].id, |task| {
            task.completion = Some(serde_json::from_value(json!({
            "version":1, "spec_digest":"fixture", "reviewed_by":"owner", "audited":false,
            "require_gate":false, "criteria":[
                {"id":"C1", "required":true, "live":false, "scenario":"check", "expected":"pass"}
            ],
        })).unwrap());
            Ok(())
        })
        .unwrap();
    grant(&ctx, &plan);
    let before = ctx.store.list().unwrap();
    let events = ctx.store.events().unwrap();
    // When the shared readback is built from fresh durable records.
    let progress = plan_progress(&ctx, &plan.id).unwrap();
    // Then the denominator is unchanged, including dropped work, with no implicit acceptance.
    assert_eq!(progress.counts.accepted, 1);
    assert_eq!(progress.counts.running, 1);
    assert_eq!(progress.counts.review, 1);
    assert_eq!(progress.counts.waiting, 1);
    assert_eq!(progress.counts.missing, 1);
    assert_eq!(progress.work.len(), 5);
    assert_eq!(progress.coverage[0].tasks, [children[0].id.clone()]);
    assert_eq!(progress.unmapped_criteria.len(), 1);
    assert_eq!(
        progress.unmapped_criteria[0].criterion.as_deref(),
        Some("C2")
    );
    let dependency = &progress.work[3].prerequisites[0];
    assert!(dependency.accepted);
    assert!(!dependency.satisfied);
    assert_eq!(dependency.target, "main");
    assert_eq!(ctx.store.list().unwrap(), before);
    assert_eq!(ctx.store.events().unwrap().len(), events.len());
}

#[test]
fn missing_authority_and_paused_policy_are_visible_in_projection() {
    // Given a Plan whose Ready work has no execution grant.
    let (_dir, ctx, plan, _) = fixture();
    // When projected without a grant and then with an explicitly paused grant.
    let ungranted = plan_progress(&ctx, &plan.id).unwrap();
    grant(&ctx, &plan);
    ctx.store.pause_operating_policy(true).unwrap();
    let paused = plan_progress(&ctx, &plan.id).unwrap();
    // Then both identify missing authorization rather than claiming dispatch readiness.
    assert_eq!(ungranted.work[3].state, PlanWorkState::Missing);
    assert_eq!(paused.work[3].state, PlanWorkState::Missing);
    assert!(paused.work[3].reason.is_some());
}

#[test]
fn accepted_prerequisites_require_receipt_and_ancestry_in_the_exact_target() {
    for strategy in ["fast_forward", "keep_branch"] {
        // Given a Done prerequisite accepted on a branch ahead of main.
        let (_dir, ctx, plan, children) = fixture();
        let repo = ctx.store.project_path("app").unwrap();
        git(&repo, &["checkout", "-qb", "accepted"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "accepted",
            ],
        );
        let sha = corral_git::rev_sha(&repo, "HEAD").unwrap();
        accepted(&ctx, &children[0], &sha, strategy);
        grant(&ctx, &plan);
        // When the prerequisite is read before inclusion.
        let progress = plan_progress(&ctx, &plan.id).unwrap();
        let prerequisite = &progress.work[3].prerequisites[0];
        // Then Done alone cannot admit the consumer.
        assert!(prerequisite.accepted);
        assert!(!prerequisite.satisfied);
        let waiting = ctx.store.get(&children[3].id).unwrap();
        assert!(target_blocker(&ctx.store, &waiting).unwrap().is_some());
        let original_base = corral_git::rev_sha(&repo, "main").unwrap();
        // Once the recorded commit is present, only an actual integration receipt qualifies.
        git(&repo, &["branch", "-f", "main", &sha]);
        let included = plan_progress(&ctx, &plan.id).unwrap();
        assert_eq!(
            included.work[3].prerequisites[0].satisfied,
            strategy == "fast_forward"
        );
        if strategy == "fast_forward" {
            assert!(check_dispatch_base(&ctx.store, &waiting, "main", &original_base).is_err());
            assert!(check_dispatch_base(&ctx.store, &waiting, "main", &sha).is_ok());
        }
    }
}

#[test]
fn malformed_graphs_fail_even_for_terminal_or_unauthorized_members() {
    for failure in ["missing-member", "missing-dependency", "cycle"] {
        // Given a Plan containing a malformed reachable graph.
        let (_dir, ctx, plan, children) = fixture();
        match failure {
            "missing-member" => {
                std::fs::remove_file(&ctx.store.get(&children[4].id).unwrap().path).unwrap()
            }
            "missing-dependency" => {
                ctx.store
                    .update(&children[0].id, |task| {
                        task.depends_on.push("T-999".into());
                        Ok(())
                    })
                    .unwrap();
            }
            "cycle" => {
                ctx.store
                    .update(&children[0].id, |task| {
                        task.depends_on.push(children[3].id.clone());
                        Ok(())
                    })
                    .unwrap();
            }
            _ => unreachable!(),
        }
        // When the projection is requested.
        let error = plan_progress(&ctx, &plan.id).unwrap_err();
        // Then no member or invalid edge is silently discarded.
        assert!(format!("{error:#}").contains(if failure == "cycle" {
            "cycle"
        } else {
            "missing"
        }));
    }
}

#[test]
fn integration_target_metadata_never_launches_a_main_based_consumer() {
    // Given an explicitly selected integration destination with no owned-target executor.
    let (_dir, ctx, plan, children) = fixture();
    let scope = ApprovedScope {
        delivery_target: PlanDeliveryTarget::Integration {
            branch: "feature/plan".into(),
        },
        ..plan.approved_scope.clone().unwrap()
    };
    ctx.store
        .amend_plan_scope(&plan.id, 0, scope, |_, _, _| Ok(()))
        .unwrap();
    // When ordinary admission checks this Plan member.
    let reason = target_blocker(&ctx.store, &ctx.store.get(&children[3].id).unwrap()).unwrap();
    // Then execution remains held instead of interpreting target metadata as a main launch.
    assert!(reason.is_some());
}
