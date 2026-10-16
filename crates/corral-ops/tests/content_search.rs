use std::fs;
use std::path::PathBuf;

use corral_core::{
    Artifact, ArtifactKind, ArtifactTier, PinRef, RetentionAction, Run, Status, Store,
};
use corral_ops::{
    ContentHit, ContentSearch, Ctx, SearchData, add_artifact, artifact_gc, search_content,
};
use serde_json::{Value, json};

struct Fixture {
    tmp: tempfile::TempDir,
    ctx: Ctx,
    task: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let task = store
            .create("Investigation", Some("app"), Status::Done, "")
            .unwrap()
            .id;
        Self {
            tmp,
            ctx: corral_ops::resources::FixedResourceProbe::ctx(store),
            task,
        }
    }

    fn add(&self, name: &str, text: &str, kind: ArtifactKind, title: &str) -> Artifact {
        let file = self.tmp.path().join(name);
        fs::write(&file, text).unwrap();
        add_artifact(&self.ctx, &self.task, &file, Some(kind), Some(title)).unwrap()
    }

    fn search(&self, query: &str) -> Vec<ContentHit> {
        search_content(
            &self.ctx,
            query,
            &ContentSearch {
                scope: "artifacts".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn index_path(&self) -> PathBuf {
        self.ctx.store.state_dir().join("content-search.json")
    }

    fn cache(&self) -> Value {
        serde_json::from_slice(&fs::read(self.index_path()).unwrap()).unwrap()
    }

    fn retention(&mut self, action: RetentionAction) {
        self.ctx.store.config.artifacts.retention.action = action;
        self.ctx.store.config.artifacts.retention.larger_than = Some("1B".into());
    }
}

#[test]
fn compressed_hit_reuses_write_time_text_and_reindex_streams_canonical_gzip() {
    // Given indexed text followed by gzip retention.
    let mut f = Fixture::new();
    let artifact = f.add(
        "gate.log",
        &format!("{}\nerror: stale review\n", "padding\n".repeat(200)),
        ArtifactKind::Log,
        "Gate evidence",
    );
    let reference = PinRef::of_artifact(&f.task, &artifact).to_string();
    assert!(
        f.cache()["documents"].get(&reference).is_some(),
        "indexed on add"
    );
    f.retention(RetentionAction::Compress);
    artifact_gc(&f.ctx, false).unwrap();
    let current = f.ctx.store.get(&f.task).unwrap().artifacts.remove(0);
    assert!(current.compressed_bytes.is_some());
    let snapshot = f.ctx.store.dir().join(&current.path);
    let gzip = fs::read(&snapshot).unwrap();

    // When searching both the warm index and a rebuilt compressed source.
    fs::write(&snapshot, b"deliberately unavailable decoder input").unwrap();
    let warm = f.search("error:\"stale review\"");
    fs::write(&snapshot, gzip).unwrap();
    let rebuilt = search_content(
        &f.ctx,
        "error:\"stale review\"",
        &ContentSearch {
            scope: "artifacts".into(),
            reindex: true,
            ..Default::default()
        },
    )
    .unwrap();

    // Then cached search never needs the archive, and reindex retains the same source identity.
    for hits in [&warm, &rebuilt] {
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].artifact_ref.as_deref(), Some(reference.as_str()));
        assert_eq!(hits[0].task, f.task);
        assert_eq!(hits[0].run.as_deref(), Some("R0"));
        assert_eq!(hits[0].kind, "log");
        assert_eq!(hits[0].tier, ArtifactTier::Transient);
        assert_eq!(hits[0].line_start, 202);
        assert!(hits[0].snippet.contains("stale review"));
    }
}

#[test]
fn overflow_gzip_is_searchable_after_the_prefix_and_across_chunk_boundaries() {
    // Given a large line with a phrase crossing a 64 KiB chunk, after the cache prefix.
    let mut f = Fixture::new();
    let prefix = "padding\n".repeat(40_000);
    let line = format!("error: {}stale review tail\n", "x".repeat(65_536 - 11));
    f.add(
        "large.log",
        &format!("{prefix}{line}"),
        ArtifactKind::Log,
        "Large",
    );
    f.retention(RetentionAction::Compress);
    artifact_gc(&f.ctx, false).unwrap();

    // When searching beyond the cached prefix, with both positive and negative structured terms.
    let hits = search_content(
        &f.ctx,
        "error:\"stale review\" -test:unrelated",
        &ContentSearch {
            scope: "artifacts".into(),
            context: 1,
            ..Default::default()
        },
    )
    .unwrap();

    // Then streaming finds the source line and keeps the phrase in a bounded snippet.
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line_start, 40_000);
    assert_eq!(hits[0].line_end, 40_001);
    assert_eq!(hits[0].snippet.lines().count(), 2);
    assert!(hits[0].snippet.contains("stale review"));
    assert!(hits[0].snippet.chars().count() <= 1200);
    assert!(f.search("error:\"stale review\" -error:tail").is_empty());
}

#[test]
fn unicode_case_insensitive_phrases_survive_source_chunk_boundaries() {
    // Given a phrase whose source bytes are longer than its case-folded query.
    let f = Fixture::new();
    let query = "k".repeat(4096);
    f.add(
        "unicode.log",
        &format!("{}{}\n", "x".repeat(65_536 - 6000), "K".repeat(4096)),
        ArtifactKind::Log,
        "Unicode boundary",
    );

    // When a case-insensitive phrase spans two source chunks.
    let hits = f.search(&query);

    // Then matching retains enough original bytes, not just the shorter folded bytes.
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].line_start, hits[0].line_end), (1, 1));
    assert!(hits[0].snippet.chars().count() <= 1200);
}

#[test]
fn snippets_keep_the_hit_with_context_and_cap_unicode_and_control_characters() {
    // Given a long matching line and long surrounding lines.
    let f = Fixture::new();
    let surrounding = format!("{}\n", "界".repeat(1800));
    let body = format!(
        "{}error: {}needle\u{1b}[31m\n{}",
        surrounding.repeat(20),
        "K".repeat(5000),
        surrounding.repeat(20)
    );
    f.add("context.log", &body, ArtifactKind::Log, "Context");

    // When requesting the maximum surrounding context.
    let hits = search_content(
        &f.ctx,
        "needle",
        &ContentSearch {
            scope: "artifacts".into(),
            context: 20,
            ..Default::default()
        },
    )
    .unwrap();

    // Then context cannot crowd the hit out or exceed the character budget.
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].line_start, hits[0].line_end), (1, 41));
    assert!(hits[0].snippet.contains("needle"));
    assert!(hits[0].snippet.chars().count() <= 1200);
    assert!(!hits[0].snippet.contains('\u{1b}'));
}

#[test]
fn relevance_boosts_title_heading_verdict_failure_and_error_lines() {
    // Given equal-frequency matches in each ranked field.
    let f = Fixture::new();
    f.add(
        "plain.txt",
        "needle ordinary",
        ArtifactKind::Evidence,
        "Plain",
    );
    f.add(
        "error.txt",
        "error: needle",
        ArtifactKind::Evidence,
        "Error",
    );
    f.add(
        "test.txt",
        "test startup_cli::needle ... FAILED",
        ArtifactKind::Evidence,
        "Test",
    );
    f.add("heading.md", "# needle", ArtifactKind::Evidence, "Heading");
    f.add(
        "verdict.md",
        "Verdict: needle",
        ArtifactKind::Evidence,
        "Review",
    );
    f.add("title.txt", "unrelated", ArtifactKind::Evidence, "needle");

    // When ranking all matches.
    let hits = f.search("needle");

    // Then titles lead and every signal beats ordinary body text.
    assert_eq!(hits.len(), 6);
    assert_eq!(hits[0].title, "needle");
    assert_eq!(hits[5].title, "Plain");
    assert!(hits[1..5].iter().all(|h| h.score > hits[5].score));
    assert_eq!(f.search("test:startup_cli").len(), 1);
    assert_eq!(f.search("verdict:needle").len(), 1);
}

#[test]
fn equal_relevance_uses_tier_then_fixed_recency_and_filters_intersect() {
    // Given tied matches with explicit tiers and timestamps.
    let f = Fixture::new();
    for (name, tier, at) in [
        ("keep", ArtifactTier::Keep, "2025-01-01T00:00:00Z"),
        ("old", ArtifactTier::Normal, "2025-01-01T00:00:00Z"),
        ("new", ArtifactTier::Normal, "2026-01-01T00:00:00Z"),
    ] {
        let a = f.add(
            &format!("{name}.txt"),
            &format!("needle {name}"),
            ArtifactKind::Evidence,
            name,
        );
        f.ctx
            .store
            .update(&f.task, |t| {
                let artifact = t.artifacts.iter_mut().find(|x| x.path == a.path).unwrap();
                artifact.added = at.parse().unwrap();
                artifact.tier_override = Some(tier);
                Ok(())
            })
            .unwrap();
    }
    let options = ContentSearch {
        scope: "artifacts".into(),
        task: Some(f.task.clone()),
        project: Some("app".into()),
        kind: Some(ArtifactKind::Evidence),
        tier: Some(ArtifactTier::Normal),
        since: Some("2025-06-01".into()),
        limit: 1,
        ..Default::default()
    };

    // When sorting ties and intersecting explicit filters.
    let all = f.search("needle");
    let narrowed = search_content(&f.ctx, "needle", &options).unwrap();

    // Then tier wins first, recency breaks remaining ties, and only the intersection remains.
    assert_eq!(
        all.iter().map(|h| h.title.as_str()).collect::<Vec<_>>(),
        ["keep", "new", "old"]
    );
    assert_eq!(narrowed.len(), 1);
    assert_eq!(narrowed[0].title, "new");
}

#[test]
fn specifications_reviews_comments_change_requests_and_events_share_the_index() {
    // Given each durable non-artifact content source.
    let f = Fixture::new();
    f.ctx
        .store
        .update(&f.task, |t| {
            t.body = "specmarker".into();
            t.comments.push(
                serde_json::from_value(json!({
                    "author": "owner", "at": "2026-01-01T00:00:00Z", "text": "commentmarker"
                }))
                .unwrap(),
            );
            let run: Run = serde_json::from_value(json!({
                "id":"R1", "agent":"test", "worktree":f.tmp.path(), "branch":"corral/test",
                "base":"main", "base_sha":"abc", "started":"2026-01-01T00:00:00Z",
                "summary":"summarymarker",
                "review": {
                    "reviewer":"owner", "sha":"abc", "at":"2026-01-01T00:00:00Z",
                    "verdict":"changes", "notes":"reviewmarker"
                }
            }))
            .unwrap();
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    f.ctx
        .store
        .log_event(
            Some(&f.task),
            "observed",
            json!({
                "notes":"eventmarker", "run":"R1"
            }),
        )
        .unwrap();

    // When searching each scope and loading the board's shared snapshot.
    let mut results = Vec::new();
    for (query, scope) in [
        ("specmarker", "specs"),
        ("commentmarker", "reviews"),
        ("summarymarker", "reviews"),
        ("verdict:changes reviewmarker", "reviews"),
        ("eventmarker", "all"),
    ] {
        results.push(
            search_content(
                &f.ctx,
                query,
                &ContentSearch {
                    scope: scope.into(),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
    }
    let data = SearchData::load(&f.ctx.store).unwrap();

    // Then all source types participate and indexed content reaches the existing board finder.
    assert!(results.iter().all(|hits| hits.len() == 1));
    assert_eq!(results[4][0].scope, "events");
    assert_eq!(results[4][0].run.as_deref(), Some("R1"));
    assert!(data.extra_text[&f.task].contains("reviewmarker"));
}

#[test]
fn sync_indexes_spec_and_change_requests_without_searching_or_contacting_a_server() {
    // Given changes written after the previous content index snapshot.
    let mut f = Fixture::new();
    corral_ops::refresh_search_index(&f.ctx).unwrap();
    f.ctx
        .store
        .update(&f.task, |t| {
            t.body = "syncmarker".into();
            Ok(())
        })
        .unwrap();
    f.ctx
        .store
        .log_event(
            Some(&f.task),
            "changes_requested",
            json!({
                "text": "requestmarker", "run": "R0"
            }),
        )
        .unwrap();
    f.ctx.herdr = corral_herdr::Herdr::with_bin(f.tmp.path().join("absent-herdr"), None);

    // When sync runs against an explicitly unavailable, isolated client.
    corral_ops::sync_at(&f.ctx, "2026-10-16T00:00:00Z".parse().unwrap()).unwrap();

    // Then the index is updated before any search, including the reviews scope.
    assert_eq!(
        f.cache()["documents"][format!("{}:spec", f.task)]["text"],
        "syncmarker\n"
    );
    let hits = search_content(
        &f.ctx,
        "requestmarker",
        &ContentSearch {
            scope: "reviews".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, "changes_requested");
}

#[test]
fn aggregate_cache_budget_evicts_text_without_losing_search_results() {
    // Given more than the 8 MiB cache budget in real artifact snapshots.
    let f = Fixture::new();
    let padding = "padding text\n".repeat(22_000);
    let seed = f.add(
        "seed.log",
        &format!("{padding}overflowbudget\n"),
        ArtifactKind::Log,
        "Seed",
    );
    f.ctx
        .store
        .update(&f.task, |t| {
            for i in 0..40 {
                let mut artifact = seed.clone();
                artifact.path = format!("artifacts/{}/R0/budget-{i:02}.log", t.id);
                artifact.title = format!("Budget {i:02}");
                std::fs::hard_link(
                    f.ctx.store.dir().join(&seed.path),
                    f.ctx.store.dir().join(&artifact.path),
                )
                .unwrap();
                t.artifacts.push(artifact);
            }
            Ok(())
        })
        .unwrap();

    // When rebuilding the bounded index and searching every overflow source.
    let hits = search_content(
        &f.ctx,
        "overflowbudget",
        &ContentSearch {
            scope: "artifacts".into(),
            limit: 100,
            reindex: true,
            ..Default::default()
        },
    )
    .unwrap();

    // Then eviction bounds cache bytes rather than silently omitting old documents.
    assert_eq!(hits.len(), 41);
    let cache = f.cache();
    let documents = cache["documents"].as_object().unwrap();
    let text_bytes: usize = documents
        .values()
        .map(|d| d["text"].as_str().unwrap().len())
        .sum();
    assert!(text_bytes <= 8 * 1024 * 1024);
    assert!(documents.len() <= 4096);
    assert!(fs::metadata(f.index_path()).unwrap().len() <= 64 * 1024 * 1024);
}

#[test]
fn incremental_cache_replaces_changed_specs_drops_purged_content_and_recovers_corruption() {
    // Given an indexed spec and an artifact subsequently purged.
    let mut f = Fixture::new();
    f.ctx
        .store
        .update(&f.task, |t| {
            t.body = "oldmarker".into();
            Ok(())
        })
        .unwrap();
    f.add("purge.log", "purgedmarker\n", ArtifactKind::Log, "Purge");
    f.ctx
        .store
        .update(&f.task, |t| {
            t.body = "newmarker".into();
            Ok(())
        })
        .unwrap();
    f.retention(RetentionAction::Purge);
    artifact_gc(&f.ctx, false).unwrap();

    // When reading the updated index and searching after cache corruption.
    let updated = f.cache();
    fs::write(f.index_path(), b"{broken derived cache").unwrap();
    let hits = search_content(&f.ctx, "newmarker", &ContentSearch::default()).unwrap();

    // Then task changes and purge are incremental, corruption is rebuildable, and bytes are bounded.
    assert_eq!(
        updated["documents"][format!("{}:spec", f.task)]["text"],
        "newmarker\n"
    );
    assert!(
        updated["documents"]
            .as_object()
            .unwrap()
            .values()
            .all(|d| d["text"] != "purgedmarker\n")
    );
    assert_eq!(hits.len(), 1);
    assert!(f.search("purgedmarker").is_empty());
    assert!(fs::metadata(f.index_path()).unwrap().len() <= 64 * 1024 * 1024);
}

#[test]
fn invalid_queries_and_output_budgets_are_rejected_at_the_boundary() {
    // Given an isolated search store.
    let f = Fixture::new();

    // When callers exceed each bound or use an unknown field.
    let cases = [
        ("unknown:value".to_string(), ContentSearch::default()),
        ("x".repeat(4097), ContentSearch::default()),
        (
            "needle".into(),
            ContentSearch {
                context: 21,
                ..Default::default()
            },
        ),
        (
            "needle".into(),
            ContentSearch {
                limit: 101,
                ..Default::default()
            },
        ),
        ("needle ".repeat(65), ContentSearch::default()),
    ];
    let errors: Vec<_> = cases
        .iter()
        .map(|(q, options)| search_content(&f.ctx, q, options).is_err())
        .collect();

    // Then no invalid request silently ignores a limit or returns partial matches.
    assert!(errors.into_iter().all(|error| error));
}
