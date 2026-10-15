use std::collections::BTreeMap;
use std::path::Path;

use corral_core::{ConfigSource, Store, detect_gate_presets};
use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, MouseEventKind};

use super::SettingsView;
use super::automation::{Key, List, Scope};
use super::model::Field;
use super::mouse_tests::{click, ev, find, footer, render};
use super::tests::{Fixture, HEAD_COMMENT, c, fixture, k, render_at};
use crate::app::App;
use crate::{Mode, View};

const STEPS: usize = 80;
/// A cell inside the settings form at every size these tests draw.
const FORM: (u16, u16) = (40, 10);

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `app` owns scripts/gate.sh (long) and a justfile with `check`; `web` has a justfile with `gate`. No gate is
/// configured anywhere.
fn repo_fixture() -> Fixture {
    let fx = fixture();
    let mut script = String::from("#!/usr/bin/env bash\necho GATE-TOP\n");
    for i in 0..STEPS {
        script.push_str(&format!("\techo step {i}\n"));
    }
    script.push_str("echo GATE-BOTTOM\n");
    write(&fx.root, "app/scripts/gate.sh", &script);
    write(
        &fx.root,
        "app/justfile",
        "# JUSTFILE-APP\ncheck:\n    cargo test\n",
    );
    write(
        &fx.root,
        "web/justfile",
        "# JUSTFILE-WEB\ngate:\n    echo web\n",
    );
    fx
}

fn field(project: &str, key: Key) -> Field {
    Field::Review(Scope::Project(project.into()), key)
}

fn walk_to(v: &mut SettingsView, ctx: &Ctx, target: &Field) {
    v.on_key(c('6'), ctx);
    for _ in 0..v.rows().len() {
        if v.current().is_some_and(|r| r.field == *target) {
            return;
        }
        v.on_key(k(KeyCode::Down), ctx);
    }
    panic!("{target:?} not reachable with ↓");
}

#[test]
fn keyboard_stages_a_detected_gate_lets_the_owner_edit_it_and_saves() {
    let fx = repo_fixture();
    let ctx = &fx.ctx;
    let presets = detect_gate_presets(&fx.root.join("app")).unwrap();
    let commands: Vec<Vec<String>> = presets.iter().map(|p| p.commands.clone()).collect();
    assert_eq!(
        commands,
        vec![
            vec!["bash scripts/gate.sh".to_string()],
            vec!["just check".into()]
        ]
    );
    let before = fx.toml();
    let mut v = SettingsView::new(ctx);
    walk_to(&mut v, ctx, &field("app", Key::Preset(0)));
    let s = render_at(&mut v, ctx, 160, 45);
    let row = s
        .lines()
        .find(|l| l.contains("▸ Repository gate"))
        .unwrap_or_else(|| panic!("no selected preset row\n{s}"));
    assert!(
        row.contains("bash scripts/gate.sh") && row.contains("⏎ use this gate"),
        "{row}"
    );
    assert!(s.contains("Just check") && s.contains("just check"), "{s}");
    assert!(v.hints().contains(&("⏎", "use this gate")));

    v.on_key(k(KeyCode::Enter), ctx);
    assert_eq!(
        v.draft.projects["app"].review.gate,
        Some(vec!["bash scripts/gate.sh".into()])
    );
    assert_eq!(
        v.current().map(|r| r.field),
        Some(field("app", Key::Item(List::Gate, 0))),
        "the staged command is selected for editing"
    );
    assert_eq!(fx.toml(), before, "staging alone writes nothing");
    let s = render_at(&mut v, ctx, 160, 45);
    assert!(
        !s.contains("use this gate"),
        "a gated project offers no presets\n{s}"
    );

    v.on_key(k(KeyCode::Enter), ctx);
    for ch in " --fast".chars() {
        v.on_key(c(ch), ctx);
    }
    v.on_key(k(KeyCode::Enter), ctx);
    v.on_key(c('s'), ctx);
    assert!(!v.dirty());
    let toml = fx.toml();
    assert!(
        toml.contains("[projects.app.review]") && toml.contains(HEAD_COMMENT),
        "{toml}"
    );
    let saved = Store::open(&fx.root).unwrap().config;
    assert_eq!(
        saved.review_policy(Some("app"), None).gate,
        vec!["bash scripts/gate.sh --fast"]
    );
    assert!(saved.review_policy(Some("web"), None).gate.is_empty());
}

#[test]
fn preview_follows_the_selected_preset_and_pgdn_reads_a_long_script() {
    let fx = repo_fixture();
    let ctx = &fx.ctx;
    let mut v = SettingsView::new(ctx);
    walk_to(&mut v, ctx, &field("app", Key::Preset(0)));
    let s = render_at(&mut v, ctx, 160, 45);
    assert!(
        s.contains("┌ Repository gate preview · read-only · 83 lines")
            && s.contains("│ echo GATE-TOP")
            && s.contains("│     echo step 0")
            && !s.contains("GATE-BOTTOM"),
        "the selected preset's script, tabs expanded, starts below its rows\n{s}"
    );
    let mut s = s;
    for _ in 0..60 {
        if s.contains("│ echo GATE-BOTTOM") {
            break;
        }
        v.on_key(k(KeyCode::PageDown), ctx);
        s = render_at(&mut v, ctx, 160, 45);
    }
    assert!(
        s.contains("│ echo GATE-BOTTOM") && s.contains("└ end of 83 lines"),
        "{s}"
    );

    v.on_key(k(KeyCode::Down), ctx);
    assert_eq!(
        v.current().map(|r| r.field),
        Some(field("app", Key::Preset(1)))
    );
    let s = render_at(&mut v, ctx, 160, 45);
    assert!(
        s.contains("┌ Just check preview")
            && s.contains("│ # JUSTFILE-APP")
            && !s.contains("GATE-TOP"),
        "↓ to the next preset previews its justfile\n{s}"
    );

    walk_to(&mut v, ctx, &field("web", Key::Preset(0)));
    let s = render_at(&mut v, ctx, 160, 80);
    assert!(
        s.contains("┄ Repository gate preview · 83 lines · select a row of app to read it")
            && s.contains("│ # JUSTFILE-WEB")
            && !s.contains("│ echo GATE-TOP"),
        "only the selected project's preview is expanded\n{s}"
    );
}

#[test]
fn mouse_stages_a_preset_with_its_button_wheel_reads_the_script_and_saves() {
    let fx = repo_fixture();
    let mut app = App::new(fx.ctx.clone(), Mode::Settings).unwrap();
    let s = render(&mut app, 160, 45);
    click(&mut app, find(&s, "  Review", "Review"));
    let mut s = render(&mut app, 160, 45);
    for _ in 0..30 {
        if s.contains("Repository gate") {
            break;
        }
        ev(&mut app, MouseEventKind::ScrollDown, FORM);
        s = render(&mut app, 160, 45);
    }
    click(&mut app, find(&s, "Repository gate", "⏎ use this gate"));
    let mut s = render(&mut app, 160, 45);
    let gate = s
        .lines()
        .find(|l| l.contains("Gate 1"))
        .unwrap_or_else(|| panic!("staged command not shown\n{s}"));
    assert!(
        gate.contains("▸ Gate 1") && gate.contains("bash scripts/gate.sh"),
        "{gate}"
    );
    assert!(
        s.contains("┌ scripts/gate.sh · bash scripts/gate.sh · read-only")
            && s.contains("│ echo GATE-TOP"),
        "the staged gate's script is previewed\n{s}"
    );
    assert!(s.contains("● unsaved changes"), "{s}");
    for _ in 0..40 {
        if s.contains("│ echo GATE-BOTTOM") {
            break;
        }
        ev(&mut app, MouseEventKind::ScrollDown, FORM);
        s = render(&mut app, 160, 45);
    }
    assert!(
        s.contains("│ echo GATE-BOTTOM"),
        "the wheel reads the whole script\n{s}"
    );
    click(&mut app, footer(&s, "s save"));
    let saved = Store::open(&fx.root).unwrap().config;
    assert_eq!(
        saved.projects["app"].review.gate,
        Some(vec!["bash scripts/gate.sh".into()])
    );
    assert!(fx.toml().contains(HEAD_COMMENT));
}

/// Global gate `just gate` with env; `app` runs its own scripts/gate.sh and adds a size limit.
fn configured(fx: &Fixture) -> Ctx {
    fx.ctx
        .store
        .edit_config_from(ConfigSource::SettingsUi, |doc| {
            let mut cfg = doc.config()?;
            cfg.review.gate = Some(vec!["just gate".into()]);
            cfg.review.env = Some(BTreeMap::from([("RUST_LOG".into(), "warn".into())]));
            if let Some(app) = cfg.projects.get_mut("app") {
                app.review.gate = Some(vec!["scripts/gate.sh".into()]);
                app.review.size_limits = Some(BTreeMap::from([("ts".into(), 400)]));
            }
            doc.sync_from(&cfg)
        })
        .unwrap();
    crate::board::tests::fixture_ctx(Store::open(&fx.root).unwrap())
}

#[test]
fn configured_gates_show_the_repo_script_or_justfile_read_only_with_env_and_limits() {
    let fx = repo_fixture();
    let ctx = &configured(&fx);
    let mut v = SettingsView::new(ctx);
    walk_to(&mut v, ctx, &field("app", Key::Mode(List::Gate)));
    let s = render_at(&mut v, ctx, 160, 200);
    assert!(
        s.contains("┌ scripts/gate.sh · read-only · 83 lines") && s.contains("│ echo GATE-TOP"),
        "{s}"
    );
    assert!(
        s.contains("┄ justfile · just gate · 3 lines · select a row of web to read it"),
        "{s}"
    );
    assert!(
        !s.contains("use this gate"),
        "gated projects offer no presets\n{s}"
    );
    let line = |needle: &str, nth: usize| {
        s.lines()
            .filter(|l| l.contains(needle))
            .nth(nth)
            .unwrap_or_else(|| panic!("{needle:?} #{nth} not rendered\n{s}"))
            .to_string()
    };
    assert!(line("Gate env", 0).contains("RUST_LOG=warn"), "{s}");
    assert!(
        line("Size limits", 0).contains("rs ≤ 800 (built-in default)"),
        "{s}"
    );
    assert!(
        line("Size limits", 1).contains("rs ≤ 800 · ts ≤ 400"),
        "{s}"
    );
    assert!(
        line("Gate env", 1).contains("RUST_LOG=warn (from [review])"),
        "{s}"
    );

    walk_to(&mut v, ctx, &field("web", Key::Mode(List::Gate)));
    let s = render_at(&mut v, ctx, 160, 80);
    assert!(
        s.contains("┌ justfile · just gate · read-only · 3 lines")
            && s.contains("│ # JUSTFILE-WEB")
            && s.contains("┄ scripts/gate.sh · 83 lines"),
        "{s}"
    );
    let before = fx.toml();
    v.on_key(c('s'), ctx);
    assert_eq!(fx.toml(), before, "previews never write");
}

/// Opt-in render evidence: `CORRAL_T078_EVIDENCE=<dir> cargo test -p corral-tui gate_render_evidence` writes the
/// Automation screen at 120x40 and 80x24 for each step of the keyboard journey.
#[test]
fn compact_terminals_can_scroll_through_the_entire_read_only_script() {
    for (width, height) in [(120, 40), (80, 24)] {
        // Given a selected repository gate whose script exceeds the terminal height.
        let fx = repo_fixture();
        let mut view = SettingsView::new(&fx.ctx);
        walk_to(&mut view, &fx.ctx, &field("app", Key::Preset(0)));
        let before = fx.toml();
        let mut screen = render_at(&mut view, &fx.ctx, width, height);

        // When page scrolling reads the script without activating a config field.
        for _ in 0..STEPS {
            if screen.contains("echo GATE-BOTTOM") {
                break;
            }
            view.on_key(k(KeyCode::PageDown), &fx.ctx);
            screen = render_at(&mut view, &fx.ctx, width, height);
        }

        // Then its final line is readable and the preview did not modify config.
        assert!(
            screen.contains("echo GATE-BOTTOM"),
            "{width}x{height}\n{screen}"
        );
        assert_eq!(fx.toml(), before);
        if let Some(dir) = std::env::var_os("CORRAL_T078_EVIDENCE") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                std::path::Path::new(&dir).join(format!("6-script-bottom-{width}x{height}.txt")),
                screen,
            )
            .unwrap();
        }
    }
}

#[test]
fn gate_render_evidence() {
    let Some(dir) = std::env::var_os("CORRAL_T078_EVIDENCE") else {
        println!("CORRAL_T078_EVIDENCE unset; skipping render evidence");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fx = repo_fixture();
    let to_preset = {
        let mut v = SettingsView::new(&fx.ctx);
        walk_to(&mut v, &fx.ctx, &field("app", Key::Preset(0)));
        v.sel
    };
    let mut app = App::new(fx.ctx.clone(), Mode::Settings).unwrap();
    let shot = |app: &mut App, name: &str| {
        for (w, h) in [(120, 40), (80, 24)] {
            let text = render(app, w, h);
            std::fs::write(dir.join(format!("{name}-{w}x{h}.txt")), text).unwrap();
        }
    };
    app.handle_key(c('6'));
    for _ in 0..to_preset {
        app.handle_key(k(KeyCode::Down));
    }
    shot(&mut app, "1-preset-selected");
    app.handle_key(k(KeyCode::Down));
    shot(&mut app, "2-next-preset-preview");
    app.handle_key(k(KeyCode::Up));
    app.handle_key(k(KeyCode::Enter));
    shot(&mut app, "3-staged");
    app.handle_key(c('s'));
    shot(&mut app, "4-saved");
    let gated = repo_fixture();
    let ctx = configured(&gated);
    let to_gate = {
        let mut v = SettingsView::new(&ctx);
        walk_to(&mut v, &ctx, &field("app", Key::Mode(List::Gate)));
        v.sel
    };
    let mut app = App::new(ctx, Mode::Settings).unwrap();
    app.handle_key(c('6'));
    for _ in 0..to_gate {
        app.handle_key(k(KeyCode::Down));
    }
    shot(&mut app, "5-configured-script");
}
