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
        screen.contains("outcome") && screen.contains(&format!("corral/{}", fx.id)),
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
