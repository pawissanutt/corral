//! Real board sync, gate shell, and independent reviewer in plugin-free corral-test.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use corral_core::{
    AgentCmd, AutoStage, CheckpointUpdate, OperatingAuthority, OperatingSelection, Status, Store,
    Verdict,
};
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};
use notify::{RecursiveMode, Watcher};

#[path = "resource_support/mod.rs"]
mod resource_support;

fn command(program: &str, root: &Path, args: &[&str]) -> String {
    let output = Command::new(program)
        .current_dir(root)
        .env("CORRAL_ROOT", root)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn git(root: &Path, args: &[&str]) -> String {
    command("git", root, args)
}

fn cli(root: &Path, args: &[&str]) -> String {
    command(env!("CARGO_BIN_EXE_corral"), root, args)
}

struct Fixture {
    ctx: Ctx,
    id: String,
    _tmp: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Discard releases only the fixture's saved worker, gate and reviewer panes.
        if let Err(error) = corral_ops::discard(&self.ctx, &self.id) {
            eprintln!("isolated automation cleanup: {error:#}");
        }
    }
}

fn fixture() -> Option<Fixture> {
    let tmp = tempfile::tempdir().unwrap();
    let wrapper = tmp.path().join("live-herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../corral-ops/tests/support/live-herdr.sh"),
        &wrapper,
    )
    .unwrap();
    let herdr = corral_herdr::Herdr::with_bin(wrapper, Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: plugin-free corral-test server unavailable");
        return None;
    }
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-qb", "main"]);
    git(&app, &["config", "user.name", "corral test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    git(&app, &["config", "commit.gpgsign", "false"]);
    git(&app, &["commit", "--allow-empty", "-qm", "init"]);
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.herdr.session = Some("corral-test".into());
    store.config.agents.insert(
        "worker".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    // A separate real terminal process checks the committed candidate and records
    // an attempt-bound verdict. The driving test never submits a verdict.
    store.config.agents.insert(
        "independent".into(),
        AgentCmd {
            command: format!(
                "test \"$(git -C '{{worktree}}' show HEAD:widget.txt)\" = spins && \
                 attempt=$('{}' --root '{}' show {{task}} --json | jq -er '.runs[-1].review | .identity.attempt + \":\" + (.route.callback // .route.attempt)') && \
                 '{}' --root '{}' verdict {{task}} approve --attempt \"$attempt\" --notes 'Independent fixture checked the committed widget' </dev/null",
                env!("CARGO_BIN_EXE_corral"),
                tmp.path().display(),
                env!("CARGO_BIN_EXE_corral"),
                tmp.path().display(),
            ),
        },
    );
    store.config.review.gate = Some(vec!["test \"$(git show HEAD:widget.txt)\" = spins".into()]);
    store.config.review.reviewer = Some("independent".into());
    store.config.review.remove_worktree = Some(false);
    resource_support::configure(&mut store.config);
    store.save_config().unwrap();
    let ctx = Ctx {
        store,
        herdr,
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
    };
    let task = ctx
        .store
        .create("T-204 real board candidate", Some("app"), Status::Ready, "")
        .unwrap();
    assert!(matches!(
        corral_ops::dispatch(
            &ctx,
            &task.id,
            &DispatchOpts {
                profile: Some("worker".into()),
                ..Default::default()
            }
        )
        .unwrap(),
        DispatchOutcome::Started { .. }
    ));
    let task = ctx.store.get(&task.id).unwrap();
    let run = task.current_run().unwrap();
    std::fs::write(run.worktree.join("widget.txt"), "spins\n").unwrap();
    git(&run.worktree, &["add", "widget.txt"]);
    git(&run.worktree, &["commit", "-qm", "widget"]);
    let progress = corral_ops::progress_readback(&ctx, &task.id).unwrap();
    corral_ops::save_checkpoint(
        &ctx,
        &task.id,
        &CheckpointUpdate {
            task: task.id.clone(),
            run: progress.run,
            spec_revision: progress.spec_revision,
            expected_revision: 0,
            progress_revision: 0,
            head: progress.head,
            base: progress.base,
            tree: progress.tree,
            criterion_ids: vec![],
            evidence_refs: vec![],
            next_action: "Automatic independent review".into(),
            decisions: vec![],
            waits: vec![],
            resources: vec![],
        },
    )
    .unwrap();
    corral_ops::done(
        &ctx,
        &task.id,
        "Verified: committed fixture\nNot verified: none",
    )
    .unwrap();
    Some(Fixture {
        ctx,
        id: task.id,
        _tmp: tmp,
    })
}

fn receive_until(
    receiver: &mpsc::Receiver<notify::Result<notify::Event>>,
    predicate: impl Fn() -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !predicate() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("automation state deadline expired");
        receiver
            .recv_timeout(remaining)
            .expect("expected automation filesystem event")
            .expect("automation filesystem watcher");
    }
}

#[test]
fn isolated_board_explains_assist_then_gates_and_reviews_under_confirmed_policy() {
    // Given a real completed task, with subscriptions armed before any auto launch.
    let Some(mut fx) = fixture() else {
        return;
    };
    println!("REAL_BOARD_FIXTURE {}", fx.ctx.store.root().display());
    let (tx, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // State probes read task files. Access notifications would feed those
        // reads back into the subscription and grow an unbounded event queue.
        if event
            .as_ref()
            .is_ok_and(|event| matches!(event.kind, notify::EventKind::Access(_)))
        {
            return;
        }
        tx.send(event).unwrap();
    })
    .unwrap();
    watcher
        .watch(fx.ctx.store.dir(), RecursiveMode::Recursive)
        .unwrap();

    // When the board's sync path runs in Assist, then with an owner-confirmed scope.
    cli(fx.ctx.store.root(), &["sync"]);
    let status = cli(fx.ctx.store.root(), &["autopilot", "status", "--json"]);
    let brief = cli(fx.ctx.store.root(), &["pm", "brief", "--json"]);
    let next = cli(fx.ctx.store.root(), &["pm", "next", "--json"]);
    let show = cli(fx.ctx.store.root(), &["show", &fx.id, "--json"]);
    let reason = corral_ops::automation_blocker(&fx.ctx, &fx.ctx.store.get(&fx.id).unwrap())
        .unwrap()
        .unwrap();
    let task_json: serde_json::Value = serde_json::from_str(&show).unwrap();
    assert_eq!(task_json["automation_blocker"], reason);
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["mode"], "assist");
    assert!(status["effective"]["authorization"].is_null());
    assert!(status["actions"].as_array().unwrap().is_empty());
    assert!(
        brief.contains("no matching confirmed workspace grant"),
        "{brief}"
    );
    assert!(
        next.contains("no matching confirmed workspace grant"),
        "{next}"
    );
    assert_eq!(
        fx.ctx
            .store
            .get(&fx.id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate,
        None
    );
    println!("ASSIST_BLOCKED {}: {reason}", fx.id);
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&OperatingSelection {
            tasks: [fx.id.clone()].into(),
            authority: OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "isolated fixture owner", jiff::Timestamp::now())
        .unwrap();
    println!("CONFIRMED_DIGEST {}", preview.id);
    cli(fx.ctx.store.root(), &["sync"]);
    println!("WAIT_GATE_RECEIPT {}", fx.id);
    receive_until(&receiver, || {
        corral_ops::gate_paths(&fx.ctx, &fx.id, "R1")
            .result
            .is_file()
    });
    cli(fx.ctx.store.root(), &["sync"]);
    println!("WAIT_INDEPENDENT_VERDICT {}", fx.id);
    receive_until(&receiver, || {
        fx.ctx
            .store
            .get(&fx.id)
            .unwrap()
            .current_run()
            .unwrap()
            .review
            .as_ref()
            .is_some_and(|review| review.verdict == Some(Verdict::Approve))
    });
    cli(fx.ctx.store.root(), &["sync"]);

    // Then real gate and independent reviewer processes reach reviewed automatically.
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(
        run.gate.as_ref().unwrap().result,
        corral_core::GateResult::Pass
    );
    assert_eq!(run.review.as_ref().unwrap().verdict, Some(Verdict::Approve));
    assert_eq!(run.autopilot.as_ref().unwrap().stage, AutoStage::Reviewed);
    assert_eq!(task.status, Status::Review);
    let events = fx.ctx.store.events().unwrap();
    for kind in [
        "automation_blocked",
        "automation_resumed",
        "gate",
        "review_started",
        "verdict",
    ] {
        assert!(events.iter().any(|event| event.kind == kind), "{kind}");
    }
    println!(
        "REAL_BOARD_PASS {} gate=pass reviewer=independent verdict=approve stage=reviewed",
        fx.id
    );
    // Keep watcher alive through all assertions; teardown stops its callback first.
    drop(watcher);
}
