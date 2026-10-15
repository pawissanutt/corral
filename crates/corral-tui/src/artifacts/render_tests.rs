//! Rendered previews, importance order, the raw fold, narrow-width metadata and the review marks through the
//! real app at 80x24 and 160x45. Every click goes through the frame's hit map (topmost target at the label's
//! coordinates) and the app's mouse dispatch, and is compared with the keyboard path.

use corral_core::{ArtifactKind, ArtifactTier};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::panels_fixture::{Fixture, fixture};
use crate::app::App;
use crate::app::mouse_tests::{click, find_any};
use crate::board::tests::{buffer_text, code, key};
use crate::{Action, Hit, Mode};

const SIZES: [(u16, u16); 2] = [(80, 24), (160, 45)];

const SUMMARY: &str = "# Release summary\n\nShipped the **shard cache**.\n\n- alpha item\n- beta item\n\n| name | value |\n|------|-------|\n| one | 1 |\n\n```json\n{\"ok\": true}\n```\n";

fn add(fx: &Fixture, file: &str, body: &str, kind: ArtifactKind, title: &str) {
    let path = fx.root.join(file);
    std::fs::write(&path, body).unwrap();
    corral_ops::add_artifact(&fx.ctx, &fx.id, &path, Some(kind), Some(title)).unwrap();
}

fn rich() -> Fixture {
    let fx = fixture();
    add(
        &fx,
        "summary.md",
        SUMMARY,
        ArtifactKind::Summary,
        "Release summary",
    );
    let log: String = (1..=80)
        .map(|i| {
            if i == 60 {
                "error: boom\n".to_string()
            } else {
                format!("line {i:03}\n")
            }
        })
        .collect();
    add(&fx, "run.log", &log, ArtifactKind::Log, "Gate: pass");
    add(
        &fx,
        "stat.diff",
        "+++ b/f\n+x\n",
        ArtifactKind::Diff,
        "Final diff",
    );
    let diff = fx.root.join("stat2.diff");
    std::fs::write(&diff, "+++ b/f\n+y\n").unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &diff,
        Some(ArtifactKind::Diff),
        Some("Newer diff"),
    )
    .unwrap();
    add(
        &fx,
        "config.toml",
        "[run]\nname = \"x\"\n",
        ArtifactKind::Evidence,
        "Run config",
    );
    fx
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn draw(app: &mut App, (w, h): (u16, u16)) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    buffer_text(term.backend().buffer())
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.handle_key(key(c));
    }
}

fn finder(fx: &Fixture) -> App {
    App::new(fx.ctx.clone(), Mode::Artifacts).unwrap()
}

/// Clicks a list row `label` where it is drawn; the topmost hit must be that view's row target.
fn press_row(app: &mut App, size: (u16, u16), label: &str) {
    let screen = draw(app, size);
    let (x, y) = find_any(&screen, label);
    match app.hits.at(x, y) {
        Some(Hit::Item(..)) => {}
        other => panic!("{label:?} at {size:?} resolves to {other:?}\n{screen}"),
    }
    click(app, x, y);
}

/// Clicks `label` where it is drawn. The topmost hit there must be a key button: a registered button that a
/// background target covers would fail here.
fn press(app: &mut App, size: (u16, u16), label: &str) {
    let screen = draw(app, size);
    let (x, y) = find_any(&screen, label);
    match app.hits.at(x, y) {
        Some(Hit::Key(_)) => {}
        other => panic!("{label:?} at {size:?} resolves to {other:?}\n{screen}"),
    }
    click(app, x, y);
}

#[test]
fn finder_markdown_toggles_raw_by_key_and_by_click_identically() {
    for size in SIZES {
        let fx = rich();
        let mut app = finder(&fx);
        typed(&mut app, "kind:summary");
        let shown = draw(&mut app, size);
        assert!(
            shown.contains("Release summary") && !shown.contains("# Release"),
            "{size:?}\n{shown}"
        );
        assert!(
            shown.contains("• alpha item") && shown.contains("┌") && !shown.contains("**"),
            "{size:?}\n{shown}"
        );
        assert!(shown.contains("markdown · rendered"), "{size:?}\n{shown}");

        app.handle_key(ctrl('r'));
        let by_key = draw(&mut app, size);
        app.handle_key(ctrl('r'));
        assert_eq!(draw(&mut app, size), shown, "the key toggles back");
        press(&mut app, size, "[^r] raw");
        let by_click = draw(&mut app, size);
        assert_eq!(
            by_key, by_click,
            "{size:?}: key and click render differently"
        );
        assert!(
            by_click.contains("# Release summary")
                && by_click.contains("**shard cache**")
                && !by_click.contains("┌"),
            "{size:?}\n{by_click}"
        );
        assert!(
            by_click.contains("markdown · raw") && by_click.contains("[^r] rendered"),
            "{size:?}\n{by_click}"
        );

        press(&mut app, size, "[^r] rendered");
        assert_eq!(draw(&mut app, size), shown, "the click toggles back");
    }
}

#[test]
fn finder_important_view_folds_raw_rows_and_the_fold_row_is_clickable() {
    for size in SIZES {
        let fx = rich();
        let (mut by_key, mut by_click) = (finder(&fx), finder(&fx));
        let first = draw(&mut by_key, size);
        assert!(
            first.contains("QA checklist") && first.contains("Newer diff"),
            "{size:?}\n{first}"
        );
        assert!(
            !first.contains("Final diff") && !first.contains("Gate: pass"),
            "superseded and raw rows are folded\n{first}"
        );
        assert!(first.contains("raw (2) hidden"), "{size:?}\n{first}");
        let at = |s: &str, n: &str| s.find(n).unwrap_or_else(|| panic!("{n}\n{s}"));
        assert!(
            at(&first, "QA checklist") < at(&first, "Run config"),
            "keep tier before evidence"
        );

        by_key.handle_key(code(KeyCode::Tab));
        press(&mut by_click, size, "raw (2) hidden");
        let (a, b) = (draw(&mut by_key, size), draw(&mut by_click, size));
        assert_eq!(a, b, "{size:?}: key and click render differently");
        assert!(
            a.contains("Final diff") && a.contains("Gate: pass") && !a.contains("raw (2) hidden"),
            "{size:?}\n{a}"
        );
        assert!(
            a.contains("[tier: all]") || a.contains("[tier: "),
            "{size:?}\n{a}"
        );
    }
}

#[test]
fn finder_with_only_raw_artifacts_offers_a_clickable_fold_row() {
    for size in SIZES {
        let fx = fixture();
        fx.ctx
            .store
            .update(&fx.id, |t| {
                for a in &mut t.artifacts {
                    a.tier_override = Some(ArtifactTier::Transient);
                }
                Ok(())
            })
            .unwrap();
        let mut app = finder(&fx);
        let empty = draw(&mut app, size);
        assert!(
            empty.contains("no important artifacts") && empty.contains("raw (1) hidden"),
            "{size:?}\n{empty}"
        );
        press(&mut app, size, "raw (1) hidden");
        let all = draw(&mut app, size);
        assert!(all.contains("QA checklist"), "{size:?}\n{all}");
    }
}

#[test]
fn finder_filters_by_tier_and_kind_tokens() {
    for size in SIZES {
        let fx = rich();
        let mut app = finder(&fx);
        typed(&mut app, "tier:keep");
        let keep = draw(&mut app, size);
        assert!(
            keep.contains("QA checklist") && keep.contains("Release summ"),
            "{keep}"
        );
        assert!(!keep.contains("Run config"), "{keep}");
        for _ in 0..9 {
            app.handle_key(code(KeyCode::Backspace));
        }
        typed(&mut app, "kind:report");
        let reports = draw(&mut app, size);
        assert!(
            reports.contains("QA checklist") && !reports.contains("Release summ"),
            "{reports}"
        );
        for _ in 0..11 {
            app.handle_key(code(KeyCode::Backspace));
        }
        typed(&mut app, "tier:transient");
        let raw = draw(&mut app, size);
        assert!(
            raw.contains("Gate: pass")
                && raw.contains("Final diff")
                && !raw.contains("QA checklist"),
            "{raw}"
        );
    }
}

#[test]
fn log_preview_jumps_to_the_first_error_by_key_and_by_click() {
    for size in SIZES {
        let fx = rich();
        let (mut by_key, mut by_click) = (finder(&fx), finder(&fx));
        for app in [&mut by_key, &mut by_click] {
            app.handle_key(code(KeyCode::Tab));
            typed(app, "kind:log");
        }
        let before = draw(&mut by_key, size);
        assert!(
            before.contains("[^e] error") && before.contains("line 001"),
            "{size:?}\n{before}"
        );
        by_key.handle_key(ctrl('e'));
        press(&mut by_click, size, "[^e] error");
        let (a, b) = (draw(&mut by_key, size), draw(&mut by_click, size));
        assert_eq!(a, b, "{size:?}: key and click render differently");
        assert!(
            a.contains("error: boom") && !a.contains("line 001"),
            "{size:?}\n{a}"
        );
    }
}

#[test]
fn compressed_long_log_reaches_an_error_past_the_preview_cap() {
    for size in SIZES {
        let mut fx = fixture();
        fx.ctx.store.config.artifacts.retention.larger_than = Some("1B".into());
        let log: String = (1..=2500)
            .map(|i| format!("line {i}\n"))
            .chain(["error: final failure\n".to_string()])
            .collect();
        add(&fx, "long.log", &log, ArtifactKind::Log, "Long gate log");
        corral_ops::artifact_gc(&fx.ctx, false).unwrap();
        let stored = fx.ctx.store.get(&fx.id).unwrap();
        assert!(
            stored
                .artifacts
                .iter()
                .any(|a| a.compressed_bytes.is_some()),
            "log is compressed on disk"
        );
        let mut app = finder(&fx);
        app.handle_key(code(KeyCode::Tab));
        typed(&mut app, "kind:log");
        let top = draw(&mut app, size);
        assert!(
            top.contains("line 1") && !top.contains("final failure"),
            "{size:?}\n{top}"
        );
        press(&mut app, size, "[^e] error");
        let at = draw(&mut app, size);
        assert!(at.contains("error: final failure"), "{size:?}\n{at}");
    }
}

#[test]
fn compressed_and_purged_previews_render_as_text_not_bytes() {
    let mut fx = fixture();
    fx.ctx.store.config.artifacts.retention.larger_than = Some("1B".into());
    fx.ctx.store.config.artifacts.retention.action = corral_core::RetentionAction::Compress;
    add(
        &fx,
        "notes.md",
        &format!(
            "# Packed notes\n\n- one\n- two\n{}",
            "filler line\n".repeat(300)
        ),
        ArtifactKind::Report,
        "Packed report",
    );
    fx.ctx
        .store
        .update(&fx.id, |t| {
            for a in t
                .artifacts
                .iter_mut()
                .filter(|a| a.title == "Packed report")
            {
                a.tier_override = Some(ArtifactTier::Normal);
            }
            Ok(())
        })
        .unwrap();
    corral_ops::artifact_gc(&fx.ctx, false).unwrap();
    assert!(
        fx.ctx
            .store
            .get(&fx.id)
            .unwrap()
            .artifacts
            .iter()
            .any(|a| a.title == "Packed report" && a.compressed_bytes.is_some())
    );
    for size in SIZES {
        let mut app = finder(&fx);
        typed(&mut app, "kind:report packed");
        let screen = draw(&mut app, size);
        assert!(
            screen.contains("Packed notes") && screen.contains("• one"),
            "{size:?}\n{screen}"
        );
        assert!(
            !screen.contains("binary") && !screen.contains("\u{1f}"),
            "{size:?}\n{screen}"
        );
    }
    fx.ctx.store.config.artifacts.retention.action = corral_core::RetentionAction::Purge;
    fx.ctx
        .store
        .update(&fx.id, |t| {
            for a in &mut t.artifacts {
                a.tier_override = Some(ArtifactTier::Normal);
            }
            Ok(())
        })
        .unwrap();
    corral_ops::artifact_gc(&fx.ctx, false).unwrap();
    let mut app = finder(&fx);
    typed(&mut app, "packed");
    let purged = draw(&mut app, (160, 45));
    assert!(purged.contains("Artifact content purged"), "{purged}");
    assert!(!purged.contains("Packed notes"), "{purged}");
}

#[test]
fn narrow_rows_keep_the_tier_status_and_selected_excerpt_visible() {
    let fx = fixture();
    add(
        &fx,
        "long.md",
        "# Verdict: ship it\n\nbody\n",
        ArtifactKind::Report,
        "A very long descriptive review report title that cannot fit",
    );
    for size in SIZES {
        let mut app = finder(&fx);
        typed(&mut app, "long descriptive");
        let screen = draw(&mut app, size);
        assert!(screen.contains("keep"), "{size:?}: tier badge\n{screen}");
        assert!(
            screen.contains(" new"),
            "{size:?}: unreviewed status\n{screen}"
        );
        assert!(
            screen.contains("↳ Verdict: ship it"),
            "{size:?}: summary line\n{screen}"
        );
        assert!(
            screen.contains("…"),
            "{size:?}: long titles are cut with an ellipsis\n{screen}"
        );
    }
}

#[test]
fn review_opens_the_lead_artifact_then_lists_by_importance_and_marks_reviewed_by_click() {
    for size in SIZES {
        let fx = rich();
        let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
        app.apply(Action::OpenReview(fx.id.clone()));
        let open = draw(&mut app, size);
        assert!(
            open.contains("report · QA checklist") && open.contains("all green"),
            "{size:?}\n{open}"
        );
        press(&mut app, size, "×");
        app.handle_key(code(KeyCode::Tab));
        let list = draw(&mut app, size);
        assert!(list.contains("QA checklist"), "{size:?}\n{list}");
        if size.0 >= 160 {
            assert!(list.contains("2 to review"), "{list}");
            let at = |n: &str| list.find(n).unwrap_or_else(|| panic!("{n}\n{list}"));
            assert!(at("QA checklist") < at("Release summary"), "{list}");
            assert!(at("Release summary") < at("Run config"), "{list}");
            assert!(at("Run config") < at("raw (2)"), "{list}");
            assert!(
                !list.contains("Final diff"),
                "raw items stay folded\n{list}"
            );
        }
        assert!(corral_ops::reviewed_set(&fx.ctx).is_empty());
        press(&mut app, size, "v reviewed");
        assert_eq!(
            corral_ops::reviewed_set(&fx.ctx).len(),
            1,
            "{size:?}: the footer hint is the v key"
        );
    }
}

#[test]
fn detail_folds_raw_artifacts_by_key_and_click_and_walks_keep_items() {
    for size in SIZES {
        let fx = rich();
        let open_detail = || {
            let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
            app.apply(Action::OpenDetail(fx.id.clone()));
            app.handle_key(key('3'));
            app
        };
        let (mut by_key, mut by_click) = (open_detail(), open_detail());
        let folded = draw(&mut by_key, size);
        assert!(
            folded.contains("▸ raw (2)") && !folded.contains("Final diff"),
            "{size:?}\n{folded}"
        );
        by_key.handle_key(key('f'));
        press_row(&mut by_click, size, "▸ raw (2)");
        let (a, b) = (draw(&mut by_key, size), draw(&mut by_click, size));
        assert_eq!(a, b, "{size:?}: key and click render differently");
        assert!(
            a.contains("▾ raw (2)") && a.contains("Final diff"),
            "{size:?}\n{a}"
        );
    }

    let fx = rich();
    let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    app.apply(Action::OpenDetail(fx.id.clone()));
    app.handle_key(key('3'));
    press(&mut app, (160, 45), "v reviewed");
    assert_eq!(corral_ops::reviewed_set(&fx.ctx).len(), 1);
    press(&mut app, (160, 45), "n next important");
    press(&mut app, (160, 45), "v reviewed");
    assert_eq!(
        corral_ops::reviewed_set(&fx.ctx).len(),
        2,
        "n walks to the next unreviewed keep item"
    );
    let screen = draw(&mut app, (160, 45));
    assert!(screen.contains("✓"), "{screen}");
}

#[test]
fn review_cards_show_unreviewed_keep_artifacts_until_marked_reviewed() {
    for size in SIZES {
        let fx = fixture();
        let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
        let unreviewed = draw(&mut app, size);
        assert!(unreviewed.contains("● 1 new"), "{size:?}\n{unreviewed}");

        let marked = fixture();
        let report = marked
            .ctx
            .store
            .get(&marked.id)
            .unwrap()
            .artifacts
            .remove(0);
        corral_ops::toggle_reviewed(&marked.ctx, &report).unwrap();
        let mut reviewed = App::new(marked.ctx.clone(), Mode::Board).unwrap();
        let after = draw(&mut reviewed, size);
        assert!(!after.contains("● 1 new"), "{size:?}\n{after}");
        assert_ne!(
            unreviewed, after,
            "{size:?}: the card differs once reviewed"
        );

        app.apply(Action::OpenReview(fx.id.clone()));
        draw(&mut app, size);
        press(&mut app, size, "×");
        app.handle_key(code(KeyCode::Tab));
        press(&mut app, size, "v reviewed");
        app.handle_key(code(KeyCode::Esc));
        let back = draw(&mut app, size);
        assert!(
            !back.contains("● 1 new"),
            "{size:?}: returning from review refreshes the card\n{back}"
        );
    }
}
