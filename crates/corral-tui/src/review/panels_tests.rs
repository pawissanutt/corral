use ratatui::crossterm::event::KeyCode;

use super::ReviewView;
use crate::artifacts::panels_fixture::{fixture, key, render};
use crate::choice::tests::{click, draw, line, locate, overridden};
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

#[test]
fn panels_review_retry_sheet_defaults_to_the_current_run() {
    let fx = fixture();
    let ctx = &fx.ctx;
    ctx.store
        .update(&fx.id, |t| {
            if let Some(r) = t.current_run_mut() {
                r.harness = Some("omo".into());
                r.model = Some("anthropic/opus-4".into());
                r.effort = Some("high".into());
            }
            Ok(())
        })
        .unwrap();
    let mut v = ReviewView::new(ctx, &fx.id).unwrap();
    let screen = render(&mut v, ctx);
    assert!(
        screen.contains("R1 omo·anthropic/opus-4·high done"),
        "runs history compares harness·model·effort\n{screen}"
    );

    assert_eq!(v.on_key(key(KeyCode::Char('R')), ctx), Action::None);
    let (screen, hits) = draw(&mut v, ctx);
    assert!(screen.contains(&format!("retry {}", fx.id)), "{screen}");
    assert!(overridden(&screen, "[omo]"), "{screen}");
    assert!(
        line(&screen, "model ").contains("anthropic/opus-4")
            && line(&screen, "model ").contains("override"),
        "{screen}"
    );
    assert!(line(&screen, "effort ").contains("[high]"), "{screen}");
    assert!(
        screen.contains("R1 · omo (omo·anthropic/opus-4·high)"),
        "previous run shown for comparison\n{screen}"
    );
    let hints = v.hints();
    assert_eq!(
        &hints[..3],
        &[("h/l", "profile"), ("M", "model"), ("e", "effort")]
    );
    assert!(hints.contains(&("⏎", "retry")), "{hints:?}");

    let (x, y) = locate(&screen, "effort ", " max ");
    click(&mut v, ctx, &hits, (x + 1, y), false);
    let screen = render(&mut v, ctx);
    assert!(line(&screen, "effort ").contains("[max]"), "{screen}");
    v.on_key(key(KeyCode::Char('e')), ctx);
    let screen = render(&mut v, ctx);
    assert!(
        !line(&screen, "effort ").contains('['),
        "e past max drops the override; omo has no effort of its own\n{screen}"
    );

    v.on_key(key(KeyCode::Esc), ctx);
    let screen = render(&mut v, ctx);
    assert!(!screen.contains(&format!("retry {}", fx.id)), "{screen}");
    assert_eq!(v.on_key(key(KeyCode::Esc), ctx), Action::Back);
}
