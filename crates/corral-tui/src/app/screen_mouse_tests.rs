//! Mouse-only flows on every non-board screen and popup (docs/DESIGN.md §10 per-screen table).

use corral_core::{ArtifactKind, Project, ReviewOverride, Status};
use ratatui::crossterm::event::{KeyCode, MouseEventKind};

use super::App;
use super::mouse_tests::{click, find_any, mouse, render};
use crate::artifacts::panels_fixture::fixture;
use crate::board::tests::{code, key, seeded};
use crate::{Action, Hit, Mode, Toast, Tone};

fn wheel(app: &mut App, kind: MouseEventKind, (x, y): (u16, u16)) {
    app.handle_mouse(mouse(kind, x, y));
}

fn status(app: &App, id: &str) -> Status {
    app.ctx.store.get(id).unwrap().status
}

#[test]
fn detail_tabs_rows_wheel_preview_and_back() {
    let fx = fixture();
    let body: Vec<String> = (1..=80).map(|i| format!("spec line {i:02}")).collect();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.body = body.join("\n");
            Ok(())
        })
        .unwrap();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    app.apply(Action::OpenDetail(fx.id.clone()));

    let text = render(&mut app);
    assert!(text.contains("spec line 01"), "{text}");
    wheel(
        &mut app,
        MouseEventKind::ScrollDown,
        find_any(&text, "spec line 05"),
    );
    let text = render(&mut app);
    assert!(
        !text.contains("spec line 03") && text.contains("spec line 04"),
        "wheel over the spec scrolls it\n{text}"
    );
    wheel(
        &mut app,
        MouseEventKind::ScrollDown,
        find_any(&text, "No live run."),
    );
    assert_eq!(
        render(&mut app),
        text,
        "wheel over Live leaves the spec alone"
    );

    let (x, y) = find_any(&text, "3 Artifacts");
    click(&mut app, x, y);
    let text = render(&mut app);
    let (x, y) = find_any(&text, "QA checklist");
    click(&mut app, x, y);
    let text = render(&mut app);
    assert!(
        text.contains("all green"),
        "a click on the selected artifact previews it\n{text}"
    );
    click(&mut app, 0, 2);
    let text = render(&mut app);
    assert!(
        !text.contains("all green") && text.contains("QA checklist"),
        "an outside click closes the preview only\n{text}"
    );

    let (x, y) = find_any(&text, "2 Runs");
    click(&mut app, x, y);
    let text = render(&mut app);
    assert!(text.contains("outcome"), "Runs tab by click\n{text}");
    let (x, y) = find_any(&text, "‹ back");
    click(&mut app, x, y);
    assert_eq!(app.stack.len(), 1, "‹ back is Esc");
}

#[test]
fn review_approve_by_mouse_goes_through_the_merge_sheet() {
    let fx = fixture();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    app.apply(Action::OpenReview(fx.id.clone()));

    let text = render(&mut app);
    let (x, y) = find_any(&text, "[Approve]");
    click(&mut app, x + 1, y);
    assert!(render(&mut app).contains("keep branch, mark done only"));
    click(&mut app, 0, 2);
    assert!(
        !render(&mut app).contains("keep branch, mark done only"),
        "an outside click cancels the merge sheet"
    );
    assert_eq!(status(&app, &fx.id), Status::Review);

    let text = render(&mut app);
    let (x, y) = find_any(&text, "[Approve]");
    click(&mut app, x + 1, y);
    let text = render(&mut app);
    let (x, y) = find_any(&text, "keep branch, mark done only");
    click(&mut app, x, y);
    let text = render(&mut app);
    assert!(text.contains("(•) keep branch"), "{text}");
    let (x, y) = find_any(&text, "[Approve]");
    click(&mut app, x + 1, y);
    assert_eq!(app.stack.len(), 1, "approved: back to the board");
    assert_eq!(status(&app, &fx.id), Status::Done);
}

#[test]
fn review_wheel_file_jump_and_discard_confirm() {
    let fx = fixture();
    let worktree = fx.ctx.store.get(&fx.id).unwrap().runs[0].worktree.clone();
    let big: String = (1..=200).map(|i| format!("big line {i:03}\n")).collect();
    std::fs::write(worktree.join("big.txt"), big).unwrap();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    app.apply(Action::OpenReview(fx.id.clone()));

    let text = render(&mut app);
    let (_, title_y) = find_any(&text, "Files 2");
    let top = |text: &str| {
        text.lines()
            .nth(usize::from(title_y) + 1)
            .unwrap()
            .to_string()
    };
    let first = find_any(&text, "shard_cache.rs");
    wheel(&mut app, MouseEventKind::ScrollDown, first);
    let text = render(&mut app);
    assert!(
        top(&text).contains("diff --git a/shard_cache.rs"),
        "wheel scrolls the Files box\n{text}"
    );
    wheel(&mut app, MouseEventKind::ScrollUp, first);
    let text = render(&mut app);
    let (x, y) = find_any(&text, "big.txt");
    click(&mut app, x, y);
    let text = render(&mut app);
    assert!(
        top(&text).contains("diff --git a/big.txt b/big.txt"),
        "a file row jumps to its diff\n{text}"
    );

    let (x, y) = find_any(&text, "[Discard]");
    click(&mut app, x + 1, y);
    let text = render(&mut app);
    assert!(text.contains("Discard run R1?"), "{text}");
    let (x, y) = find_any(&text, "[Cancel]");
    click(&mut app, x + 1, y);
    assert!(!render(&mut app).contains("Discard run R1?"));
    assert_eq!(status(&app, &fx.id), Status::Review, "cancel keeps the run");

    let text = render(&mut app);
    let (x, y) = find_any(&text, "[Discard]");
    click(&mut app, x + 1, y);
    let text = render(&mut app);
    let (x, y) = find_any(&text, "[Discard]");
    click(&mut app, x + 1, y);
    assert_eq!(app.stack.len(), 1);
    assert_eq!(status(&app, &fx.id), Status::Dropped);
}

#[test]
fn artifact_finder_click_wheel_and_close() {
    let fx = fixture();
    let notes = fx.root.join("notes.md");
    let lines: String = (1..=120).map(|i| format!("note line {i:03}\n")).collect();
    std::fs::write(&notes, format!("# Notes\n{lines}")).unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &notes,
        Some(ArtifactKind::Plan),
        Some("Design notes"),
    )
    .unwrap();
    let mut app = App::new(fx.ctx.clone(), Mode::Artifacts).unwrap();

    let text = render(&mut app);
    if text.contains("note line 001") {
        let (x, y) = find_any(&text, "QA checklist");
        click(&mut app, x, y);
        assert!(
            render(&mut app).contains("all green"),
            "click selects a row"
        );
    }
    let text = render(&mut app);
    let (x, y) = find_any(&text, "Design notes");
    click(&mut app, x, y);
    let text = render(&mut app);
    assert!(
        text.contains("note line 001"),
        "click selects a row\n{text}"
    );

    wheel(
        &mut app,
        MouseEventKind::ScrollDown,
        find_any(&text, "note line 005"),
    );
    let text = render(&mut app);
    assert!(
        !text.contains("note line 002") && text.contains("note line 004"),
        "wheel over the preview scrolls it\n{text}"
    );

    let design = find_any(&text, "Design notes");
    let qa = find_any(&text, "QA checklist");
    let toward_qa = if qa.1 > design.1 {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    wheel(&mut app, toward_qa, design);
    let text = render(&mut app);
    assert!(
        text.contains("all green"),
        "wheel over the list moves the selection\n{text}"
    );

    let (x, y) = find_any(&text, "filter:");
    click(&mut app, x, y);
    assert_eq!(render(&mut app), text, "the filter already has focus");
    let (x, y) = find_any(&text, " × ");
    click(&mut app, x + 1, y);
    assert!(app.quit, "× closes the finder");
}

#[test]
fn dispatch_sheet_pills_and_buttons() {
    let fx = seeded();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    app.handle_key(key('l'));
    app.handle_key(key('d'));
    let text = render(&mut app);
    assert!(text.contains("[interactive]"), "{text}");
    let (x, y) = find_any(&text, " headless ");
    click(&mut app, x + 1, y);
    let text = render(&mut app);
    assert!(text.contains("[headless]"), "mode pill by click\n{text}");

    let (x, y) = find_any(&text, "[Dispatch]");
    assert_eq!(
        app.hits.at(x + 1, y),
        Some(&Hit::Key(code(KeyCode::Enter))),
        "[Dispatch] presses ⏎"
    );
    let (x, y) = find_any(&text, "[Cancel]");
    click(&mut app, x + 1, y);
    assert!(!render(&mut app).contains("[Dispatch]"), "[Cancel] closes");
    assert_eq!(status(&app, "T-003"), Status::Ready);
}

#[test]
fn capture_pills_and_save_button() {
    let fx = seeded();
    let mut ctx = fx.ctx.clone();
    for p in ["alpha", "beta"] {
        let project = Project {
            path: p.into(),
            default_profile: None,
            base: None,
            review: ReviewOverride::default(),
        };
        ctx.store.config.projects.insert(p.into(), project);
    }
    let cwd = ctx.store.root().to_path_buf();
    let mut app = App::new(ctx, Mode::Capture(cwd)).unwrap();
    for ch in "picked by mouse".chars() {
        app.handle_key(key(ch));
    }
    let text = render(&mut app);
    let (x, y) = find_any(&text, " task ");
    click(&mut app, x + 1, y);
    let (x, y) = find_any(&text, " beta ");
    click(&mut app, x + 1, y);
    let text = render(&mut app);
    assert!(text.contains("[task]") && text.contains("[beta]"), "{text}");
    assert_eq!(app.stack[0].hints()[0], ("⏎", "save task"));

    let (x, y) = find_any(&text, "[Save]");
    click(&mut app, x + 1, y);
    assert!(app.quit);
    let saved = fx.ctx.store.get("T-008").unwrap();
    assert_eq!(
        (saved.title.as_str(), saved.status, saved.project.as_deref()),
        ("picked by mouse", Status::Backlog, Some("beta"))
    );
}

#[test]
fn help_key_click_acts_on_the_board() {
    let fx = seeded();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    app.handle_key(key('?'));
    let text = render(&mut app);
    let (x, y) = find_any(&text, "artifact finder");
    click(&mut app, x - 18, y);
    assert_eq!(app.stack.len(), 2, "clicking A in help opens the finder");
    app.handle_key(code(KeyCode::Esc));
    assert!(
        !render(&mut app).contains("move selection"),
        "help closed first"
    );
}

#[test]
fn toast_click_dismisses_or_opens_its_task() {
    let fx = seeded();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    app.toast(Toast::new(Tone::Info, "plain note", "nothing to open"));
    app.toast(Toast::new(Tone::Attention, "▲ T-005 needs you", "which file?").about("T-005"));
    let text = render(&mut app);
    let (x, y) = find_any(&text, "plain note");
    click(&mut app, x, y);
    assert_eq!(
        (app.toasts.len(), app.stack.len()),
        (1, 1),
        "plain toast dismissed"
    );
    let text = render(&mut app);
    let (x, y) = find_any(&text, "T-005 needs you");
    click(&mut app, x, y);
    assert_eq!(
        (app.toasts.len(), app.stack.len()),
        (0, 2),
        "task toast opens detail"
    );
}

#[test]
fn reply_line_buttons() {
    let fx = seeded();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    for _ in 0..3 {
        app.handle_key(key('l'));
    }
    app.handle_key(key('r'));
    let text = render(&mut app);
    let (x, y) = find_any(&text, "type your answer");
    click(&mut app, x, y);
    let text = render(&mut app);
    assert!(
        text.contains("Reply → T-005"),
        "a click in the field keeps it\n{text}"
    );
    let (x, y) = find_any(&text, "[Cancel]");
    click(&mut app, x + 1, y);
    assert!(
        !render(&mut app).contains("Reply → T-005"),
        "[Cancel] closes"
    );
}
