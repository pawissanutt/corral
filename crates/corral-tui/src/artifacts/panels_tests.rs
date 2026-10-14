use ratatui::crossterm::event::KeyCode;

use super::ArtifactsView;
use super::panels_fixture::{fixture, key, render};
use crate::{Action, View};

#[test]
fn panels_artifacts_lists_and_filters() {
    let fx = fixture();
    let mut v = ArtifactsView::new(&fx.ctx, None).unwrap();
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("QA checklist"), "{screen}");
    assert!(screen.contains("artifacts · 1 across 1 tasks"), "{screen}");
    assert!(screen.contains("all green"), "preview missing:\n{screen}");

    for c in "zzz".chars() {
        assert_eq!(v.on_key(key(KeyCode::Char(c)), &fx.ctx), Action::None);
    }
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("no artifacts match"), "{screen}");
    assert!(!screen.contains("QA checklist"), "{screen}");

    for _ in 0..3 {
        v.on_key(key(KeyCode::Backspace), &fx.ctx);
    }
    for c in "report app".chars() {
        v.on_key(key(KeyCode::Char(c)), &fx.ctx);
    }
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("QA checklist"), "{screen}");
    assert_eq!(v.on_key(key(KeyCode::Esc), &fx.ctx), Action::Back);
}

#[test]
fn panels_artifacts_only_task_filter() {
    let fx = fixture();
    let mut v = ArtifactsView::new(&fx.ctx, Some(&fx.id)).unwrap();
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("QA checklist"), "{screen}");
    let mut other = ArtifactsView::new(&fx.ctx, Some("T-999")).unwrap();
    let screen = render(&mut other, &fx.ctx);
    assert!(screen.contains("No artifacts yet"), "{screen}");
}
