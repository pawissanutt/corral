use corral_core::{ArtifactKind, ArtifactTier};
use ratatui::crossterm::event::KeyCode;

use super::panels_fixture::{Fixture, fixture, key, render_at};
use super::ui::PreviewPopup;
use super::{ArtifactsView, SORT_KEY, TIER_KEY};
use crate::hit::Hit;
use crate::{Action, View};

const SIZES: [(u16, u16); 2] = [(80, 24), (160, 45)];

#[test]
fn compressed_preview_titles_preserve_label_size_and_tier_at_both_sizes() {
    // Given a compressed artifact with a filename and title longer than the narrow preview.
    let mut fx = fixture();
    fx.ctx.store.config.artifacts.retention.larger_than = Some("1B".into());
    let source = fx.root.join("compressed-raw-output.log");
    std::fs::write(&source, vec![b'L'; 4096]).unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &source,
        Some(ArtifactKind::Log),
        Some("Compressed raw command output with a long title"),
    )
    .unwrap();
    corral_ops::artifact_gc(&fx.ctx, false).unwrap();

    // When finder and detail preview titles render at either supported terminal size.
    for (w, h) in SIZES {
        let mut view = ArtifactsView::new(&fx.ctx, None).unwrap();
        for c in "compressed".chars() {
            view.on_key(key(KeyCode::Char(c)), &fx.ctx);
        }
        let artifact = view.selected().unwrap().artifact.clone();
        assert!(artifact.compressed_bytes.is_some());
        let (screen, _) = render_at(&mut view, &fx.ctx, w, h);
        let header = screen
            .lines()
            .find(|line| line.contains("╭ preview "))
            .unwrap();

        // Then neither the label nor the original size and full tier are overwritten.
        assert!(header.contains("5 KB"), "{w}x{h}\n{screen}");
        assert!(header.contains("transient ╮"), "{w}x{h}\n{screen}");
        let mut popup = PreviewPopup::open(&fx.ctx, &artifact);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| popup.draw(f, f.area(), &mut crate::HitMap::default()))
            .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("5 KB"), "{w}x{h}\n{text}");
        assert!(text.contains("transient"), "{w}x{h}\n{text}");
    }
}

/// Sets the tier override and `added` time of the fixture task's artifact titled `title`.
fn set(fx: &Fixture, title: &str, tier: ArtifactTier, added: &str) {
    let added = added.parse().unwrap();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            for a in t.artifacts.iter_mut().filter(|a| a.title == title) {
                a.tier_override = Some(tier);
                a.added = added;
            }
            Ok(())
        })
        .unwrap();
}

fn add(fx: &Fixture, file: &str, title: &str, tier: ArtifactTier, added: &str) {
    let src = fx.root.join(file);
    std::fs::write(&src, format!("# {title}\n")).unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &src,
        Some(ArtifactKind::Report),
        Some(title),
    )
    .unwrap();
    set(fx, title, tier, added);
}

/// Newest first: QA checklist (normal), Build log (transient), Design notes (keep).
fn tiered() -> Fixture {
    let fx = fixture();
    set(
        &fx,
        "QA checklist",
        ArtifactTier::Normal,
        "2026-01-01T12:03:00Z",
    );
    add(
        &fx,
        "build.log",
        "Build log",
        ArtifactTier::Transient,
        "2026-01-01T12:02:00Z",
    );
    add(
        &fx,
        "notes.md",
        "Design notes",
        ArtifactTier::Keep,
        "2026-01-01T12:01:00Z",
    );
    fx
}

fn order(v: &ArtifactsView) -> Vec<String> {
    v.matches
        .iter()
        .map(|&i| v.rows[i].artifact.title.clone())
        .collect()
}

fn selected(v: &ArtifactsView) -> String {
    v.selected().unwrap().artifact.title.clone()
}

fn find(screen: &str, needle: &str) -> Option<(u16, u16)> {
    screen.lines().enumerate().find_map(|(y, line)| {
        line.find(needle)
            .map(|b| (line[..b].chars().count() as u16, y as u16))
    })
}

/// Clicks the `[label]` button the way the app does: the topmost hit under the button's first label cell, and a
/// `Hit::Key` is pressed through `on_key`.
fn click(v: &mut ArtifactsView, fx: &Fixture, (w, h): (u16, u16), label: &str) -> Action {
    let (screen, hits) = render_at(v, &fx.ctx, w, h);
    let (x, y) = find(&screen, &format!("[{label}]"))
        .unwrap_or_else(|| panic!("{w}x{h}: no [{label}] button:\n{screen}"));
    match hits.at(x + 1, y).cloned() {
        Some(Hit::Key(k)) => v.on_key(k, &fx.ctx),
        other => panic!("{w}x{h}: [{label}] is not a key button: {other:?}"),
    }
}

#[test]
fn tier_sort_key_and_click_agree_at_both_sizes() {
    for size @ (w, h) in SIZES {
        let fx = tiered();
        let mut by_key = ArtifactsView::new(&fx.ctx, None).unwrap();
        let mut by_click = ArtifactsView::new(&fx.ctx, None).unwrap();
        assert_eq!(
            order(&by_key),
            ["QA checklist", "Build log", "Design notes"]
        );

        assert_eq!(by_key.on_key(SORT_KEY, &fx.ctx), Action::None);
        assert_eq!(
            click(&mut by_click, &fx, size, "sort: newest"),
            Action::None
        );
        assert_eq!(
            order(&by_key),
            ["Design notes", "QA checklist", "Build log"],
            "{w}x{h}"
        );
        assert_eq!(order(&by_click), order(&by_key), "{w}x{h}");
        assert_eq!(
            selected(&by_key),
            "QA checklist",
            "selection follows the row"
        );
        assert_eq!(selected(&by_click), "QA checklist");
        let (a, _) = render_at(&mut by_key, &fx.ctx, w, h);
        let (b, _) = render_at(&mut by_click, &fx.ctx, w, h);
        assert_eq!(a, b, "{w}x{h}: key and click render differently");
        assert!(a.contains("[sort: tier]"), "{w}x{h}\n{a}");

        by_key.on_key(SORT_KEY, &fx.ctx);
        click(&mut by_click, &fx, size, "sort: tier");
        assert_eq!(
            order(&by_key),
            ["QA checklist", "Build log", "Design notes"]
        );
        assert_eq!(order(&by_click), order(&by_key));
    }
}

#[test]
fn tier_filter_key_and_click_agree_at_both_sizes() {
    let cycle = [
        ("tier: all", "tier: keep", vec!["Design notes"]),
        ("tier: keep", "tier: normal", vec!["QA checklist"]),
        ("tier: normal", "tier: transient", vec!["Build log"]),
        (
            "tier: transient",
            "tier: all",
            vec!["QA checklist", "Build log", "Design notes"],
        ),
    ];
    for size @ (w, h) in SIZES {
        let fx = tiered();
        let mut by_key = ArtifactsView::new(&fx.ctx, None).unwrap();
        let mut by_click = ArtifactsView::new(&fx.ctx, None).unwrap();
        for (from, to, want) in &cycle {
            by_key.on_key(TIER_KEY, &fx.ctx);
            click(&mut by_click, &fx, size, from);
            assert_eq!(&order(&by_key), want, "{w}x{h} {from} -> {to}");
            assert_eq!(order(&by_click), order(&by_key), "{w}x{h} {from} -> {to}");
            let (a, _) = render_at(&mut by_key, &fx.ctx, w, h);
            let (b, _) = render_at(&mut by_click, &fx.ctx, w, h);
            assert_eq!(a, b, "{w}x{h}: key and click render differently");
            assert!(a.contains(&format!("[{to}]")), "{w}x{h}\n{a}");
        }
    }
}

#[test]
fn tier_badges_controls_and_preview_titles_render_unclipped() {
    let fx = tiered();
    for (w, h) in SIZES {
        let mut v = ArtifactsView::new(&fx.ctx, None).unwrap();
        let (screen, _) = render_at(&mut v, &fx.ctx, w, h);
        for needle in [
            "[sort: newest]",
            "[tier: all]",
            "keep ",
            "norm ",
            "temp ",
            "╭ preview ",
            "26 B · normal ╮",
        ] {
            assert!(
                screen.contains(needle),
                "{w}x{h}: missing {needle:?}\n{screen}"
            );
        }
        let (sx, sy) = find(&screen, "[sort: newest]").unwrap();
        let (tx, ty) = find(&screen, "[tier: all]").unwrap();
        assert_eq!(sy, ty);
        assert!(sx < tx && tx + 11 < w, "{w}x{h}: buttons at {sx}/{tx}");
    }

    let mut v = ArtifactsView::new(&fx.ctx, None).unwrap();
    let (screen, _) = render_at(&mut v, &fx.ctx, 160, 45);
    assert!(
        screen.contains("keep 1 · normal 1 · transient 1"),
        "{screen}"
    );
    assert!(screen.contains("qa-report.md · 26 B · normal"), "{screen}");

    for c in "transient".chars() {
        v.on_key(key(KeyCode::Char(c)), &fx.ctx);
    }
    assert_eq!(order(&v), ["Build log"]);

    let row = v.selected().unwrap().artifact.clone();
    let mut popup = PreviewPopup::open(&fx.ctx, &row);
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    term.draw(|f| popup.draw(f, f.area(), &mut crate::HitMap::default()))
        .unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("· transient"), "{text}");
}
