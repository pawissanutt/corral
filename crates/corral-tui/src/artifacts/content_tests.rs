use corral_core::{Artifact, ArtifactKind, ArtifactTier, Status};
use ratatui::crossterm::event::KeyCode;

use super::panels_fixture::{Fixture, fixture, key, render};
use super::{ArtifactsView, SORT_KEY, TIER_KEY};
use crate::{Action, View};

fn type_query(view: &mut ArtifactsView, fx: &Fixture, query: &str) {
    while !view.query.is_empty() {
        assert_eq!(view.on_key(key(KeyCode::Backspace), &fx.ctx), Action::None);
    }
    for c in query.chars() {
        assert_eq!(view.on_key(key(KeyCode::Char(c)), &fx.ctx), Action::None);
    }
}

fn add(fx: &Fixture, task: &str, title: &str, kind: ArtifactKind, body: &str) -> Artifact {
    let source = fx.root.join("finder-source.txt");
    std::fs::write(&source, body).unwrap();
    corral_ops::add_artifact(&fx.ctx, task, &source, Some(kind), Some(title)).unwrap()
}

fn titles(view: &ArtifactsView) -> Vec<&str> {
    view.matches
        .iter()
        .map(|&i| view.rows[i].artifact.title.as_str())
        .collect()
}

#[test]
fn finder_matches_body_only_terms_and_mixed_metadata_via_keys() {
    // Given a term absent from every metadata field.
    let fx = fixture();
    let artifact = add(&fx, &fx.id, "Notes", ArtifactKind::Report, "obsidian\n");
    let mut view = ArtifactsView::new(&fx.ctx, None).unwrap();

    // When the actual finder receives metadata and body terms by key input.
    type_query(&mut view, &fx, "app obsidian");

    // Then it selects and renders the matching artifact, not an unrelated report.
    assert_eq!(titles(&view), ["Notes"]);
    assert_eq!(view.selected().unwrap().artifact.path, artifact.path);
    assert!(render(&mut view, &fx.ctx).contains("obsidian"));
    for width in [80, 120] {
        let (screen, _) = super::panels_fixture::render_at(&mut view, &fx.ctx, width, 24);
        assert!(screen.contains("results 1") && screen.contains("Notes"));
        assert!(screen.contains("obsidian"));
        println!("finder {width}x24:\n{screen}");
    }
}

#[test]
fn finder_uses_cached_gzip_body_after_snapshot_becomes_unavailable() {
    // Given a compressed snapshot already loaded into the shared content cache.
    let mut fx = fixture();
    fx.ctx.store.config.artifacts.retention.larger_than = Some("1B".into());
    let artifact = add(
        &fx,
        &fx.id,
        "Raw output",
        ArtifactKind::Log,
        &"obsidian output\n".repeat(100),
    );
    corral_ops::artifact_gc(&fx.ctx, false).unwrap();
    let stored = fx.ctx.store.get(&fx.id).unwrap();
    assert!(
        stored
            .artifacts
            .iter()
            .find(|a| a.path == artifact.path)
            .unwrap()
            .compressed_bytes
            .is_some()
    );
    let mut view = ArtifactsView::new(&fx.ctx, None).unwrap();
    std::fs::remove_file(fx.ctx.store.dir().join(&artifact.path)).unwrap();

    // When typing and backspacing cannot reread or decompress that snapshot.
    view.on_key(key(KeyCode::Tab), &fx.ctx);
    type_query(&mut view, &fx, "obsidian");
    view.on_key(key(KeyCode::Backspace), &fx.ctx);
    view.on_key(key(KeyCode::Char('n')), &fx.ctx);

    // Then cached body matching still finds it.
    assert_eq!(titles(&view), ["Raw output"]);
}

#[test]
fn finder_intersects_body_task_multiple_kinds_and_tiers_via_keys() {
    // Given matching bodies across tasks, kinds and tiers.
    let fx = fixture();
    add(
        &fx,
        &fx.id,
        "Report",
        ArtifactKind::Report,
        "obsidian report",
    );
    add(
        &fx,
        &fx.id,
        "Summary",
        ArtifactKind::Summary,
        "obsidian summary",
    );
    add(&fx, &fx.id, "Raw", ArtifactKind::Log, "obsidian raw");
    add(
        &fx,
        &fx.id,
        "Other kind",
        ArtifactKind::Diff,
        "obsidian diff",
    );
    let other = fx
        .ctx
        .store
        .create("Other task", None, Status::Backlog, "")
        .unwrap();
    add(
        &fx,
        &other.id,
        "Other task",
        ArtifactKind::Report,
        "obsidian other",
    );
    fx.ctx
        .store
        .update(&fx.id, |task| {
            for artifact in &mut task.artifacts {
                artifact.added = match artifact.title.as_str() {
                    "Report" => "2026-01-01T12:01:00Z",
                    "Summary" => "2026-01-01T12:02:00Z",
                    _ => "2026-01-01T12:03:00Z",
                }
                .parse()
                .unwrap();
                if artifact.title == "Report" {
                    artifact.tier_override = Some(ArtifactTier::Keep);
                } else if artifact.title == "Summary" {
                    artifact.tier_override = Some(ArtifactTier::Normal);
                }
            }
            Ok(())
        })
        .unwrap();
    let mut view = ArtifactsView::new(&fx.ctx, Some(&fx.id)).unwrap();

    // When text and multiple kind/tier filters are typed into the task-restricted finder.
    type_query(
        &mut view,
        &fx,
        "obsidian kind:report kind:summary tier:keep tier:normal",
    );

    // Then kinds and tiers are unions within their groups and intersect the body and task.
    assert_eq!(titles(&view), ["Report", "Summary"]);
    view.on_key(SORT_KEY, &fx.ctx);
    assert_eq!(titles(&view), ["Summary", "Report"]);
    type_query(&mut view, &fx, "obsidian kind:report kind:summary");
    view.on_key(TIER_KEY, &fx.ctx);
    assert_eq!(titles(&view), ["Report"]);
    type_query(
        &mut view,
        &fx,
        "obsidian kind:report kind:summary tier:normal",
    );
    assert_eq!(
        titles(&view),
        ["Summary"],
        "typed tiers override the tier cycle"
    );
}

#[test]
fn finder_keeps_body_matched_raw_rows_folded_until_tab() {
    // Given an important artifact and raw output sharing only a body term.
    let fx = fixture();
    add(
        &fx,
        &fx.id,
        "Report",
        ArtifactKind::Report,
        "obsidian report",
    );
    add(&fx, &fx.id, "Raw", ArtifactKind::Log, "obsidian raw");
    let mut view = ArtifactsView::new(&fx.ctx, None).unwrap();

    // When body text is entered with the default important fold.
    type_query(&mut view, &fx, "obsidian");

    // Then matching raw rows are counted and Tab reveals them without losing the filter.
    assert_eq!(titles(&view), ["Report"]);
    assert_eq!(view.hidden, 1);
    view.on_key(key(KeyCode::Tab), &fx.ctx);
    assert_eq!(view.matches.len(), 2);
    assert_eq!(view.hidden, 0);
    view.on_key(key(KeyCode::Tab), &fx.ctx);
    assert_eq!(titles(&view), ["Report"]);
    assert_eq!(view.hidden, 1);
}

#[test]
fn finder_refresh_reloads_content_preserves_query_and_drops_purged_rows() {
    // Given an open filtered finder and a newly attached body match.
    let fx = fixture();
    let original = add(
        &fx,
        &fx.id,
        "Original",
        ArtifactKind::Report,
        "obsidian original",
    );
    let mut view = ArtifactsView::new(&fx.ctx, Some(&fx.id)).unwrap();
    type_query(&mut view, &fx, "obsidian");
    add(&fx, &fx.id, "New", ArtifactKind::Report, "obsidian new");
    assert_eq!(titles(&view), ["Original"]);

    // When the view refreshes from the shared snapshot.
    view.refresh(&fx.ctx);

    // Then new content is searchable, and the existing query and selection survive.
    assert_eq!(view.query, "obsidian");
    assert_eq!(view.matches.len(), 2);
    assert_eq!(view.selected().unwrap().artifact.path, original.path);

    // Given a subsequent purge under the same snapshot path.
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.artifacts
                .iter_mut()
                .find(|a| a.path == original.path)
                .unwrap()
                .purged_at = Some("2026-01-02T00:00:00Z".parse().unwrap());
            Ok(())
        })
        .unwrap();

    // When the finder refreshes again, purged content cannot remain a cached match.
    view.refresh(&fx.ctx);
    assert_eq!(titles(&view), ["New"]);
}

#[test]
fn finder_searches_gzip_overflow_beyond_cached_prefix() {
    // Given compressed output larger than the shared per-document extraction limit.
    let mut fx = fixture();
    fx.ctx.store.config.artifacts.retention.larger_than = Some("1B".into());
    let body = format!("{}obsidian-tail\n", "ordinary output\n".repeat(20_000));
    let artifact = add(&fx, &fx.id, "Large output", ArtifactKind::Log, &body);
    corral_ops::artifact_gc(&fx.ctx, false).unwrap();
    let mut view = ArtifactsView::new(&fx.ctx, None).unwrap();
    assert!(
        view.rows
            .iter()
            .find(|r| r.artifact.path == artifact.path)
            .unwrap()
            .artifact
            .compressed_bytes
            .is_some()
    );

    // When a body-only term past the cached prefix is entered through real key handling.
    type_query(&mut view, &fx, "tier:transient obsidian-tail");

    // Then bounded streaming search finds the overflow without loading the whole archive.
    assert_eq!(titles(&view), ["Large output"]);
}

#[test]
fn finder_does_not_keep_old_results_when_overflow_content_cannot_be_read() {
    // Given visible metadata matches and an overflow snapshot removed after load.
    let fx = fixture();
    let artifact = add(
        &fx,
        &fx.id,
        "Output",
        ArtifactKind::Evidence,
        &"ordinary output\n".repeat(20_000),
    );
    let mut view = ArtifactsView::new(&fx.ctx, None).unwrap();
    type_query(&mut view, &fx, "app");
    assert!(!view.matches.is_empty());
    std::fs::remove_file(fx.ctx.store.dir().join(&artifact.path)).unwrap();

    // When another key requires content that is no longer available.
    let action = view.on_key(key(KeyCode::Char('z')), &fx.ctx);

    // Then the read error is reported without presenting stale matching rows.
    assert!(matches!(action, Action::Toast(_)));
    assert!(view.matches.is_empty());
    assert_eq!(view.hidden, 0);
}
