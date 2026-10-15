use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use corral_core::{Config, Project, ReviewOverride, Store};
use corral_ops::Ctx;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::SettingsView;
use crate::board::tests::buffer_text;
use crate::{Action, HitMap, Tone, View};

static SEQ: AtomicUsize = AtomicUsize::new(0);

pub(super) const HEAD_COMMENT: &str = "# owner notes: this comment must survive saves";
pub(super) const SLOT_COMMENT: &str = "# agents at once; raise when the box is idle";

pub(super) struct Fixture {
    pub ctx: Ctx,
    pub root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    pub fn toml(&self) -> String {
        std::fs::read_to_string(self.ctx.store.config_path()).unwrap()
    }
}

/// Workspace with projects `app` (inherits everything) and `web` (base main, profile claude, cap 1),
/// `[defaults] profile = "omp"` and a commented corral.toml.
pub(super) fn fixture() -> Fixture {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("corral-settings-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["app", "web"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    Store::init(&root).unwrap();
    let mut cfg = Config::default_for(&root);
    let project = |path: &str| Project {
        path: path.into(),
        default_profile: None,
        base: None,
        review: ReviewOverride::default(),
    };
    cfg.projects.insert("app".into(), project("app"));
    let mut web = project("web");
    web.base = Some("main".into());
    web.default_profile = Some("claude".into());
    cfg.projects.insert("web".into(), web);
    cfg.limits.per_project.insert("web".into(), 1);
    cfg.defaults.profile = Some("omp".into());
    cfg.herdr.session = Some("corral-test".into());
    let text = cfg.to_toml().replacen(
        "[limits]\n",
        &format!("{HEAD_COMMENT}\n[limits]\n{SLOT_COMMENT}\n"),
        1,
    );
    assert!(text.contains(SLOT_COMMENT), "{text}");
    let store = Store::init(&root).unwrap();
    std::fs::write(store.config_path(), text).unwrap();
    let ctx = Ctx::from_store(Store::open(&root).unwrap());
    Fixture { ctx, root }
}

pub(super) fn render_at(v: &mut SettingsView, ctx: &Ctx, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        let area = f.area();
        v.draw(f, area, ctx, &mut HitMap::default());
    })
    .unwrap();
    buffer_text(term.backend().buffer())
}

fn render(v: &mut SettingsView, ctx: &Ctx) -> String {
    render_at(v, ctx, 160, 45)
}

pub(super) fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn c(ch: char) -> KeyEvent {
    k(KeyCode::Char(ch))
}

fn press(v: &mut SettingsView, ctx: &Ctx, keys: &[KeyEvent]) -> Action {
    let mut last = Action::None;
    for key in keys {
        last = v.on_key(*key, ctx);
    }
    last
}

fn type_value(v: &mut SettingsView, ctx: &Ctx, text: &str) {
    v.on_key(k(KeyCode::Enter), ctx);
    v.on_key(
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        ctx,
    );
    for ch in text.chars() {
        v.on_key(c(ch), ctx);
    }
    v.on_key(k(KeyCode::Enter), ctx);
}

fn line_of(screen: &str, needle: &str) -> usize {
    screen
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not rendered\n{screen}"))
}

#[test]
fn general_section_shows_fields_help_and_effective_values() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    let s = render(&mut v, &fx.ctx);
    for want in [
        "‹ back",
        "settings · .corral/corral.toml",
        "▸ General",
        "Projects",
        "Artifacts",
        "▸ Global slot limit",
        "Runs that may hold a slot at once",
        "PM agent profile",
        "[omo]",
        "Mouse",
        "[on]",
        "corral-test",
    ] {
        assert!(s.contains(want), "{want:?} missing\n{s}");
    }
    let row = s.lines().nth(line_of(&s, "Default profile")).unwrap();
    assert!(row.contains("[omp]") && row.contains("inherit"), "{row}");
    assert!(!s.contains("unsaved"), "clean on open\n{s}");
    assert!(!s.contains('✗'), "valid config has no errors\n{s}");
}

#[test]
fn projects_section_shows_inherited_and_own_values() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    v.on_key(k(KeyCode::Tab), &fx.ctx);
    let s = render(&mut v, &fx.ctx);
    let app = line_of(&s, "▸ app");
    let web = line_of(&s, "web   × remove");
    assert!(app < web, "{s}");
    let block = |from: usize, to: usize| {
        s.lines()
            .skip(from)
            .take(to - from)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let app_rows = block(app, web);
    assert!(
        app_rows.contains("unset → the repo's current branch"),
        "{app_rows}"
    );
    assert!(
        app_rows.contains("[inherit]") && app_rows.contains("→ omp (from General)"),
        "{app_rows}"
    );
    assert!(
        app_rows.contains("unset → no cap below the global limit (3)"),
        "{app_rows}"
    );
    let web_rows = block(web, s.lines().count());
    assert!(
        web_rows.contains("main") && web_rows.contains("[claude]"),
        "{web_rows}"
    );
    assert!(s.contains("+ add project"), "{s}");
    assert!(
        v.hints().contains(&("x", "remove project")),
        "{:?}",
        v.hints()
    );
}

#[test]
fn artifacts_section_lists_globs_and_max_bytes() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    v.on_key(c('3'), &fx.ctx);
    let s = render(&mut v, &fx.ctx);
    for want in [
        "▸ Artifacts",
        "▸ Glob 1",
        ".omo/evidence/**",
        "↑  ↓  ×",
        "+ add glob",
        "Max bytes",
        "20000000",
    ] {
        assert!(s.contains(want), "{want:?} missing\n{s}");
    }
    assert!(v.hints().contains(&("K/J", "reorder")));
}

#[test]
fn dirty_state_confirm_and_save_keep_comments() {
    let fx = fixture();
    let before = fx.toml();
    let mut v = SettingsView::new(&fx.ctx);
    type_value(&mut v, &fx.ctx, "5");
    let s = render(&mut v, &fx.ctx);
    assert!(s.contains("● unsaved changes"), "{s}");
    assert!(
        s.lines()
            .nth(line_of(&s, "Global slot limit"))
            .unwrap()
            .contains('5')
    );
    assert!(
        v.hints().contains(&("s", "save ● unsaved")),
        "{:?}",
        v.hints()
    );
    assert_eq!(fx.toml(), before, "edits are staged, not written");

    assert_eq!(v.on_key(k(KeyCode::Esc), &fx.ctx), Action::None);
    let s = render(&mut v, &fx.ctx);
    assert!(
        s.contains("Your settings changes are not saved yet."),
        "{s}"
    );
    assert_eq!(v.on_key(c('n'), &fx.ctx), Action::None);
    assert!(!render(&mut v, &fx.ctx).contains("not saved yet"));

    match v.on_key(c('s'), &fx.ctx) {
        Action::Toast(t) => assert_eq!((t.tone, t.title.as_str()), (Tone::Good, "Settings saved")),
        other => panic!("{other:?}"),
    }
    let after = fx.toml();
    assert!(after.contains("running = 5"), "{after}");
    assert!(
        after.contains(HEAD_COMMENT) && after.contains(SLOT_COMMENT),
        "{after}"
    );
    assert_eq!(
        after.replace("running = 5", "running = 3"),
        before,
        "only the slot limit changed"
    );
    assert!(!v.dirty());
    assert_eq!(
        v.on_key(k(KeyCode::Esc), &fx.ctx),
        Action::Back,
        "clean esc goes back"
    );
}

#[test]
fn discard_drops_staged_edits() {
    let fx = fixture();
    let before = fx.toml();
    let mut v = SettingsView::new(&fx.ctx);
    press(&mut v, &fx.ctx, &[c('j'), c('l')]);
    assert!(v.dirty());
    assert_eq!(
        press(&mut v, &fx.ctx, &[k(KeyCode::Esc), c('y')]),
        Action::Back
    );
    assert_eq!(fx.toml(), before);
}

#[test]
fn validation_errors_show_inline_and_block_saving() {
    let fx = fixture();
    let before = fx.toml();
    let mut v = SettingsView::new(&fx.ctx);
    type_value(&mut v, &fx.ctx, "0");
    let s = render(&mut v, &fx.ctx);
    let field = line_of(&s, "Global slot limit");
    let err = line_of(&s, "✗ limits.running must be > 0");
    assert_eq!(
        err,
        field + 2,
        "error sits under the field's help line\n{s}"
    );
    match v.on_key(c('s'), &fx.ctx) {
        Action::Toast(t) => assert_eq!((t.tone, t.title.as_str()), (Tone::Warn, "Not saved")),
        other => panic!("{other:?}"),
    }
    assert_eq!(fx.toml(), before, "invalid drafts are never written");

    type_value(&mut v, &fx.ctx, "lots");
    let s = render(&mut v, &fx.ctx);
    assert!(s.contains("✗ \"lots\" is not a whole number"), "{s}");
    assert!(v.captures_text(), "a bad number keeps the editor open");
    v.on_key(k(KeyCode::Esc), &fx.ctx);
    type_value(&mut v, &fx.ctx, "4");
    assert!(!render(&mut v, &fx.ctx).contains('✗'));
}

#[test]
fn projects_add_remove_and_cap_save() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    v.on_key(c('2'), &fx.ctx);
    press(
        &mut v,
        &fx.ctx,
        &[c('a'), c('l'), c('i'), c('b'), k(KeyCode::Enter)],
    );
    let s = render(&mut v, &fx.ctx);
    assert!(
        s.contains("✗ projects.lib.path:") && s.contains("is not a directory"),
        "{s}"
    );
    assert_eq!(
        v.rows()[v.sel].label,
        "Path",
        "the new project's path is selected"
    );
    v.on_key(c('x'), &fx.ctx);
    assert!(!render(&mut v, &fx.ctx).contains("lib"));

    let cap = v
        .rows()
        .iter()
        .position(|r| r.field == super::model::Field::ProjectCap("app".into()))
        .unwrap();
    v.select(cap);
    type_value(&mut v, &fx.ctx, "2");
    let web = v.rows().iter().position(|r| r.label == "web").unwrap();
    v.select(web);
    v.on_key(c('x'), &fx.ctx);
    assert!(matches!(v.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));
    let cfg = Store::open(&fx.root).unwrap().config;
    assert_eq!(cfg.projects.keys().collect::<Vec<_>>(), ["app"]);
    assert_eq!(cfg.limits.per_project.get("app"), Some(&2));
    assert_eq!(
        cfg.limits.per_project.get("web"),
        None,
        "removing a project drops its cap"
    );
    assert!(fx.toml().contains(HEAD_COMMENT));
}

#[test]
fn glob_list_add_remove_reorder_and_pick() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    let globs = fx.ctx.store.config.artifacts.globs.clone();
    v.on_key(c('3'), &fx.ctx);
    press(&mut v, &fx.ctx, &[c('J'), c('x')]);
    press(
        &mut v,
        &fx.ctx,
        &[
            c('a'),
            c('*'),
            c('.'),
            c('l'),
            c('o'),
            c('g'),
            k(KeyCode::Enter),
        ],
    );
    v.on_key(c('1'), &fx.ctx);
    press(&mut v, &fx.ctx, &[c('j'), c('j'), c('h')]);
    assert!(matches!(v.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));
    let cfg = Store::open(&fx.root).unwrap().config;
    let mut want = globs.clone();
    want.swap(0, 1);
    want.remove(1);
    want.push("*.log".into());
    assert_eq!(cfg.artifacts.globs, want);
    assert_eq!(
        cfg.pm.agent, "generic",
        "pills are sorted claude, generic, omo, omp; h from omo picks the previous one"
    );
}

#[test]
fn save_keeps_keys_changed_on_disk_since_open() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("sync.idle_after_secs", "99"))
        .unwrap();
    type_value(&mut v, &fx.ctx, "6");
    assert!(matches!(v.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));
    let cfg = Store::open(&fx.root).unwrap().config;
    assert_eq!((cfg.limits.running, cfg.sync.idle_after_secs), (6, 99));
}
