use corral_core::{Profile, ProfileChoice, RunMode};
use corral_ops::{Ctx, HarnessInfo, HarnessReport, ModelSource};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::board::tests::{Fixture, buffer_text, code, key, seeded};
use crate::board::{BoardView, Overlay};
use crate::overlays::DispatchSheet;
use crate::{Action, Hit, HitMap, Mode, MouseCtx, View};

/// The seeded board plus a `fast` profile: claude harness, model sonnet, effort low.
pub(crate) fn fixture() -> Fixture {
    let mut fx = seeded();
    fx.ctx.store.config.profiles.insert(
        "fast".into(),
        Profile {
            harness: "claude".into(),
            model: Some("sonnet".into()),
            effort: Some("low".into()),
            args: Vec::new(),
        },
    );
    fx
}

pub(crate) fn report() -> HarnessReport {
    let info = |key: &str, models: &[&str]| HarnessInfo {
        key: key.into(),
        program: None,
        path: None,
        version: None,
        models: models.iter().map(|m| m.to_string()).collect(),
        model_source: ModelSource::Listed,
        efforts: Vec::new(),
        takes_effort: true,
        notes: Vec::new(),
    };
    HarnessReport {
        detected_at: jiff::Timestamp::now(),
        harnesses: vec![
            info("omo", &["anthropic/opus-4", "openai/gpt-5"]),
            info("claude", &["opus", "sonnet", "haiku"]),
        ],
    }
}

pub(crate) fn draw(view: &mut dyn View, ctx: &Ctx) -> (String, HitMap) {
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| view.draw(f, f.area(), ctx, &mut hits))
        .unwrap();
    (buffer_text(term.backend().buffer()), hits)
}

pub(crate) fn locate(text: &str, row: &str, needle: &str) -> (u16, u16) {
    let (y, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains(row) && l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {row:?} and {needle:?}\n{text}"));
    let byte = line.find(needle).unwrap_or(0);
    (line[..byte].width() as u16, y as u16)
}

pub(crate) fn overridden(text: &str, pill: &str) -> bool {
    let l = line(text, "profile ");
    l.contains(pill) && l.contains("(overridden)")
}

pub(crate) fn line<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?}\n{text}"))
}

/// A left click at (`x`, `y`) routed like `App::handle_mouse`: key hits press their key, outside-popup clicks
/// are Esc unless text is typed, everything else reaches the view's `on_mouse`.
pub(crate) fn click(
    view: &mut dyn View,
    ctx: &Ctx,
    hits: &HitMap,
    (x, y): (u16, u16),
    double: bool,
) -> Action {
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    match hits.at(x, y).cloned() {
        Some(Hit::Key(k)) => view.on_key(k, ctx),
        Some(Hit::Dismiss) if view.has_unsaved_text() => Action::None,
        Some(Hit::Dismiss) => view.on_key(code(KeyCode::Esc), ctx),
        Some(Hit::Inert) => Action::None,
        hit => view.on_mouse(ev, &MouseCtx { ctx, hit, double }),
    }
}

fn sheet(b: &mut BoardView) -> &mut DispatchSheet {
    match &mut b.overlay {
        Some(Overlay::Dispatch(s)) => s,
        _ => panic!("dispatch sheet expected"),
    }
}

/// Board with the dispatch sheet open on T-003 (Ready, profile omo).
fn open_sheet(ctx: &Ctx) -> BoardView {
    let mut b = BoardView::new(ctx).unwrap();
    b.on_key(key('l'), ctx);
    b.on_key(key('d'), ctx);
    sheet(&mut b);
    b
}

#[test]
fn dispatch_sheet_renders_profiles_and_a_per_run_override() {
    let fx = fixture();
    let ctx = &fx.ctx;
    let mut b = open_sheet(ctx);
    let (text, _) = draw(&mut b, ctx);
    let profile = line(&text, "profile ");
    for p in ["claude", " fast ", "generic", "[omo]", " omp "] {
        assert!(profile.contains(p), "{p} pill\n{text}");
    }
    assert!(!text.contains("(overridden)"), "{text}");
    assert!(line(&text, "model ").contains("harness default"), "{text}");
    let effort = line(&text, "effort ");
    assert!(
        effort.contains(" off ") && effort.contains(" max ") && !effort.contains('['),
        "omo levels, none chosen\n{text}"
    );

    for _ in 0..5 {
        b.on_key(key('e'), ctx);
    }
    let (text, _) = draw(&mut b, ctx);
    assert!(overridden(&text, "[omo]"), "{text}");
    assert!(
        line(&text, "effort ").contains("[high]") && line(&text, "effort ").contains("override"),
        "{text}"
    );

    sheet(&mut b).choice.report = Some(Ok(report()));
    b.on_key(key('M'), ctx);
    let (text, _) = draw(&mut b, ctx);
    assert!(
        text.contains("model · omo")
            && text.contains("profile default (harness default)")
            && text.contains("anthropic/opus-4"),
        "{text}"
    );
    assert_eq!(b.hints()[0], ("↑↓", "select"));
    for c in "gpt".chars() {
        b.on_key(key(c), ctx);
    }
    let (text, _) = draw(&mut b, ctx);
    assert!(
        text.contains("use \"gpt\"")
            && text.contains("openai/gpt-5")
            && !text.contains("anthropic/opus-4"),
        "filtered\n{text}"
    );
    b.on_key(code(KeyCode::Down), ctx);
    b.on_key(code(KeyCode::Enter), ctx);
    assert!(sheet(&mut b).choice.picker.is_none());
    assert_eq!(
        sheet(&mut b).choice.choice(),
        ProfileChoice {
            profile: Some("omo".into()),
            model: Some("openai/gpt-5".into()),
            effort: Some("high".into()),
        }
    );
    let (text, _) = draw(&mut b, ctx);
    let model = line(&text, "model ");
    assert!(
        model.contains("openai/gpt-5") && model.contains("override"),
        "{text}"
    );
    assert_eq!(
        &b.hints()[..3],
        &[("h/l", "profile"), ("M", "model"), ("e", "effort")]
    );

    b.on_key(key('h'), ctx);
    b.on_key(key('h'), ctx);
    let (text, _) = draw(&mut b, ctx);
    assert!(
        line(&text, "profile ").contains("[fast]") && !text.contains("(overridden)"),
        "a new profile drops the overrides\n{text}"
    );
    assert!(
        line(&text, "model ").contains("sonnet") && line(&text, "model ").contains("from profile"),
        "{text}"
    );
    assert!(line(&text, "effort ").contains("[low]"), "{text}");
    assert_eq!(
        sheet(&mut b).choice.choice(),
        ProfileChoice {
            profile: Some("fast".into()),
            model: None,
            effort: None,
        }
    );

    b.on_key(key('M'), ctx);
    b.on_key(code(KeyCode::Esc), ctx);
    assert!(
        b.overlay.is_some() && sheet(&mut b).choice.picker.is_none(),
        "esc in the picker closes only the picker"
    );
    b.on_key(key('l'), ctx);
    assert!(
        matches!(b.on_key(key('M'), ctx), Action::Toast(t) if t.body.contains("takes no model flag")),
        "generic has no model flag"
    );
}

#[test]
fn effort_cycle_wraps_back_to_the_profile_effort() {
    let fx = fixture();
    let ctx = &fx.ctx;
    let mut b = open_sheet(ctx);
    b.on_key(key('h'), ctx);
    b.on_key(key('h'), ctx);
    let efforts = |b: &mut BoardView| sheet(b).choice.effort.clone();
    let mut seen = Vec::new();
    for _ in 0..5 {
        b.on_key(key('e'), ctx);
        seen.push(efforts(&mut b));
    }
    let s = |v: &str| Some(v.to_string());
    assert_eq!(
        seen,
        [s("medium"), s("high"), s("xhigh"), s("max"), None],
        "fast is claude at low: cycles up, then back to the profile's own level"
    );
}

#[test]
fn dispatch_sheet_controls_work_with_the_mouse() {
    let fx = fixture();
    let ctx = &fx.ctx;
    let mut b = open_sheet(ctx);
    let (text, hits) = draw(&mut b, ctx);
    let (x, y) = locate(&text, "profile ", " fast ");
    click(&mut b, ctx, &hits, (x + 1, y), false);
    let (text, hits) = draw(&mut b, ctx);
    assert!(line(&text, "profile ").contains("[fast]"), "{text}");

    let (x, y) = locate(&text, "effort ", " high ");
    click(&mut b, ctx, &hits, (x + 2, y), false);
    let (text, hits) = draw(&mut b, ctx);
    assert!(
        line(&text, "effort ").contains("[high]") && overridden(&text, "[fast]"),
        "{text}"
    );

    let (x, y) = locate(&text, "mode ", " headless ");
    click(&mut b, ctx, &hits, (x + 1, y), false);
    assert_eq!(sheet(&mut b).mode, RunMode::Headless);

    sheet(&mut b).choice.report = Some(Ok(report()));
    let (x, y) = locate(&text, "model ", "sonnet");
    click(&mut b, ctx, &hits, (x, y), false);
    let (text, hits) = draw(&mut b, ctx);
    assert!(
        text.contains("model · claude"),
        "model row opens the picker\n{text}"
    );

    let (x, y) = locate(&text, "", "haiku");
    click(&mut b, ctx, &hits, (x, y), false);
    assert!(
        sheet(&mut b).choice.picker.is_some(),
        "first click selects the row"
    );
    let (_, hits) = draw(&mut b, ctx);
    click(&mut b, ctx, &hits, (x, y), false);
    assert!(sheet(&mut b).choice.picker.is_none(), "second click picks");
    assert_eq!(sheet(&mut b).choice.model.as_deref(), Some("haiku"));

    let (text, hits) = draw(&mut b, ctx);
    let (x, y) = locate(&text, "model ", "haiku");
    click(&mut b, ctx, &hits, (x, y), false);
    let (text, hits) = draw(&mut b, ctx);
    let (x, y) = locate(&text, "", "profile default (sonnet)");
    click(&mut b, ctx, &hits, (x, y), true);
    assert_eq!(
        sheet(&mut b).choice.model,
        None,
        "double-click on profile default drops the override"
    );

    let (text, hits) = draw(&mut b, ctx);
    let (x, y) = locate(&text, "model ", "sonnet");
    click(&mut b, ctx, &hits, (x, y), false);
    let (_, hits) = draw(&mut b, ctx);
    click(&mut b, ctx, &hits, (1, 1), false);
    assert!(
        b.overlay.is_some() && sheet(&mut b).choice.picker.is_none(),
        "outside click closes only the picker"
    );
    let (text, hits) = draw(&mut b, ctx);
    let (x, y) = locate(&text, "dispatch T-003", " × ");
    click(&mut b, ctx, &hits, (x + 1, y), false);
    assert!(b.overlay.is_none(), "× closes the sheet");
}

#[test]
fn sheet_pill_clicks_route_through_the_app() {
    let fx = fixture();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    app.handle_key(key('l'));
    app.handle_key(key('d'));
    let render = |app: &mut App| {
        let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        buffer_text(term.backend().buffer())
    };
    let press = |app: &mut App, (x, y): (u16, u16)| {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    };
    let text = render(&mut app);
    let (x, y) = locate(&text, "profile ", " claude ");
    press(&mut app, (x + 1, y));
    let text = render(&mut app);
    let (x, y) = locate(&text, "effort ", " max ");
    press(&mut app, (x + 1, y));
    let text = render(&mut app);
    assert!(
        overridden(&text, "[claude]") && line(&text, "effort ").contains("[max]"),
        "{text}"
    );
    let footer = text.lines().count() - 1;
    let (x, _) = locate(&text, "dispatch", "e effort");
    press(&mut app, (x, footer as u16));
    let text = render(&mut app);
    assert!(
        !text.contains("(overridden)"),
        "footer `e` cycles past max back to the profile's effort\n{text}"
    );
}
