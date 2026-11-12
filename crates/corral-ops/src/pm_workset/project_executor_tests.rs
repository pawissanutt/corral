//! Design F: acting in one project cannot launch, repair or land anything in another.
#![cfg(unix)] // The fixture executes the POSIX fake terminal engine.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

use corral_core::{AgentCmd, OperatingAuthority, OperatingSelection, Status, Store};
use corral_herdr::Herdr;

use crate::{Ctx, DispatchOpts, PmActionOutcome};

fn git(repo: &Path, args: &[&str]) {
    let result = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// Two projects with their own repositories, one fake profile and the fake terminal engine.
fn fixture() -> (tempfile::TempDir, Ctx) {
    let tmp = tempfile::tempdir().unwrap();
    for project in ["alpha", "beta"] {
        let repo = tmp.path().join(project);
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Corral Test"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        git(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "init",
            ],
        );
    }
    let mut store = Store::init_with(tmp.path(), |store| {
        std::fs::write(
            store.config_path(),
            "[projects.alpha]\npath = \"alpha\"\n[projects.beta]\npath = \"beta\"\n[pm]\nagent = \"fake\"\n",
        )?;
        Ok(())
    })
    .unwrap();
    store.config.agents.insert(
        "fake".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    for project in ["alpha", "beta"] {
        store
            .config
            .projects
            .get_mut(project)
            .unwrap()
            .default_profile = Some("fake".into());
    }
    store.config.routing.rules.clear();
    store.config.limits.running = 8;
    store.config.limits.review = 2;
    store.save_config().unwrap();
    let engine = tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/operating_launch");
    std::fs::copy(source.join("herdr.py"), engine.join("herdr.py")).unwrap();
    let bin = engine.join("herdr");
    std::fs::copy(source.join("herdr.sh"), &bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = Ctx {
        store,
        herdr: Herdr::with_bin(bin, Some("corral-test".into())),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    (tmp, ctx)
}

fn fresh(ctx: &Ctx) -> Ctx {
    Ctx {
        store: Store::open(ctx.store.root()).unwrap(),
        ..ctx.clone()
    }
}

/// Every byte a scoped run must not touch: the tasks (status, queue membership, runs and
/// launch reservation) and their events.
fn project_state(ctx: &Ctx, ids: &[&str]) -> Vec<u8> {
    let tasks: Vec<_> = ids.iter().map(|id| ctx.store.get(id).unwrap()).collect();
    let events: Vec<_> = ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| {
            event
                .task
                .as_deref()
                .is_some_and(|task| ids.contains(&task))
        })
        .collect();
    serde_json::to_vec(&(tasks, events)).unwrap()
}

#[test]
fn acting_in_one_project_leaves_another_projects_tasks_queue_and_runs_unchanged() {
    // Given Ready work awaiting dispatch in alpha and beta, plus an owner-queued beta task.
    let (_tmp, mut ctx) = fixture();
    let create = |title: &str, project: &str| {
        ctx.store
            .create(
                title,
                Some(project),
                Status::Ready,
                "## Acceptance\nWorks.\n",
            )
            .unwrap()
    };
    let alpha = create("Alpha work", "alpha");
    let beta = create("Beta work", "beta");
    let queued = create("Beta queued", "beta");
    let preview = ctx
        .store
        .preview_operating_policy(&OperatingSelection {
            tasks: [alpha.id.clone(), beta.id.clone()].into(),
            authority: OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    ctx.store.config = ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
        .unwrap();
    crate::queue::enqueue(&fresh(&ctx), &queued.id, &DispatchOpts::default()).unwrap();
    let workset = crate::pm_workset(&fresh(&ctx)).unwrap();
    let action = workset
        .actions
        .iter()
        .find(|action| action.action.task.as_deref() == Some(alpha.id.as_str()))
        .unwrap();
    assert!(
        workset
            .actions
            .iter()
            .any(|action| action.action.task.as_deref() == Some(beta.id.as_str()))
    );
    let beta_ids = [beta.id.as_str(), queued.id.as_str()];
    let before = project_state(&ctx, &beta_ids);

    // When the PM acts on alpha's action.
    let result = crate::pm_act(&fresh(&ctx), &action.id).unwrap();

    // Then alpha is dispatched as today and every beta byte is unchanged.
    assert!(matches!(result.outcome, PmActionOutcome::Progressed));
    assert_eq!(ctx.store.get(&alpha.id).unwrap().status, Status::Running);
    assert_eq!(project_state(&ctx, &beta_ids), before);

    // And an ordinary workspace sync does act on beta, so beta was actionable all along.
    crate::sync(&fresh(&ctx)).unwrap();
    for id in beta_ids {
        assert_eq!(ctx.store.get(id).unwrap().status, Status::Running, "{id}");
    }
}
