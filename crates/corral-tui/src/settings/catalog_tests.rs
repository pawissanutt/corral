use corral_core::Store;
use ratatui::crossterm::event::{KeyCode, MouseEventKind};

use super::SettingsView;
use super::mouse_tests::{click, ev, find, footer, screen, settings_app};
use super::tests::{HEAD_COMMENT, c, fixture, k, line_of, press, render, type_text};
use crate::{Action, Tone, View};

fn row<'a>(s: &'a str, needle: &str) -> &'a str {
    s.lines().nth(line_of(s, needle)).unwrap()
}

fn new_profile(v: &mut SettingsView, fx: &super::tests::Fixture, name: &str) {
    press(v, &fx.ctx, &[c('2'), c('n')]);
    type_text(v, &fx.ctx, name);
    v.on_key(k(KeyCode::Enter), &fx.ctx);
}

#[test]
fn profiles_list_shows_columns_and_default() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    v.on_key(c('2'), &fx.ctx);
    let s = render(&mut v, &fx.ctx);
    assert!(s.contains("▸ Profiles"), "{s}");
    let head = row(&s, "profile  harness");
    assert!(head.contains("model") && head.contains("effort"), "{head}");
    let omp = row(&s, "omp      omp");
    assert!(
        omp.contains("★ default"),
        "the [defaults] profile is marked\n{s}"
    );
    assert!(!row(&s, "claude   claude").contains('★'), "{s}");
    assert!(s.contains("+ new profile"), "{s}");
    let hints = v.hints();
    for want in [
        ("n", "new"),
        ("y", "duplicate"),
        ("x", "delete"),
        ("*", "set default"),
    ] {
        assert!(hints.contains(&want), "{want:?} missing: {hints:?}");
    }
}

#[test]
fn editor_shows_harness_pills_efforts_and_preview() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    new_profile(&mut v, &fx, "claude-opus-high");
    let s = render(&mut v, &fx.ctx);
    assert!(s.contains("Profiles › claude-opus-high"), "{s}");
    let harness = row(&s, "▸ Harness");
    assert!(
        harness.contains("[claude]") && harness.contains(" omo ") && harness.contains(" omp "),
        "installed claude is the first pill and the new profile's harness\n{s}"
    );
    assert!(
        harness.find(" omo ").unwrap() < harness.find(" omp ").unwrap(),
        "missing omp sorts last\n{harness}"
    );
    let effort = row(&s, "Effort");
    for level in [
        "[inherit]",
        " low ",
        " medium ",
        " high ",
        " xhigh ",
        " max ",
    ] {
        assert!(effort.contains(level), "{level:?} missing: {effort}");
    }
    assert!(row(&s, "Model").contains("▾ choose"), "{s}");
    let preview = row(&s, "$ claude");
    assert!(preview.contains("$ claude 'Read "), "{preview}");
    assert!(
        preview.contains("T-042-R1.md"),
        "rendered with the sample task\n{preview}"
    );
}

#[test]
fn model_picker_filters_and_picks() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    new_profile(&mut v, &fx, "claude-opus-high");
    press(&mut v, &fx.ctx, &[c('j'), k(KeyCode::Enter)]);
    let s = render(&mut v, &fx.ctx);
    assert!(s.contains("model for claude-opus-high"), "{s}");
    for m in [
        "unset (the harness's default model)",
        "opus",
        "sonnet",
        "haiku",
    ] {
        assert!(s.contains(m), "{m:?} missing\n{s}");
    }
    assert!(s.contains("3 models (built-in aliases)"), "{s}");
    type_text(&mut v, &fx.ctx, "op");
    let s = render(&mut v, &fx.ctx);
    assert!(s.contains("▸ opus"), "the prefix match is first\n{s}");
    assert!(s.contains("use \"op\" (not in the list)"), "{s}");
    assert!(!s.contains("sonnet") && !s.contains("haiku"), "{s}");
    assert!(v.captures_text());
    v.on_key(k(KeyCode::Enter), &fx.ctx);
    press(&mut v, &fx.ctx, &[c('j'), c('l'), c('l'), c('l')]);
    let s = render(&mut v, &fx.ctx);
    assert!(row(&s, "Model").contains("opus"), "{s}");
    assert!(row(&s, "Effort").contains("[high]"), "{s}");
    assert!(
        s.contains("$ claude --model opus --effort high 'Read "),
        "the preview shows the flags a dispatch sends\n{s}"
    );
}

#[test]
fn free_text_model_and_save_as_default() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    new_profile(&mut v, &fx, "fast");
    press(
        &mut v,
        &fx.ctx,
        &[c('l'), c('l'), c('j'), k(KeyCode::Enter)],
    );
    let s = render(&mut v, &fx.ctx);
    assert!(
        s.contains("anthropic/claude-opus-4"),
        "omo lists its models\n{s}"
    );
    type_text(&mut v, &fx.ctx, "gpt");
    let s = render(&mut v, &fx.ctx);
    assert!(
        s.contains("▸ openai/gpt-5") && !s.contains("claude-opus-4"),
        "{s}"
    );
    press(&mut v, &fx.ctx, &[k(KeyCode::Down), k(KeyCode::Enter)]);
    v.on_key(c('*'), &fx.ctx);
    match v.on_key(c('s'), &fx.ctx) {
        Action::Toast(t) => assert_eq!(t.tone, Tone::Good, "{t:?}"),
        other => panic!("{other:?}"),
    }
    let cfg = Store::open(&fx.root).unwrap().config;
    let p = &cfg.profiles["fast"];
    assert_eq!(
        (p.harness.as_str(), p.model.as_deref(), p.effort.as_deref()),
        ("omo", Some("gpt"), None),
        "the typed text is a free-text model"
    );
    assert_eq!(cfg.defaults.profile.as_deref(), Some("fast"));
    assert!(fx.toml().contains(HEAD_COMMENT));
}

#[test]
fn switching_harness_drops_what_it_cannot_take() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    new_profile(&mut v, &fx, "deep");
    press(&mut v, &fx.ctx, &[c('j'), c('j'), c('h')]);
    v.on_key(c('k'), &fx.ctx);
    v.on_key(c('k'), &fx.ctx);
    v.on_key(c('l'), &fx.ctx);
    let s = render(&mut v, &fx.ctx);
    assert!(row(&s, "Harness").contains("[generic]"), "{s}");
    assert!(
        row(&s, "Model").contains("generic takes no model flag"),
        "{s}"
    );
    assert!(
        row(&s, "Effort").contains("generic takes no effort flag"),
        "{s}"
    );
    assert!(s.contains("$ cat '"), "{s}");
    assert!(!s.contains('✗'), "the profile stays valid\n{s}");
}

#[test]
fn delete_refuses_referenced_and_duplicate_copies() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    v.on_key(c('2'), &fx.ctx);
    let omp = v
        .rows()
        .iter()
        .position(|r| r.label.trim() == "omp")
        .unwrap();
    v.select(omp);
    match v.on_key(c('x'), &fx.ctx) {
        Action::Toast(t) => {
            assert_eq!(
                (t.tone, t.title.as_str()),
                (Tone::Warn, "Profile omp is in use")
            );
            assert!(t.body.contains("defaults.profile"), "{}", t.body);
        }
        other => panic!("{other:?}"),
    }
    let claude = v
        .rows()
        .iter()
        .position(|r| r.label.trim() == "claude")
        .unwrap();
    v.select(claude);
    match v.on_key(c('x'), &fx.ctx) {
        Action::Toast(t) => assert!(
            t.body.contains("projects.web.default_profile"),
            "{}",
            t.body
        ),
        other => panic!("{other:?}"),
    }
    let generic = v
        .rows()
        .iter()
        .position(|r| r.label.trim() == "generic")
        .unwrap();
    v.select(generic);
    assert_eq!(v.on_key(c('x'), &fx.ctx), Action::None);
    assert!(!v.draft.profiles.contains_key("generic"));

    v.select(claude);
    v.on_key(c('y'), &fx.ctx);
    assert!(
        render(&mut v, &fx.ctx).contains("› claude-copy▏"),
        "duplicate proposes a name"
    );
    v.on_key(k(KeyCode::Enter), &fx.ctx);
    assert_eq!(v.open.as_deref(), Some("claude-copy"));
    assert_eq!(v.draft.profiles["claude-copy"], v.draft.profiles["claude"]);
    v.on_key(k(KeyCode::Esc), &fx.ctx);
    assert!(v.open.is_none(), "esc in the editor returns to the list");
    assert!(render(&mut v, &fx.ctx).contains("▸ claude-copy"));
}

#[test]
fn harnesses_section_shows_install_state_and_edits_command() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    v.on_key(c('3'), &fx.ctx);
    let s = render(&mut v, &fx.ctx);
    for want in [
        "detected 2026-10-15 09:00 UTC · r re-detects",
        "claude   ✓ installed 1.0.0 · /usr/bin/claude",
        "omp   ✗ not installed (omp not found)",
        "--model <model> (3 built-in aliases) · --effort low|medium|high|xhigh|max",
        "generic",
        "no model flag · no effort flag",
    ] {
        assert!(s.contains(want), "{want:?} missing\n{s}");
    }
    assert!(v.hints().contains(&("r", "re-detect")), "{:?}", v.hints());
    v.on_key(c('j'), &fx.ctx);
    super::tests::type_value(
        &mut v,
        &fx.ctx,
        "claude --verbose {model_args} \"$(cat '{prompt_file}')\"",
    );
    assert!(matches!(v.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));
    let cfg = Store::open(&fx.root).unwrap().config;
    assert!(
        cfg.harnesses["claude"]
            .command
            .starts_with("claude --verbose ")
    );
}

#[test]
fn mouse_creates_profile_picks_model_and_saves_default() {
    let (fx, mut app) = settings_app();
    let s = screen(&mut app);
    click(&mut app, find(&s, "Profiles", "Profiles"));
    let s = screen(&mut app);
    click(&mut app, footer(&s, "n new"));
    for ch in "claude-opus-high".chars() {
        app.handle_key(c(ch));
    }
    let s = screen(&mut app);
    click(&mut app, footer(&s, "⏎ apply"));
    let s = screen(&mut app);
    assert!(s.contains("Profiles › claude-opus-high"), "{s}");
    click(&mut app, find(&s, "Harness", " omo "));
    let s = screen(&mut app);
    click(&mut app, find(&s, "Harness", " claude "));
    let s = screen(&mut app);
    click(&mut app, find(&s, "Model", "▾ choose"));
    let s = screen(&mut app);
    assert!(s.contains("model for claude-opus-high"), "{s}");
    let list = find(&s, "sonnet", "sonnet");
    ev(&mut app, MouseEventKind::ScrollDown, list);
    let s = screen(&mut app);
    assert!(
        s.contains("▸ opus"),
        "wheel over the list moves the selection\n{s}"
    );
    click(&mut app, find(&s, "▸ opus", "opus"));
    let s = screen(&mut app);
    assert!(!s.contains("model for"), "a click on a row chooses it\n{s}");
    click(&mut app, find(&s, "Effort", " high "));
    let s = screen(&mut app);
    assert!(s.contains("$ claude --model opus --effort high"), "{s}");
    click(&mut app, footer(&s, "* set default"));
    let s = screen(&mut app);
    click(&mut app, footer(&s, "s save"));
    let cfg = Store::open(&fx.root).unwrap().config;
    let p = &cfg.profiles["claude-opus-high"];
    assert_eq!(
        (p.harness.as_str(), p.model.as_deref(), p.effort.as_deref()),
        ("claude", Some("opus"), Some("high"))
    );
    assert_eq!(cfg.defaults.profile.as_deref(), Some("claude-opus-high"));
}

#[test]
fn mouse_opens_rows_closes_picker_and_returns_to_list() {
    let (_fx, mut app) = settings_app();
    app.handle_key(c('2'));
    let s = screen(&mut app);
    let omo = find(&s, "omo      omo", "omo");
    click(&mut app, omo);
    click(&mut app, omo);
    let s = screen(&mut app);
    assert!(
        s.contains("Profiles › omo"),
        "a click on the selected row opens it\n{s}"
    );
    click(&mut app, find(&s, "Model", "▾ choose"));
    let s = screen(&mut app);
    assert!(s.contains("model for omo"), "{s}");
    click(&mut app, (1, 1));
    let s = screen(&mut app);
    assert!(
        !s.contains("model for omo"),
        "an outside click closes an empty picker\n{s}"
    );
    click(&mut app, footer(&s, "esc profiles"));
    let s = screen(&mut app);
    assert!(
        s.contains("+ new profile") && !s.contains("Profiles ›"),
        "{s}"
    );
}
