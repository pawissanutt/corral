use corral_core::ArtifactKind;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;

use super::ArtifactsView;
use super::panels_fixture::{fixture, key, render};
use super::ui::PreviewPopup;
use crate::{Action, View};

const TWO_TONE_PNG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/two-tone.png");

/// Rows holding solid red / solid blue image cells (halfblocks draws a cell whose two pixels match as a space
/// with that background), so the red top half must sit above the blue bottom half.
fn image_rows(buf: &Buffer) -> (Vec<u16>, Vec<u16>) {
    let (mut red, mut blue) = (Vec::new(), Vec::new());
    for y in 0..buf.area.height {
        let row = (0..buf.area.width).filter_map(|x| buf.cell((x, y)));
        let bgs: Vec<Color> = row.map(|c| c.bg).collect();
        if bgs.iter().filter(|&&c| c == Color::Rgb(255, 0, 0)).count() >= 4 {
            red.push(y);
        }
        if bgs.iter().filter(|&&c| c == Color::Rgb(0, 0, 255)).count() >= 4 {
            blue.push(y);
        }
    }
    (red, blue)
}

fn assert_two_tone(buf: &Buffer, what: &str) {
    let (red, blue) = image_rows(buf);
    assert!(
        red.len() >= 3 && blue.len() >= 3,
        "{what}: red rows {red:?}, blue rows {blue:?}"
    );
    assert!(
        red.iter().max() < blue.iter().min(),
        "{what}: red must be above blue: {red:?} / {blue:?}"
    );
    let boundary = buf
        .content()
        .iter()
        .any(|c| c.symbol() == "▀" || c.symbol() == "▄");
    assert!(
        boundary,
        "{what}: no half-block glyph at the color boundary"
    );
}

#[test]
fn panels_image_preview_renders_halfblocks() {
    let fx = fixture();
    let shot = fx.root.join("two-tone.png");
    std::fs::copy(TWO_TONE_PNG, &shot).unwrap();
    let a = corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &shot,
        Some(ArtifactKind::Screenshot),
        Some("Two tone"),
    )
    .unwrap();

    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut popup = PreviewPopup::open(&fx.ctx, &a);
    term.draw(|f| popup.draw(f, f.area(), &mut crate::HitMap::default()))
        .unwrap();
    assert_two_tone(term.backend().buffer(), "preview popup");

    let mut finder = ArtifactsView::new(&fx.ctx, Some(&fx.id)).unwrap();
    finder.on_key(key(KeyCode::Down), &fx.ctx);
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    term.draw(|f| {
        let area = f.area();
        finder.draw(f, area, &fx.ctx, &mut crate::HitMap::default());
    })
    .unwrap();
    assert_two_tone(term.backend().buffer(), "finder preview");
    let screen = render(&mut finder, &fx.ctx);
    assert!(screen.contains("png image (82 bytes)"), "{screen}");
    assert!(screen.contains("⏎ open"), "{screen}");
}

#[test]
fn panels_artifacts_lists_and_filters() {
    let fx = fixture();
    let mut v = ArtifactsView::new(&fx.ctx, None).unwrap();
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("QA checklist"), "{screen}");
    assert!(screen.contains("artifacts · 1 across 1 tasks"), "{screen}");
    assert!(screen.contains("all green"), "preview missing:\n{screen}");

    for c in "zzz".chars() {
        assert_eq!(v.on_key(key(KeyCode::Char(c)), &fx.ctx), Action::None);
    }
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("no artifacts match"), "{screen}");
    assert!(!screen.contains("QA checklist"), "{screen}");

    for _ in 0..3 {
        v.on_key(key(KeyCode::Backspace), &fx.ctx);
    }
    for c in "report app".chars() {
        v.on_key(key(KeyCode::Char(c)), &fx.ctx);
    }
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("QA checklist"), "{screen}");
    assert_eq!(v.on_key(key(KeyCode::Esc), &fx.ctx), Action::Back);
}

#[test]
fn panels_artifacts_only_task_filter() {
    let fx = fixture();
    let mut v = ArtifactsView::new(&fx.ctx, Some(&fx.id)).unwrap();
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("QA checklist"), "{screen}");
    let mut other = ArtifactsView::new(&fx.ctx, Some("T-999")).unwrap();
    let screen = render(&mut other, &fx.ctx);
    assert!(screen.contains("No artifacts yet"), "{screen}");
}
