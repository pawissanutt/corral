use corral_core::{Accent, Config, Status, Store};

use super::SettingsView;
use super::model::{Field, SECTIONS, Section};
use super::preferences::Preference;
use super::tests::{c, fixture, render_at};
use crate::{Action, View};

#[test]
fn preferences_are_staged_and_saved_when_controls_are_changed() {
    // Given the Preferences section with an unrelated concurrent edit.
    let fx = fixture();
    let mut view = SettingsView::new(&fx.ctx);
    view.set_section(
        SECTIONS
            .iter()
            .position(|s| *s == Section::Preferences)
            .unwrap(),
    );
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("ui.mouse", "false"))
        .unwrap();
    // When the existing settings controls stage and save the preferences.
    for (preference, value) in [
        (Preference::Accent, "magenta"),
        (Preference::Compact, "on"),
        (Preference::Wip, "off"),
        (Preference::Column(Status::Done), "off"),
        (Preference::NotifyNeedsMe, "on"),
        (Preference::NotifyReview, "on"),
        (Preference::AutoDispatch, "off"),
    ] {
        view.set(&fx.ctx, &Field::Preference(preference), value)
            .unwrap();
    }
    view.set(&fx.ctx, &Field::KeyBinding("capture".into()), "z")
        .unwrap();
    assert!(!Store::open(&fx.root).unwrap().config.ui.compact_cards);
    assert!(matches!(view.on_key(c('s'), &fx.ctx), Action::Toast(_)));
    // Then the persisted config reflects every changed control, retaining concurrent edits.
    let cfg = Store::open(&fx.root).unwrap().config;
    assert_eq!(cfg.ui.accent, Accent::Magenta);
    assert!(cfg.ui.compact_cards && cfg.ui.notify_needs_me && cfg.ui.notify_review);
    assert!(!cfg.ui.show_wip && !cfg.ui.mouse && !cfg.ui.columns.contains(&Status::Done));
    assert_eq!(cfg.ui.bound_key('n'), 'z');
    assert!(cfg.dispatch.paused);
    assert!(!view.dirty());
}

#[test]
fn preferences_keyboard_and_mouse_toggle_the_same_field() {
    // Given the first Preferences control and a selected compact-card field.
    let fx = fixture();
    let mut keyboard = SettingsView::new(&fx.ctx);
    keyboard.set_section(7);
    keyboard.select_field(&Field::Preference(Preference::Compact));
    let mut mouse = SettingsView::new(&fx.ctx);
    mouse.set_section(7);
    render_at(&mut mouse, &fx.ctx, 100, 30);
    let row = mouse
        .rows()
        .iter()
        .position(|r| r.field == Field::Preference(Preference::Compact))
        .unwrap();
    // When keyboard and mouse independently activate the same toggle.
    keyboard.on_key(c(' '), &fx.ctx);
    mouse.click(super::fields::Target::Pill(row, 0), &fx.ctx);
    // Then both produce the same staged value, without persisting.
    assert!(keyboard.draft.ui.compact_cards);
    assert_eq!(keyboard.draft, mouse.draft);
    assert!(!Store::open(&fx.root).unwrap().config.ui.compact_cards);
}

#[test]
fn invalid_key_draft_is_retained_when_save_is_rejected() {
    // Given capture colliding with the default dispatch key.
    let fx = fixture();
    let mut view = SettingsView::new(&fx.ctx);
    view.set(&fx.ctx, &Field::KeyBinding("capture".into()), "d")
        .unwrap();
    // When save validates the whole staged key map.
    let result = view.save(&fx.ctx);
    // Then the file is untouched and the editable draft remains available.
    assert!(result.is_err());
    assert!(view.dirty());
    assert!(Store::open(&fx.root).unwrap().config.ui.keys.is_empty());
}

#[test]
fn preference_columns_keep_a_usable_board_when_last_column_is_disabled() {
    // Given only Ready visible.
    let root = tempfile::tempdir().unwrap();
    let mut cfg = Config::default_for(root.path());
    cfg.ui.columns = vec![Status::Ready];
    // When the last column is disabled.
    super::model::set_value(
        &mut cfg,
        &Field::Preference(Preference::Column(Status::Ready)),
        "off",
    )
    .unwrap();
    // Then configuration validation prevents the empty board layout from being saved.
    assert!(
        cfg.validate(root.path())
            .unwrap_err()
            .0
            .iter()
            .any(|p| p.starts_with("ui.columns"))
    );
}
