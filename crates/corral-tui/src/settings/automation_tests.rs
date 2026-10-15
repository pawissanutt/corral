use corral_core::{Accept, MergeMode, Store};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use unicode_width::UnicodeWidthStr;

use super::automation::{Key, List, Scope};
use super::model::Field;
use super::tests::{HEAD_COMMENT, SLOT_COMMENT, c, fixture, k, render_at};
use super::{SECTIONS, SettingsView};
use crate::app::App;
use crate::board::tests::buffer_text;
use crate::{Mode, View};

fn project(p: &str, key: Key) -> Field {
    Field::Review(Scope::Project(p.into()), key)
}

fn global(key: Key) -> Field {
    Field::Review(Scope::Global, key)
}

/// Opens the Automation section and selects `field` with ↓ only, as a keyboard user would.
fn walk_to(v: &mut SettingsView, ctx: &corral_ops::Ctx, field: &Field) {
    v.on_key(c('6'), ctx);
    for _ in 0..v.rows().len() {
        if v.current().is_some_and(|r| r.field == *field) {
            return;
        }
        v.on_key(k(KeyCode::Down), ctx);
    }
    panic!("{field:?} not reachable with ↓");
}

fn type_in(v: &mut SettingsView, ctx: &corral_ops::Ctx, text: &str) {
    v.on_key(k(KeyCode::Enter), ctx);
    for ch in text.chars() {
        v.on_key(c(ch), ctx);
    }
    v.on_key(k(KeyCode::Enter), ctx);
}

#[test]
fn automation_section_renders_policy_rows_with_inherited_values() {
    let fx = fixture();
    let mut v = SettingsView::new(&fx.ctx);
    assert_eq!(SECTIONS.len(), 6);
    v.on_key(c('6'), &fx.ctx);
    let s = render_at(&mut v, &fx.ctx, 160, 80);
    for part in [
        "Automation",
        "Autopilot",
        "[on]",
        "Review policy, all projects",
        "+ add gate command",
        "Shell commands run in the run worktree",
        "+ add owner path",
        "app review",
        "web review",
        "Gate commands",
        "Owner paths",
    ] {
        assert!(s.contains(part), "missing {part:?}\n{s}");
    }
    let accept = s
        .lines()
        .find(|l| l.contains("Accept mode"))
        .unwrap_or_else(|| panic!("no accept row\n{s}"));
    assert!(
        accept.contains("[inherit]")
            && accept.contains(" owner ")
            && accept.contains(" auto ")
            && accept.contains("→ owner (built-in default)"),
        "{accept}"
    );
    let reviewer = s.lines().find(|l| l.contains("Reviewer")).unwrap();
    assert!(
        reviewer.contains(" none ") && reviewer.contains(" pm "),
        "{reviewer}"
    );
    assert!(
        s.contains("Diff limit") && s.contains("1500 lines (built-in default)"),
        "{s}"
    );
}

#[test]
fn keys_change_a_projects_accept_mode_and_save_it() {
    let fx = fixture();
    let ctx = &fx.ctx;
    let mut v = SettingsView::new(ctx);
    walk_to(&mut v, ctx, &project("app", Key::Accept));
    v.on_key(c('l'), ctx);
    v.on_key(c('l'), ctx);
    assert_eq!(
        v.draft.projects["app"].review.accept,
        Some(Accept::Auto),
        "→ → picks auto"
    );
    assert!(v.dirty());
    v.on_key(c('s'), ctx);
    assert!(!v.dirty());
    let toml = fx.toml();
    assert!(
        toml.contains("[projects.app.review]") && toml.contains("accept = \"auto\""),
        "{toml}"
    );
    assert!(
        toml.contains(HEAD_COMMENT) && toml.contains(SLOT_COMMENT),
        "{toml}"
    );
    let saved = Store::open(&fx.root).unwrap().config;
    assert_eq!(saved.review_policy(Some("app"), None).accept, Accept::Auto);
    assert_eq!(saved.review_policy(Some("web"), None).accept, Accept::Owner);
}

#[test]
fn gate_list_editor_adds_reorders_removes_and_projects_can_own_a_list() {
    let fx = fixture();
    let ctx = &fx.ctx;
    let mut v = SettingsView::new(ctx);
    walk_to(&mut v, ctx, &global(Key::Add(List::Gate)));
    type_in(&mut v, ctx, "cargo test");
    walk_to(&mut v, ctx, &global(Key::Add(List::Gate)));
    type_in(&mut v, ctx, "cargo clippy");
    assert_eq!(
        v.draft.review.gate.as_deref(),
        Some(&["cargo test".to_string(), "cargo clippy".to_string()][..])
    );
    walk_to(&mut v, ctx, &global(Key::Item(List::Gate, 1)));
    v.on_key(c('K'), ctx);
    assert_eq!(
        v.draft.review.gate.as_deref(),
        Some(&["cargo clippy".to_string(), "cargo test".to_string()][..]),
        "K moves the command up"
    );
    assert_eq!(
        v.current().map(|r| r.field),
        Some(global(Key::Item(List::Gate, 0))),
        "selection follows the moved command"
    );

    walk_to(&mut v, ctx, &project("web", Key::Mode(List::Gate)));
    v.on_key(c('l'), ctx);
    assert_eq!(
        v.draft.projects["web"].review.gate.as_deref(),
        Some(&["cargo clippy".to_string(), "cargo test".to_string()][..]),
        "an own list starts from the inherited one"
    );
    walk_to(&mut v, ctx, &project("web", Key::Item(List::Gate, 1)));
    v.on_key(c('x'), ctx);
    assert_eq!(
        v.draft.projects["web"].review.gate.as_deref(),
        Some(&["cargo clippy".to_string()][..])
    );

    walk_to(&mut v, ctx, &project("web", Key::Merge));
    v.on_key(c('h'), ctx);
    assert_eq!(v.draft.projects["web"].review.merge, Some(MergeMode::Ff));
    walk_to(&mut v, ctx, &global(Key::MaxRounds));
    type_in(&mut v, ctx, "3");
    assert_eq!(v.draft.review.max_rounds, Some(3));
    walk_to(&mut v, ctx, &Field::Autopilot);
    v.on_key(k(KeyCode::Enter), ctx);
    assert_eq!(v.draft.review.autopilot, Some(false));

    v.on_key(c('s'), ctx);
    let saved = Store::open(&fx.root).unwrap().config;
    assert_eq!(
        saved.review.gate,
        Some(vec!["cargo clippy".into(), "cargo test".into()])
    );
    assert_eq!(saved.review.max_rounds, Some(3));
    assert!(!saved.autopilot_enabled());
    let web = saved.review_policy(Some("web"), None);
    assert_eq!(web.gate, vec!["cargo clippy"]);
    assert_eq!(web.merge, MergeMode::Ff);
    assert_eq!(saved.review_policy(Some("app"), None).gate.len(), 2);
}

#[test]
fn invalid_owner_path_is_marked_on_its_row_and_blocks_saving() {
    let fx = fixture();
    let ctx = &fx.ctx;
    let mut v = SettingsView::new(ctx);
    walk_to(&mut v, ctx, &global(Key::Add(List::OwnerPaths)));
    type_in(&mut v, ctx, "/etc/**");
    let s = render_at(&mut v, ctx, 160, 80);
    let row = s
        .lines()
        .position(|l| l.contains("Owner path 1"))
        .unwrap_or_else(|| panic!("no owner path row\n{s}"));
    assert!(
        s.lines().nth(row + 1).unwrap().contains("must be relative"),
        "{s}"
    );
    v.on_key(c('s'), ctx);
    assert!(
        !fx.toml().contains("owner_paths"),
        "invalid draft not written"
    );
}

fn screen(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(160, 80)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    buffer_text(term.backend().buffer())
}

fn click(app: &mut App, (x, y): (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

/// The cell where `needle` starts on the first row that also contains `on_row` at or after row `from`.
fn find(text: &str, from: usize, on_row: &str, needle: &str) -> (u16, u16) {
    let (y, line) = text
        .lines()
        .enumerate()
        .skip(from)
        .find(|(_, l)| l.contains(on_row) && l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} on a {on_row:?} row not rendered\n{text}"));
    let byte = line.find(needle).unwrap();
    (line[..byte].width() as u16, y as u16)
}

#[test]
fn mouse_changes_a_projects_accept_mode_and_saves_it() {
    let fx = fixture();
    let mut app = App::new(fx.ctx.clone(), Mode::Settings).unwrap();
    let s = screen(&mut app);
    click(&mut app, find(&s, 0, "Automation", "Automation"));
    let s = screen(&mut app);
    let app_heading = s
        .lines()
        .position(|l| l.contains("app review"))
        .unwrap_or_else(|| panic!("no app section\n{s}"));
    click(&mut app, find(&s, app_heading, "Accept mode", " auto "));
    let s = screen(&mut app);
    let line = s
        .lines()
        .skip(app_heading)
        .find(|l| l.contains("Accept mode"))
        .unwrap();
    assert!(
        line.contains("[auto]"),
        "the clicked pill is chosen: {line}"
    );
    let footer = s.lines().count() - 1;
    let save = s.lines().nth(footer).unwrap();
    let byte = save.find("s save").unwrap_or_else(|| panic!("{save}"));
    click(&mut app, (save[..byte].width() as u16, footer as u16));
    let toml = fx.toml();
    assert!(
        toml.contains("[projects.app.review]") && toml.contains("accept = \"auto\""),
        "{toml}"
    );
}
