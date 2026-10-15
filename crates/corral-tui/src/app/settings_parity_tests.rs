//! App-level Settings journeys. The two input-only scripts never mix input types.
//! Separate focused tests exercise the documented free-text exception for mouse users.

#[path = "config_tests.rs"]
mod config_tests;

use corral_core::{Config, Project, ReviewOverride, Store};
use corral_herdr::Herdr;
use corral_ops::{Ctx, HarnessInfo, HarnessReport, ModelSource};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use unicode_width::UnicodeWidthStr;

use super::App;
use crate::board::tests::buffer_text;
use crate::settings::SettingsView;
use crate::{Mode, Tone};

struct Fixture {
    root: tempfile::TempDir,
    ctx: Ctx,
    report: HarnessReport,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for name in ["app", "web", "lib"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        let mut store = Store::init(root.path()).unwrap();
        let mut cfg = Config::default_for(root.path());
        cfg.projects.clear();
        for name in ["app", "web"] {
            cfg.projects.insert(
                name.into(),
                Project {
                    artifact_retention: None,
                    post_land: Vec::new(),
                    path: name.into(),
                    base: None,
                    default_profile: None,
                    review: ReviewOverride::default(),
                },
            );
        }
        cfg.defaults.profile = Some("omp".into());
        cfg.herdr.session = Some("corral-test".into());
        cfg.artifacts.globs = vec!["*.md".into(), "*.png".into()];
        store.config = cfg;
        store.save_config().unwrap();
        let store = Store::open(root.path()).unwrap();
        let report = HarnessReport {
            detected_at: "2026-10-15T09:00:00Z".parse().unwrap(),
            harnesses: store
                .config
                .harnesses
                .keys()
                .map(|key| HarnessInfo {
                    key: key.clone(),
                    program: Some(key.clone()),
                    path: Some(root.path().join(key)),
                    version: Some("test-1".into()),
                    models: vec!["opus".into(), "sonnet".into(), "haiku".into()],
                    model_source: ModelSource::Listed,
                    efforts: Vec::new(),
                    takes_effort: true,
                    notes: Vec::new(),
                })
                .collect(),
        };
        // No environment mutation and no live herdr or harness subprocesses.
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(root.path().join("never-call-herdr"), None),
        };
        Self { root, ctx, report }
    }

    fn app(&self) -> App {
        let mut app = App::new(self.ctx.clone(), Mode::Settings).unwrap();
        app.stack[0] = Box::new(SettingsView::with_discovery_report(
            &self.ctx,
            self.report.clone(),
        ));
        draw(&mut app);
        app
    }

    fn config(&self) -> Config {
        Store::open(self.root.path()).unwrap().config
    }

    fn concurrent_edit(&self) {
        self.ctx
            .store
            .edit_config(|doc| {
                doc.set_str("sync.idle_after_secs", "99")?;
                doc.set_str("limits.running", "11")
            })
            .unwrap();
    }
}

fn draw_at(app: &mut App, width: u16, height: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    term.draw(|frame| app.draw(frame)).unwrap();
    buffer_text(term.backend().buffer())
}

fn draw(app: &mut App) -> String {
    draw_at(app, 160, 55)
}

/// The keyboard-only script's sole input path.
fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    draw(app);
}

fn keys(app: &mut App, text: &str) {
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
}

fn down(app: &mut App, count: usize) {
    for _ in 0..count {
        press(app, KeyCode::Down);
    }
}

/// Only text editing keys: also used by the focused mouse text-entry exceptions.
fn text(app: &mut App, value: &str) {
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    keys(app, value);
}

fn value(app: &mut App, value: &str) {
    press(app, KeyCode::Enter);
    text(app, value);
    press(app, KeyCode::Enter);
}

fn position(screen: &str, row: &str, label: &str) -> (u16, u16) {
    let (y, line) = screen
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(row) && line.contains(label))
        .unwrap_or_else(|| panic!("missing {label:?} on {row:?}\n{screen}"));
    let byte = line.find(label).unwrap();
    (
        u16::try_from(line[..byte].width()).unwrap(),
        u16::try_from(y).unwrap(),
    )
}

/// The mouse-only script's sole input path.
fn mouse(app: &mut App, kind: MouseEventKind, at: (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind,
        column: at.0,
        row: at.1,
        modifiers: KeyModifiers::NONE,
    });
}

fn click(app: &mut App, row: &str, label: &str) {
    let screen = draw(app);
    let at = position(&screen, row, label);
    mouse(app, MouseEventKind::Down(MouseButton::Left), at);
    draw(app);
    mouse(app, MouseEventKind::Up(MouseButton::Left), at);
    draw(app);
}

fn open_row(app: &mut App, row: &str) {
    click(app, row, row);
    if !app.stack.last().unwrap().captures_text() {
        click(app, row, row);
    }
}

fn saved(app: &App) {
    assert!(
        app.toasts.iter().any(|(toast, _)| toast.tone == Tone::Good),
        "save did not succeed"
    );
}

#[test]
fn input_parity_keyboard_only_settings_journey() {
    // Given a real file store and a known, synchronous discovery report.
    let fx = Fixture::new();
    let mut app = fx.app();
    let before = fx.config();

    // When General, Profiles, Harnesses, Projects and Artifacts are edited by keys alone.
    value(&mut app, "6");
    down(&mut app, 1);
    press(&mut app, KeyCode::Right); // omp -> inherit
    press(&mut app, KeyCode::Right); // inherit -> claude
    down(&mut app, 1);
    press(&mut app, KeyCode::Left); // omo -> generic
    down(&mut app, 1);
    value(&mut app, "settings-test");
    down(&mut app, 1);
    press(&mut app, KeyCode::Enter);
    assert_eq!(fx.config(), before, "edits must remain staged");

    keys(&mut app, "2");
    press(&mut app, KeyCode::Enter); // claude editor
    press(&mut app, KeyCode::Right); // generic drops unsupported overrides
    press(&mut app, KeyCode::Left); // claude
    down(&mut app, 1);
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "son");
    press(&mut app, KeyCode::Enter);
    down(&mut app, 1);
    for _ in 0..3 {
        press(&mut app, KeyCode::Right);
    }
    down(&mut app, 1);
    value(&mut app, "--verbose");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "y");
    press(&mut app, KeyCode::Enter); // accept the prefilled duplicate name
    keys(&mut app, "*");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "x"); // referenced default cannot be deleted
    assert_eq!(app.toasts.last().unwrap().0.tone, Tone::Warn);
    keys(&mut app, "1");
    down(&mut app, 1);
    press(&mut app, KeyCode::Left); // claude-copy -> claude
    keys(&mut app, "2");
    down(&mut app, 1);
    keys(&mut app, "x");
    keys(&mut app, "n");
    keys(&mut app, "fresh");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "r"); // injected re-detection, never a subprocess

    keys(&mut app, "3r");
    down(&mut app, 1);
    let command = format!("{} --verbose", before.harnesses["claude"].command);
    value(&mut app, &command);
    keys(&mut app, "4");
    down(&mut app, 1);
    value(&mut app, "app");
    down(&mut app, 1);
    value(&mut app, "main");
    down(&mut app, 1);
    press(&mut app, KeyCode::Right);
    down(&mut app, 1);
    value(&mut app, "2");
    down(&mut app, 1);
    keys(&mut app, "x"); // web header
    keys(&mut app, "a");
    keys(&mut app, "lib");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "5JKJx");
    keys(&mut app, "a");
    keys(&mut app, "*.log");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "5");
    down(&mut app, 3);
    value(&mut app, "4096");
    fx.concurrent_edit();
    keys(&mut app, "s");

    // Then persisted values cover every editable section and preserve concurrent unrelated edits.
    saved(&app);
    let cfg = fx.config();
    assert_eq!((cfg.limits.running, cfg.sync.idle_after_secs), (6, 99));
    assert_eq!(cfg.defaults.profile.as_deref(), Some("claude"));
    assert_eq!(cfg.pm.agent, "generic");
    assert_eq!(cfg.herdr.session.as_deref(), Some("settings-test"));
    assert!(!cfg.ui.mouse);
    assert!(!cfg.profiles.contains_key("claude-copy"));
    assert!(cfg.profiles.contains_key("fresh"));
    let profile = &cfg.profiles["claude"];
    assert_eq!(profile.model.as_deref(), Some("sonnet"));
    assert_eq!(profile.effort.as_deref(), Some("high"));
    assert_eq!(profile.args, ["--verbose"]);
    assert_eq!(cfg.harnesses["claude"].command, command);
    assert_eq!(cfg.projects["app"].base.as_deref(), Some("main"));
    assert_eq!(
        cfg.projects["app"].default_profile.as_deref(),
        Some("claude")
    );
    assert_eq!(cfg.limits.per_project["app"], 2);
    assert!(!cfg.projects.contains_key("web"));
    assert_eq!(cfg.projects["lib"].path, "lib");
    assert_eq!(cfg.artifacts.globs, ["*.png", "*.log"]);
    assert_eq!(cfg.artifacts.max_bytes, 4096);
    press(&mut app, KeyCode::Esc);
    assert!(app.quit);
}

#[test]
fn input_parity_mouse_only_settings_journey() {
    // Given staged values and a duplicate's prefilled name, no text entry is needed.
    let fx = Fixture::new();
    let mut app = fx.app();
    let before = fx.config();

    // When all non-text actions are driven only through MouseEvents.
    click(&mut app, "Default profile", " claude ");
    click(&mut app, "PM agent profile", " generic ");
    click(&mut app, "Mouse", " off ");
    assert_eq!(fx.config(), before);
    click(&mut app, "Profiles", "Profiles");
    click(&mut app, "claude ", "claude ");
    click(&mut app, "claude ", "claude ");
    click(&mut app, "Harness", " generic ");
    click(&mut app, "Harness", " claude ");
    click(&mut app, "Model", "▾ choose");
    let screen = draw(&mut app);
    mouse(
        &mut app,
        MouseEventKind::ScrollDown,
        position(&screen, "unset (", "unset ("),
    );
    click(&mut app, "⏎ choose", "⏎ choose"); // apply wheel selection
    click(&mut app, "Effort", " high ");
    click(&mut app, "esc profiles", "esc profiles");
    click(&mut app, "y duplicate", "y duplicate");
    click(&mut app, "⏎ apply", "⏎ apply");
    click(&mut app, "* set default", "* set default");
    click(&mut app, "esc profiles", "esc profiles");
    click(&mut app, "x delete", "x delete");
    assert_eq!(app.toasts.last().unwrap().0.tone, Tone::Warn);
    click(&mut app, "F8", "F8"); // dismiss the reference warning before selecting a row
    click(&mut app, "General", "General");
    click(&mut app, "Default profile", " claude ");
    click(&mut app, "Profiles", "Profiles");
    click(&mut app, "claude-copy", "claude-copy"); // select, do not open
    click(&mut app, "x delete", "x delete");
    click(&mut app, "r re-detect", "r re-detect");
    click(&mut app, "Harnesses", "Harnesses");
    click(&mut app, "r re-detect", "r re-detect");
    click(&mut app, "Projects", "Projects");
    click(&mut app, "web", "× remove");
    click(&mut app, "Default profile", " claude ");
    click(&mut app, "Artifacts", "Artifacts");
    click(&mut app, "Glob 1", " ↓ ");
    click(&mut app, "Glob 2", " ↑ ");
    click(&mut app, "Glob 1", " ↓ ");
    click(&mut app, "Glob 2", " × ");
    fx.concurrent_edit();
    click(&mut app, "s save", "s save");

    // Then the same real save path persists the selected values.
    saved(&app);
    let cfg = fx.config();
    assert_eq!(cfg.sync.idle_after_secs, 99);
    assert!(!cfg.ui.mouse);
    assert_eq!(cfg.defaults.profile.as_deref(), Some("claude"));
    assert_eq!(cfg.pm.agent, "generic");
    assert!(!cfg.profiles.contains_key("claude-copy"));
    assert_eq!(cfg.profiles["claude"].model.as_deref(), Some("opus"));
    assert_eq!(cfg.profiles["claude"].effort.as_deref(), Some("high"));
    assert!(!cfg.projects.contains_key("web"));
    assert_eq!(
        cfg.projects["app"].default_profile.as_deref(),
        Some("claude")
    );
    assert_eq!(cfg.artifacts.globs, ["*.png"]);
    click(&mut app, "‹ back", "‹ back");
    assert!(app.quit);
}

#[test]
fn input_parity_mouse_text_entry_exception_fields_and_adds() {
    // Given mouse navigation; only replacing free text uses keyboard events.
    let fx = Fixture::new();
    let mut app = fx.app();

    // When edits, adds, filtering and applying remain mouse actions.
    open_row(&mut app, "Global slot limit");
    text(&mut app, "invalid");
    click(&mut app, "⏎ apply", "⏎ apply");
    assert!(app.stack.last().unwrap().captures_text());
    text(&mut app, "8");
    click(&mut app, "⏎ apply", "⏎ apply");
    open_row(&mut app, "herdr session");
    text(&mut app, "mouse-test");
    click(&mut app, "⏎ apply", "⏎ apply");
    click(&mut app, "Profiles", "Profiles");
    click(&mut app, "n new", "n new");
    text(&mut app, "mouse-profile");
    click(&mut app, "⏎ apply", "⏎ apply");
    click(&mut app, "Model", "▾ choose");
    text(&mut app, "son");
    click(&mut app, "sonnet", "sonnet"); // filtered discovered model
    click(&mut app, "Model", "▾ choose");
    text(&mut app, "custom-model");
    click(&mut app, "use \"custom-model\"", "use \"custom-model\"");
    open_row(&mut app, "Extra args");
    text(&mut app, "--verbose");
    click(&mut app, "⏎ apply", "⏎ apply");
    click(&mut app, "Harnesses", "Harnesses");
    open_row(&mut app, "Command");
    text(
        &mut app,
        &format!("{} --verbose", fx.config().harnesses["claude"].command),
    );
    click(&mut app, "⏎ apply", "⏎ apply");
    click(&mut app, "Projects", "Projects");
    click(&mut app, "a add project", "a add project");
    text(&mut app, "lib");
    click(&mut app, "⏎ apply", "⏎ apply");
    open_row(&mut app, "Slot cap"); // app's cap
    text(&mut app, "2");
    click(&mut app, "⏎ apply", "⏎ apply");
    click(&mut app, "Artifacts", "Artifacts");
    click(&mut app, "a add glob", "a add glob");
    text(&mut app, "*.svg");
    click(&mut app, "⏎ apply", "⏎ apply");
    open_row(&mut app, "Glob 1");
    text(&mut app, "*.txt");
    click(&mut app, "⏎ apply", "⏎ apply");
    open_row(&mut app, "Max bytes");
    text(&mut app, "2048");
    click(&mut app, "⏎ apply", "⏎ apply");
    click(&mut app, "s save", "s save");

    // Then text exceptions also reach the persisted config through real app handling.
    saved(&app);
    let cfg = fx.config();
    assert_eq!(cfg.limits.running, 8);
    assert_eq!(cfg.herdr.session.as_deref(), Some("mouse-test"));
    assert_eq!(
        cfg.profiles["mouse-profile"].model.as_deref(),
        Some("custom-model")
    );
    assert_eq!(cfg.profiles["mouse-profile"].args, ["--verbose"]);
    assert!(cfg.harnesses["claude"].command.ends_with(" --verbose"));
    assert_eq!(cfg.projects["lib"].path, "lib");
    assert_eq!(cfg.limits.per_project["app"], 2);
    assert_eq!(cfg.artifacts.globs, ["*.txt", "*.png", "*.svg"]);
    assert_eq!(cfg.artifacts.max_bytes, 2048);
}

#[test]
fn input_parity_keyboard_only_settings_discard_and_keep_editing() {
    // Given a staged toggle.
    let fx = Fixture::new();
    let before = fx.config();
    let mut app = fx.app();
    down(&mut app, 4);
    press(&mut app, KeyCode::Enter);

    // When back is cancelled once, then confirmed as discard, using keys only.
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "n");
    assert!(!app.quit);
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "y");

    // Then the Settings process exits without writing the draft.
    assert!(app.quit);
    assert_eq!(fx.config(), before);
}

#[test]
fn input_parity_mouse_only_settings_discard_and_keep_editing() {
    // Given a staged toggle.
    let fx = Fixture::new();
    let before = fx.config();
    let mut app = fx.app();
    click(&mut app, "Mouse", " off ");

    // When back is cancelled once, then confirmed as discard, using the mouse only.
    click(&mut app, "‹ back", "‹ back");
    click(&mut app, "n keep editing", "n keep editing");
    assert!(!app.quit);
    click(&mut app, "‹ back", "‹ back");
    click(&mut app, "y discard", "y discard");

    // Then no persisted setting changed.
    assert!(app.quit);
    assert_eq!(fx.config(), before);
}

#[test]
fn input_parity_keyboard_only_settings_save_and_close() {
    // Given a staged boolean change.
    let fx = Fixture::new();
    let mut app = fx.app();
    down(&mut app, 4);
    press(&mut app, KeyCode::Enter);

    // When the unsaved-changes prompt saves and closes.
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "s");

    // Then the saved config and the app exit are both observable.
    assert!(app.quit);
    assert!(!fx.config().ui.mouse);
}

#[test]
fn input_parity_mouse_only_settings_save_and_close() {
    // Given a staged boolean change.
    let fx = Fixture::new();
    let mut app = fx.app();
    click(&mut app, "Mouse", " off ");

    // When the prompt's save-and-close button is clicked.
    click(&mut app, "‹ back", "‹ back");
    click(&mut app, "s save and close", "s save and close");

    // Then the mouse path also persists and exits.
    assert!(app.quit);
    assert!(!fx.config().ui.mouse);
}

#[test]
fn input_parity_keyboard_only_settings_preview_scrolls_at_short_height() {
    // Given a profile editor taller than its viewport.
    let fx = Fixture::new();
    let mut app = fx.app();
    keys(&mut app, "2");
    press(&mut app, KeyCode::Enter);
    down(&mut app, 3);
    let before = draw_at(&mut app, 80, 12);

    // When PageDown scrolls below the final editable row (no mouse wheel).
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    let after = draw_at(&mut app, 80, 12);

    // Then the dispatch preview, unreachable by row selection alone, is visible.
    assert_ne!(after, before);
    assert!(after.contains("$ claude"), "{after}");
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert_ne!(draw_at(&mut app, 80, 12), after);
}

#[test]
fn input_parity_mouse_only_settings_wrapped_pills_and_list_buttons() {
    // Given a long profile name and glob that would push controls beyond column 80.
    let fx = Fixture::new();
    let mut ctx = fx.ctx.clone();
    let name = "zzzz-profile-with-a-long-name";
    let profile = ctx.store.config.profiles["claude"].clone();
    ctx.store.config.profiles.insert(name.into(), profile);
    ctx.store.config.artifacts.globs[0] = format!("{}*.md", "long-path/".repeat(8));
    ctx.store.save_config().unwrap();
    let mut app = App::new(ctx.clone(), Mode::Settings).unwrap();
    app.stack[0] = Box::new(SettingsView::with_discovery_report(&ctx, fx.report.clone()));

    // When the wrapped profile pill and list remove button are clicked at column 80.
    let screen = draw_at(&mut app, 80, 30);
    let at = position(&screen, name, name);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);
    let screen = draw_at(&mut app, 80, 30);
    let at = position(&screen, "Artifacts", "Artifacts");
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);
    let screen = draw_at(&mut app, 80, 30);
    let at = position(&screen, " × ", " × ");
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);
    let screen = draw_at(&mut app, 80, 30);
    let at = position(&screen, "s save", "s save");
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);

    // Then neither control is clipped into an unreachable region.
    saved(&app);
    let cfg = fx.config();
    assert_eq!(cfg.defaults.profile.as_deref(), Some(name));
    assert_eq!(cfg.artifacts.globs, ["*.png"]);
}

#[test]
fn input_parity_keyboard_only_settings_cancel_duplicate_then_new() {
    // Given a source with non-default overrides.
    let fx = Fixture::new();
    let mut app = fx.app();
    keys(&mut app, "2");
    press(&mut app, KeyCode::Enter);
    down(&mut app, 2);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Esc);

    // When duplicate is cancelled and the + new row is activated directly.
    keys(&mut app, "y");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "fresh");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "s");

    // Then a fresh profile does not inherit the cancelled duplicate's overrides.
    saved(&app);
    let cfg = fx.config();
    assert!(cfg.profiles["claude"].effort.is_some());
    assert_eq!(cfg.profiles["fresh"].effort, None);
}
