use corral_core::Store;
use ratatui::crossterm::event::KeyCode;

use super::super::SettingsView;
use super::super::model::Field;
use super::super::mouse_tests::{self, click, find, footer, settings_app};
use super::super::tests::{HEAD_COMMENT, SLOT_COMMENT, c, fixture, k, press, render, type_value};
use crate::{Action, Tone, View};

fn saved(action: Action) -> bool {
    matches!(action, Action::Toast(t) if t.tone == Tone::Good)
}

#[test]
fn soft_target_steps_validates_saves_and_unsets_by_keyboard() {
    // Given the fixture's hard ceiling of 3 and an unset soft target.
    let fx = fixture();
    let before = fx.toml();
    let mut v = SettingsView::new(&fx.ctx);
    v.select_field(&Field::RunningTarget);
    let s = render(&mut v, &fx.ctx);
    assert!(s.contains("unset → hard ceiling (3)"), "{s}");
    // When stepped above the ceiling, the value is kept (no clamping) and saving is refused.
    v.on_key(k(KeyCode::Right), &fx.ctx);
    assert_eq!(v.draft.limits.running_target, Some(4));
    let s = render(&mut v, &fx.ctx);
    assert!(
        s.contains("✗ limits.running_target must be within 1..=limits.running"),
        "{s}"
    );
    assert!(!saved(v.on_key(c('s'), &fx.ctx)));
    assert_eq!(fx.toml(), before);
    // When stepped back into range, it saves alone with comments preserved.
    press(&mut v, &fx.ctx, &[k(KeyCode::Left), k(KeyCode::Left)]);
    assert!(saved(v.on_key(c('s'), &fx.ctx)));
    let after = fx.toml();
    assert!(after.contains("running_target = 2"), "{after}");
    assert!(
        after.contains(HEAD_COMMENT) && after.contains(SLOT_COMMENT),
        "{after}"
    );
    assert_eq!(
        after.replace("running_target = 2\n", ""),
        before,
        "only the target changed"
    );
    // When unset with x, the key is removed again and the hard ceiling applies.
    assert!(v.hints().contains(&("x", "unset")), "{:?}", v.hints());
    v.on_key(c('x'), &fx.ctx);
    assert!(saved(v.on_key(c('s'), &fx.ctx)));
    assert_eq!(fx.toml(), before);
    assert_eq!(
        Store::open(&fx.root).unwrap().config.limits.running_target,
        None
    );
}

#[test]
fn wip_zero_is_off_low_wip_is_explained_and_discard_writes_nothing() {
    let fx = fixture();
    let before = fx.toml();
    let mut v = SettingsView::new(&fx.ctx);
    v.select_field(&Field::WipLimit);
    // WIP at or below the soft target is legal, with an explanation.
    type_value(&mut v, &fx.ctx, "2");
    let s = render(&mut v, &fx.ctx);
    assert!(
        s.contains("WIP ≤ target: unfinished work, not slots"),
        "{s}"
    );
    assert!(!s.contains('✗'), "{s}");
    // Zero disables the limit.
    type_value(&mut v, &fx.ctx, "0");
    let s = render(&mut v, &fx.ctx);
    assert!(
        s.contains("auto 3 · hard 3 · WIP off") && !s.contains('✗'),
        "{s}"
    );
    // Discard leaves the file untouched.
    assert_eq!(
        press(&mut v, &fx.ctx, &[k(KeyCode::Esc), c('y')]),
        Action::Back
    );
    assert_eq!(fx.toml(), before);
}

#[test]
fn mouse_only_steps_unsets_and_saves_at_80x24() {
    let (fx, mut app) = settings_app();
    let draw = |app: &mut crate::app::App| mouse_tests::render(app, 80, 24);
    let s = draw(&mut app);
    for want in [
        "Coding concurrency",
        "Hard slot ceiling",
        "Soft coding target",
        "Unfinished work",
        "exceed the target so review never stalls coding",
    ] {
        assert!(s.contains(want), "{want:?} missing at 80x24\n{s}");
    }
    click(&mut app, find(&s, "Soft coding target", " − "));
    let s = draw(&mut app);
    click(&mut app, find(&s, "Unfinished work", " + "));
    let s = draw(&mut app);
    click(&mut app, footer(&s, "s save"));
    let cfg = Store::open(&fx.root).unwrap().config;
    assert_eq!((cfg.limits.running_target, cfg.limits.wip), (Some(2), 7));
    // Save toasts overlap the narrow form; clicking a toast dismisses it.
    loop {
        draw(&mut app);
        let Some((rect, _)) = app
            .hits
            .iter()
            .find(|(_, hit)| matches!(hit, crate::Hit::Toast(_)))
        else {
            break;
        };
        let at = (rect.x, rect.y);
        click(&mut app, at);
    }
    let s = draw(&mut app);
    click(&mut app, find(&s, "Soft coding target", "× unset"));
    let s = draw(&mut app);
    click(&mut app, footer(&s, "s save"));
    let cfg = Store::open(&fx.root).unwrap().config;
    assert_eq!((cfg.limits.running_target, cfg.limits.running), (None, 3));
    assert!(!draw(&mut app).contains("● unsaved"));
}
