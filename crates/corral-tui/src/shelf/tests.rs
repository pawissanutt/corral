use corral_core::Status;
use ratatui::crossterm::event::KeyCode;

use crate::artifacts::panels_fixture::{Fixture, fixture, key, render};
use crate::detail::DetailView;
use crate::history::HistoryView;
use crate::review::ReviewView;
use crate::{Action, View};

fn pins(fx: &Fixture) -> Vec<String> {
    fx.ctx
        .store
        .pins()
        .unwrap()
        .into_iter()
        .map(|p| p.target)
        .collect()
}

fn artifact_ref(fx: &Fixture) -> String {
    let t = fx.ctx.store.get(&fx.id).unwrap();
    super::artifact_ref(&fx.id, &t.artifacts[0])
}

fn toast_title(a: Action) -> String {
    match a {
        Action::Toast(t) => t.title,
        other => panic!("expected a toast, got {other:?}"),
    }
}

#[test]
fn detail_star_pins_the_task_or_the_selected_artifact() {
    let fx = fixture();
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    assert!(v.hints().contains(&("*", "pin")));
    assert_eq!(
        toast_title(v.on_key(key(KeyCode::Char('*')), &fx.ctx)),
        "★ Pinned #1"
    );
    v.refresh(&fx.ctx);
    let text = render(&mut v, &fx.ctx);
    assert!(
        text.contains(&format!("{} ★ Witness shard cache", fx.id)),
        "{text}"
    );

    v.on_key(key(KeyCode::Char('3')), &fx.ctx);
    assert!(
        v.hints().contains(&("*", "pin")),
        "the artifact is not pinned yet"
    );
    v.on_key(key(KeyCode::Char('*')), &fx.ctx);
    v.refresh(&fx.ctx);
    assert_eq!(pins(&fx), [fx.id.clone(), artifact_ref(&fx)]);
    let text = render(&mut v, &fx.ctx);
    assert!(text.contains("★ keep report"), "{text}");
    assert!(v.hints().contains(&("*", "unpin")));
}

#[test]
fn review_star_pins_the_focused_artifact_and_unpins_it() {
    let fx = fixture();
    let mut v = ReviewView::new(&fx.ctx, &fx.id).unwrap();
    v.on_key(key(KeyCode::Tab), &fx.ctx);
    v.on_key(key(KeyCode::Char('*')), &fx.ctx);
    v.refresh(&fx.ctx);
    assert_eq!(pins(&fx), [artifact_ref(&fx)]);
    let text = render(&mut v, &fx.ctx);
    assert!(text.contains("★ keep report"), "{text}");
    assert_eq!(
        toast_title(v.on_key(key(KeyCode::Char('*')), &fx.ctx)),
        "Unpinned"
    );
    assert!(pins(&fx).is_empty());
}

#[test]
fn history_star_pins_the_selected_finished_task() {
    let fx = fixture();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    let mut v = HistoryView::new(&fx.ctx).unwrap();
    v.on_key(key(KeyCode::Char('*')), &fx.ctx);
    v.refresh(&fx.ctx);
    assert_eq!(pins(&fx), std::slice::from_ref(&fx.id));
    let text = render(&mut v, &fx.ctx);
    assert!(text.contains("★ Witness shard cache"), "{text}");
}
