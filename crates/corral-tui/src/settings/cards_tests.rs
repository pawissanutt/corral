use corral_core::Store;
use ratatui::crossterm::event::KeyCode;

use super::model::Field;
use super::mouse_tests::{click, find, render, screen, settings_app};
use super::tests::{c, fixture, k, render_at};
use super::{SECTIONS, SettingsView};
use crate::View;
use corral_core::CardField;

fn walk_to(v: &mut SettingsView, ctx: &corral_ops::Ctx, field: &Field) {
    v.on_key(c('7'), ctx);
    for _ in 0..v.rows().len() {
        if v.current().is_some_and(|r| r.field == *field) {
            return;
        }
        v.on_key(k(KeyCode::Down), ctx);
    }
    panic!("{field:?} not reachable with \u{2193}");
}

#[test]
fn keyboard_toggles_a_field_previews_it_and_saves_to_corral_toml() {
    let fx = fixture();
    let ctx = &fx.ctx;
    let mut v = SettingsView::new(ctx);
    assert_eq!(SECTIONS.len(), 8);
    walk_to(&mut v, ctx, &Field::CardField(CardField::Tokens));
    v.on_key(c('h'), ctx);
    assert_eq!(v.draft.board.card.fields, ["cost", "tokens"]);
    let s = render_at(&mut v, ctx, 160, 45);
    assert!(
        s.contains("At 160x45") && s.contains("$1.42 \u{b7} 48.2k tok"),
        "{s}"
    );
    v.on_key(c('s'), ctx);
    assert!(!v.dirty());
    let toml = fx.toml();
    assert!(
        toml.contains("[board.card]") && toml.contains("tokens"),
        "{toml}"
    );
    let saved = Store::open(&fx.root).unwrap().config;
    assert_eq!(saved.board.card.fields, ["cost", "tokens"]);
}

#[test]
fn field_order_text_row_rejects_unknown_fields_inline() {
    let fx = fixture();
    let ctx = &fx.ctx;
    let mut v = SettingsView::new(ctx);
    walk_to(&mut v, ctx, &Field::CardFields);
    v.on_key(k(KeyCode::Enter), ctx);
    for ch in ", bogus".chars() {
        v.on_key(c(ch), ctx);
    }
    v.on_key(k(KeyCode::Enter), ctx);
    v.on_key(c('s'), ctx);
    assert!(v.dirty(), "an invalid draft is not saved");
    let s = render_at(&mut v, ctx, 160, 45);
    assert!(s.contains("unknown field 'bogus'"), "{s}");
}

#[test]
fn mouse_picks_the_section_and_toggles_a_field_at_both_sizes() {
    let (_fx, mut app) = settings_app();
    let s = screen(&mut app);
    click(&mut app, find(&s, "Board cards", "Board cards"));
    let s = screen(&mut app);
    assert!(s.contains("At 80x24") && !s.contains("48.2k tok"), "{s}");
    click(&mut app, find(&s, "tokens", " on "));
    let s = screen(&mut app);
    assert!(s.contains("48.2k tok"), "{s}");
    let small = render(&mut app, 80, 24);
    assert!(small.contains("Board cards"), "{small}");
}

#[test]
fn preview_is_the_real_card_at_both_widths_and_follows_the_compact_allocation() {
    let mut cfg = corral_core::Config::default_for(&std::env::temp_dir());
    cfg.board.card.fields = vec!["cost".into(), "tokens".into()];
    let rows = super::card_preview::rows(&cfg);
    let text = |from: usize, to: usize| -> String {
        rows.iter()
            .filter(|r| matches!(r.field, Field::CardPreview(i) if (from..to).contains(&i)))
            .map(|r| r.value.clone())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        text(0, 100).contains("$1.42 \u{b7} 48.2k tok"),
        "wide shows both fields"
    );
    assert!(
        text(100, 200).contains("$1.42") && !text(100, 200).contains("tok"),
        "narrow drops tokens first"
    );
    cfg.board.card.density = corral_core::Density::Compact;
    let rows = super::card_preview::rows(&cfg);
    let narrow: Vec<_> = rows
        .iter()
        .filter(|r| matches!(r.field, Field::CardPreview(i) if i >= 100))
        .collect();
    assert_eq!(narrow.len(), 1, "compact is one line");
    assert!(
        narrow[0].value.contains("071 $1.42"),
        "cost kept, id shortened: {:?}",
        narrow[0].value
    );
}

#[test]
fn mouse_toggles_a_field_at_80x24_too() {
    let (_fx, mut app) = settings_app();
    let s = render(&mut app, 80, 24);
    click(&mut app, find(&s, "Board cards", "Board cards"));
    let s = render(&mut app, 80, 24);
    assert!(s.contains("At 160x45") && !s.contains("48.2k tok"), "{s}");
    click(&mut app, find(&s, "tokens", " on "));
    let s = render(&mut app, 80, 24);
    assert!(s.contains("48.2k tok"), "{s}");
}

fn type_into(v: &mut SettingsView, ctx: &corral_ops::Ctx, text: &str) {
    v.on_key(k(KeyCode::Enter), ctx);
    for ch in text.chars() {
        v.on_key(c(ch), ctx);
    }
    v.on_key(k(KeyCode::Enter), ctx);
}

fn saved_card() -> impl Fn(&std::path::Path) -> corral_core::CardCfg {
    |root| Store::open(root).unwrap().config.board.card
}

#[test]
fn keys_keep_acting_on_the_same_setting_when_the_preview_height_changes() {
    for (w, h) in [(160, 45), (80, 24)] {
        let fx = fixture();
        let ctx = &fx.ctx;
        let mut v = SettingsView::new(ctx);
        render_at(&mut v, ctx, w, h);
        walk_to(&mut v, ctx, &Field::CardDensity);
        v.on_key(k(KeyCode::Right), ctx);
        assert_eq!(
            v.current().map(|r| r.field),
            Some(Field::CardDensity),
            "{w}x{h}"
        );
        v.on_key(k(KeyCode::Right), ctx);
        assert_eq!(
            v.draft.board.card.density,
            corral_core::Density::Compact,
            "{w}x{h}"
        );
        assert_eq!(
            v.current().map(|r| r.field),
            Some(Field::CardDensity),
            "{w}x{h}"
        );
        v.on_key(c('s'), ctx);
        assert_eq!(
            saved_card()(&fx.root).density,
            corral_core::Density::Compact,
            "{w}x{h}"
        );
    }
}

#[test]
fn disabling_and_re_enabling_the_last_field_keeps_it_selected() {
    for (w, h) in [(160, 45), (80, 24)] {
        let fx = fixture();
        let ctx = &fx.ctx;
        let mut v = SettingsView::new(ctx);
        render_at(&mut v, ctx, w, h);
        walk_to(&mut v, ctx, &Field::CardField(CardField::Cost));
        v.on_key(k(KeyCode::Right), ctx);
        assert!(v.draft.board.card.fields.is_empty(), "{w}x{h}: cost off");
        assert_eq!(
            v.current().map(|r| r.field),
            Some(Field::CardField(CardField::Cost)),
            "{w}x{h}"
        );
        v.on_key(k(KeyCode::Right), ctx);
        assert_eq!(
            v.draft.board.card.fields,
            ["cost"],
            "{w}x{h}: cost back on, nothing else enabled"
        );
        v.on_key(c('s'), ctx);
        assert_eq!(saved_card()(&fx.root).fields, ["cost"], "{w}x{h}");
    }
}

#[test]
fn mouse_then_keyboard_keeps_acting_on_the_clicked_setting() {
    for (w, h) in [(160, 45), (80, 24)] {
        let (fx, mut app) = settings_app();
        let s = render(&mut app, w, h);
        click(&mut app, find(&s, "Board cards", "Board cards"));
        let s = render(&mut app, w, h);
        click(&mut app, find(&s, "Density", "rich"));
        app.handle_key(k(KeyCode::Right));
        app.handle_key(c('s'));
        assert_eq!(
            saved_card()(&fx.root).density,
            corral_core::Density::Compact,
            "{w}x{h}: click picked rich, then Right moved on to compact"
        );
    }
}

#[test]
fn mouse_disabling_the_last_field_then_keyboard_restores_only_that_field() {
    for (w, h) in [(160, 45), (80, 24)] {
        let (fx, mut app) = settings_app();
        let s = render(&mut app, w, h);
        click(&mut app, find(&s, "Board cards", "Board cards"));
        let s = render(&mut app, w, h);
        click(&mut app, find(&s, "cost  ", " off"));
        app.handle_key(k(KeyCode::Right));
        app.handle_key(c('s'));
        assert_eq!(saved_card()(&fx.root).fields, ["cost"], "{w}x{h}");
    }
}

#[test]
fn saving_merges_column_overrides_key_by_key_onto_the_latest_config() {
    let mut fx = fixture();
    fx.ctx
        .store
        .edit_config(|doc| {
            doc.set_str("board.card.columns.review", "[\"age\"]")?;
            doc.set_str("board.card.columns.done", "[\"tokens\"]")
        })
        .unwrap();
    fx.ctx.store.config = Store::open(&fx.root).unwrap().config;
    let ctx = &fx.ctx;
    let mut v = SettingsView::new(ctx);

    walk_to(
        &mut v,
        ctx,
        &Field::CardColumn(corral_core::Status::Running),
    );
    type_into(&mut v, ctx, "tokens");
    walk_to(&mut v, ctx, &Field::CardColumn(corral_core::Status::Review));
    v.on_key(k(KeyCode::Enter), ctx);
    for _ in 0..3 {
        v.on_key(k(KeyCode::Backspace), ctx);
    }
    v.on_key(k(KeyCode::Enter), ctx);
    assert!(
        !v.draft.board.card.columns.contains_key("review"),
        "staged removal"
    );

    ctx.store
        .edit_config(|doc| {
            doc.set_str("board.card.columns.done", "[\"priority\"]")?;
            doc.set_str("board.card.columns.ready", "[\"age\"]")
        })
        .unwrap();
    assert!(
        matches!(v.on_key(c('s'), ctx), crate::Action::Toast(t) if t.tone == crate::Tone::Good)
    );

    let columns = saved_card()(&fx.root).columns;
    let want = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        columns.get("running"),
        Some(&want(&["tokens"])),
        "staged addition: {columns:?}"
    );
    assert_eq!(columns.get("review"), None, "staged removal: {columns:?}");
    assert_eq!(
        columns.get("done"),
        Some(&want(&["priority"])),
        "untouched key keeps the newer disk value: {columns:?}"
    );
    assert_eq!(
        columns.get("ready"),
        Some(&want(&["age"])),
        "key added on disk survives: {columns:?}"
    );
}
