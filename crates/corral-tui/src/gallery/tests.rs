use corral_core::ArtifactKind;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyModifiers;

use super::*;
use crate::artifacts::panels_fixture::{Fixture, fixture};
use crate::board::tests::{buffer_text, code, key};

fn picture(fixture: &Fixture, name: &str, color: u8) -> corral_core::Artifact {
    let path = fixture.root.join(name);
    ::image::RgbImage::from_pixel(32, 16, ::image::Rgb([color, 80, 180]))
        .save(&path)
        .unwrap();
    corral_ops::add_artifact(
        &fixture.ctx,
        &fixture.id,
        &path,
        Some(ArtifactKind::Screenshot),
        Some(name),
    )
    .unwrap()
}

fn draw(view: &mut GalleryView, ctx: &Ctx, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| view.draw(frame, frame.area(), ctx, &mut HitMap::default()))
        .unwrap();
    buffer_text(terminal.backend().buffer())
}

#[test]
fn gallery_renders_images_and_keeps_pinned_outputs_ahead_of_recent() {
    // Given both a pinned older visual and a recent one plus a nonvisual report.
    let fixture = fixture();
    let pinned = picture(&fixture, "older.png", 40);
    picture(&fixture, "recent.png", 200);
    fixture
        .ctx
        .store
        .pin(
            &PinRef::of_artifact(&fixture.id, &pinned).to_string(),
            None,
            false,
        )
        .unwrap();
    let report = fixture.root.join("report.md");
    std::fs::write(&report, "not a visual").unwrap();
    corral_ops::add_artifact(&fixture.ctx, &fixture.id, &report, None, None).unwrap();
    // When the real view renders at narrow and wide terminal sizes.
    let mut view = GalleryView::new(&fixture.ctx).unwrap();
    for (width, height) in [(40, 18), (80, 24), (160, 45)] {
        let screen = draw(&mut view, &fixture.ctx, width, height);
        // Then only visuals are included and both can render actual decoded image state.
        assert_eq!(view.matches.len(), 2);
        assert!(view.tiles.iter().all(|tile| tile.image.is_ok()));
        assert_eq!(view.tile().unwrap().row.artifact.path, pinned.path);
        assert!(screen.contains("older.png"), "{screen}");
    }
}

#[test]
fn filter_pinned_scope_and_navigation_use_the_visible_grid() {
    // Given three visuals in a two-column gallery with one pin.
    let fixture = fixture();
    let pinned = picture(&fixture, "pinned.png", 30);
    picture(&fixture, "chart.png", 80);
    picture(&fixture, "status.png", 180);
    fixture
        .ctx
        .store
        .pin(
            &PinRef::of_artifact(&fixture.id, &pinned).to_string(),
            None,
            false,
        )
        .unwrap();
    let mut view = GalleryView::new(&fixture.ctx).unwrap();
    draw(&mut view, &fixture.ctx, 80, 24);
    // When navigating vertically then switching to pinned scope.
    view.on_key(code(KeyCode::Down), &fixture.ctx);
    assert_eq!(view.selected, 2);
    view.on_key(code(KeyCode::Tab), &fixture.ctx);
    // Then the selection is bounded to the pinned output and the text filter composes with it.
    assert_eq!(view.matches.len(), 1);
    assert_eq!(view.tile().unwrap().row.artifact.path, pinned.path);
    view.on_key(key('z'), &fixture.ctx);
    assert!(view.matches.is_empty());
    view.on_key(code(KeyCode::Backspace), &fixture.ctx);
    assert_eq!(view.matches.len(), 1);
    assert_eq!(
        view.on_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &fixture.ctx
        ),
        Action::OpenArtifacts
    );
}

#[test]
fn following_pin_marks_latest_version_and_unpins_its_anchor() {
    // Given a follow-latest pin anchored on the first dashboard version.
    let fixture = fixture();
    let first = picture(&fixture, "dashboard.png", 30);
    let target = PinRef::of_artifact(&fixture.id, &first).to_string();
    fixture.ctx.store.pin(&target, None, true).unwrap();
    let second = picture(&fixture, "dashboard.png", 210);
    // When the refreshed gallery selects that pin and toggles it.
    let mut view = GalleryView::new(&fixture.ctx).unwrap();
    assert_eq!(view.tile().unwrap().row.artifact.path, second.path);
    assert_eq!(view.tile().unwrap().pin.as_deref(), Some(target.as_str()));
    view.on_key(key('*'), &fixture.ctx);
    // Then the actual stored anchor is removed rather than a second pin created for its latest target.
    assert!(fixture.ctx.store.pins().unwrap().is_empty());
}

#[test]
fn session_artifact_pin_marks_latest_visual_and_survives_pinned_only_filter() {
    let fixture = fixture();
    fixture
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    picture(&fixture, "task.png", 50);
    let path = fixture.root.join("session.png");
    ::image::RgbImage::from_pixel(32, 16, ::image::Rgb([30, 80, 180]))
        .save(&path)
        .unwrap();
    let first = corral_ops::add_session_artifact(
        &fixture.ctx,
        "A-1",
        &path,
        Some(ArtifactKind::Screenshot),
        None,
    )
    .unwrap();
    let anchor = PinRef::of_artifact("A-1", &first).to_string();
    fixture.ctx.store.pin(&anchor, None, true).unwrap();
    ::image::RgbImage::from_pixel(32, 16, ::image::Rgb([210, 80, 180]))
        .save(&path)
        .unwrap();
    let latest = corral_ops::add_session_artifact(
        &fixture.ctx,
        "A-1",
        &path,
        Some(ArtifactKind::Screenshot),
        None,
    )
    .unwrap();

    let mut view = GalleryView::new(&fixture.ctx).unwrap();
    assert_eq!(view.tile().unwrap().row.artifact.path, latest.path);
    assert_eq!(view.tile().unwrap().pin.as_deref(), Some(anchor.as_str()));
    assert!(view.tiles.iter().all(|tile| tile.image.is_ok()));
    view.on_key(code(KeyCode::Tab), &fixture.ctx);
    assert_eq!(view.matches.len(), 1);
    assert_eq!(view.tile().unwrap().row.task_id, "A-1");
    assert_eq!(view.tile().unwrap().row.artifact.path, latest.path);
    let screen = draw(&mut view, &fixture.ctx, 80, 24);
    assert!(screen.contains("session.png"), "{screen}");
    assert!(!screen.contains("task.png"), "{screen}");

    // Toggling the followed session visual removes its stored anchor, not a second pin.
    view.on_key(key('*'), &fixture.ctx);
    assert!(fixture.ctx.store.pins().unwrap().is_empty());
    view.refresh(&fixture.ctx);
    assert!(view.matches.is_empty());
}

#[test]
fn remapped_gallery_key_replaces_default_without_intercepting_filter_text() {
    // Given a persisted-compatible gallery remap and a visual artifact.
    let mut fixture = fixture();
    picture(&fixture, "chart.png", 50);
    fixture
        .ctx
        .store
        .config
        .ui
        .keys
        .insert("gallery".into(), "v".into());
    fixture
        .ctx
        .store
        .config
        .validate(fixture.ctx.store.root())
        .unwrap();
    let mut app = crate::app::App::new(fixture.ctx.clone(), crate::Mode::Board).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    // When the old and new shortcuts pass through the real application router.
    app.handle_key(key('V'));
    terminal.draw(|frame| app.draw(frame)).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Gallery /"));
    app.handle_key(key('v'));
    terminal.draw(|frame| app.draw(frame)).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Gallery /"));
    app.handle_key(key('v'));
    terminal.draw(|frame| app.draw(frame)).unwrap();

    // Then the remapped key becomes literal filter input inside the gallery.
    assert!(buffer_text(terminal.backend().buffer()).contains("No matching visuals"));
}

#[test]
fn board_and_finder_open_gallery_and_escape_returns_to_parent() {
    // Given the real application router.
    let fixture = fixture();
    picture(&fixture, "chart.png", 50);
    let mut app = crate::app::App::new(fixture.ctx.clone(), crate::Mode::Board).unwrap();
    // When opening the gallery by its board key, then returning.
    app.handle_key(key('V'));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Gallery /"));
    app.handle_key(code(KeyCode::Esc));
    app.handle_key(key('A'));
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    terminal.draw(|frame| app.draw(frame)).unwrap();
    // Then the finder reaches the same rendered gallery.
    assert!(buffer_text(terminal.backend().buffer()).contains("Gallery /"));
}
