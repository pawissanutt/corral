use corral_core::{Status, StuckProcess};
use ratatui::crossterm::event::KeyCode;

use super::DetailView;
use crate::artifacts::panels_fixture::{Fixture, fixture, key, render};
use crate::{Action, Tone, View};

/// No live process has this pid (above Linux's pid_max), so ops must refuse to kill it.
const GONE_PID: u32 = u32::MAX;

fn set_stuck(fx: &Fixture, stuck: Option<StuckProcess>) {
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.status = Status::Running;
            if let Some(r) = t.current_run_mut() {
                r.outcome = None;
                r.ended = None;
                r.idle_no_report = true;
                r.stuck = stuck;
            }
            Ok(())
        })
        .unwrap();
}

fn stuck(pid: u32, argv: &[&str]) -> StuckProcess {
    StuckProcess {
        pid,
        argv: argv.iter().map(|a| a.to_string()).collect(),
        started: "2026-01-01T11:58:00Z".into(),
    }
}

#[test]
fn detail_unstick_confirms_the_captured_process() {
    let fx = fixture();
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    let none = v.on_key(key(KeyCode::Char('u')), &fx.ctx);
    assert!(
        matches!(&none, Action::Toast(t) if t.tone == Tone::Info),
        "{none:?}"
    );

    let shown = stuck(GONE_PID, &["sleep", "1000"]);
    set_stuck(&fx, Some(shown.clone()));
    v.refresh(&fx.ctx);
    v.on_key(key(KeyCode::Char('2')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("⚠ stuck: sleep 1000"), "{screen}");
    assert!(
        screen.contains(&format!("pid {GONE_PID}")) && !screen.contains("idle, no report"),
        "stuck replaces the idle warning\n{screen}"
    );
    assert!(v.hints().contains(&("u", "unstick")));

    v.on_key(key(KeyCode::Char('u')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(
        screen.contains(&format!("Unstick run R1? Kill pid {GONE_PID}:"))
            && screen.contains("[Unstick]"),
        "{screen}"
    );
    assert_eq!(v.on_key(key(KeyCode::Esc), &fx.ctx), Action::None);
    assert!(!render(&mut v, &fx.ctx).contains("Unstick run R1?"));

    v.on_key(key(KeyCode::Char('u')), &fx.ctx);
    set_stuck(&fx, Some(stuck(1, &["other"])));
    v.refresh(&fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(
        screen.contains(&format!("Kill pid {GONE_PID}:")) && screen.contains("   sleep 1000"),
        "the popup keeps the process it opened with\n{screen}"
    );
    let out = v.on_key(key(KeyCode::Enter), &fx.ctx);
    assert!(
        matches!(&out, Action::Toast(t) if t.tone == Tone::Bad),
        "a changed process is refused, never swapped in\n{out:?}"
    );
    let run = fx.ctx.store.get(&fx.id).unwrap().runs.remove(0);
    assert_eq!(run.stuck, Some(stuck(1, &["other"])));
    assert!(!render(&mut v, &fx.ctx).contains("Unstick run R1?"));
}

#[test]
fn panels_detail_renders_and_switches_tabs() {
    let fx = fixture();
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("Witness shard cache"), "{screen}");
    assert!(
        screen.contains("1 Spec") && screen.contains("2 Runs") && screen.contains("4 Diff"),
        "{screen}"
    );
    assert!(
        screen.contains("Cache shard reads per character."),
        "{screen}"
    );
    assert!(
        screen.contains("Live") && screen.contains("No live run."),
        "{screen}"
    );

    v.on_key(key(KeyCode::Char('2')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(
        screen.contains("outcome")
            && screen.contains("harness·model·effort")
            && screen.contains(&format!("corral/{}", fx.id)),
        "{screen}"
    );
    assert!(
        !screen.contains("Cache shard reads per character."),
        "{screen}"
    );

    v.on_key(key(KeyCode::Char('3')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("QA checklist"), "{screen}");

    v.on_key(key(KeyCode::Char('4')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(
        screen.contains("shard_cache.rs") && screen.contains("+pub struct ShardCache;"),
        "{screen}"
    );

    assert_eq!(
        v.on_key(key(KeyCode::Char('a')), &fx.ctx),
        Action::Jump(fx.id.clone())
    );
    assert_eq!(v.on_key(key(KeyCode::Esc), &fx.ctx), Action::Back);
}

#[test]
fn done_task_detail_reads_snapshots_after_the_worktree_is_removed() {
    // Given a reviewed task whose final diff was snapshotted, approved with its worktree removed.
    let fx = fixture();
    let run = fx.ctx.store.get(&fx.id).unwrap().runs[0].clone();
    let diff = corral_git::diff_text(&run.worktree, &run.base_sha).unwrap();
    let diff_file = fx.root.join("final.diff");
    std::fs::write(&diff_file, diff).unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &diff_file,
        Some(corral_core::ArtifactKind::Diff),
        Some("Final diff +3 −0"),
    )
    .unwrap();
    corral_ops::approve(
        &fx.ctx,
        &fx.id,
        corral_ops::ApproveStrategy::FastForward,
        true,
        false,
    )
    .unwrap();
    assert!(!run.worktree.exists(), "worktree removed");

    // When the Done task's detail opens on its Diff tab, then on Artifacts.
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    v.on_key(key(KeyCode::Char('4')), &fx.ctx);
    let diff_tab = render(&mut v, &fx.ctx);
    v.on_key(key(KeyCode::Char('3')), &fx.ctx);
    let artifacts_tab = render(&mut v, &fx.ctx);

    // Then the diff and its stat come from the snapshot, and the artifacts are still listed.
    assert!(
        diff_tab.contains("Done · P2")
            && diff_tab.contains("1 files  +3 −0")
            && diff_tab.contains("+pub struct ShardCache;"),
        "{diff_tab}"
    );
    assert!(
        artifacts_tab.contains("QA checklist") && artifacts_tab.contains("Final diff +3 −0"),
        "{artifacts_tab}"
    );
    v.on_key(key(KeyCode::Char('2')), &fx.ctx);
    let runs_tab = render(&mut v, &fx.ctx);
    assert!(
        runs_tab.contains("(removed)") && runs_tab.contains("Added the shard cache."),
        "{runs_tab}"
    );
}
