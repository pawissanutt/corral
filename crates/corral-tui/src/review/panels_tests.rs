use ratatui::crossterm::event::KeyCode;

use super::ReviewView;
use crate::artifacts::panels_fixture::{fixture, key, render};
use crate::{Action, View};

#[test]
fn panels_review_renders_and_opens_merge_sheet() {
    let fx = fixture();
    let mut v = ReviewView::new(&fx.ctx, &fx.id).unwrap();
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains(&format!("Review {}", fx.id)), "{screen}");
    assert!(
        screen.contains("Summary") && screen.contains("Files 1") && screen.contains("Artifacts"),
        "{screen}"
    );
    assert!(screen.contains("shard_cache.rs"), "{screen}");
    assert!(screen.contains("Added the shard cache."), "{screen}");
    assert!(screen.contains("QA checklist"), "{screen}");

    assert_eq!(v.on_key(key(KeyCode::Char('a')), &fx.ctx), Action::None);
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains(&format!("approve {}", fx.id)), "{screen}");
    assert!(screen.contains("fast-forward main"), "{screen}");
    assert!(
        screen.contains("rebase onto main, then fast-forward"),
        "{screen}"
    );
    assert!(screen.contains("keep branch, mark done only"), "{screen}");
    assert!(screen.contains("remove worktree"), "{screen}");
    assert_eq!(v.hints()[0], ("j/k", "strategy"));

    v.on_key(key(KeyCode::Esc), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(!screen.contains("keep branch, mark done only"), "{screen}");

    v.on_key(key(KeyCode::Char('c')), &fx.ctx);
    assert!(v.captures_text());
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("changes ›"), "{screen}");
    v.on_key(key(KeyCode::Esc), &fx.ctx);
    assert_eq!(v.on_key(key(KeyCode::Esc), &fx.ctx), Action::Back);
}
