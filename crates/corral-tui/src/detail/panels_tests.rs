use ratatui::crossterm::event::KeyCode;

use super::DetailView;
use crate::artifacts::panels_fixture::{fixture, key, render};
use crate::{Action, View};

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
