use super::*;
use crate::{Artifact, CommentAuthor, Run, TaskComment, Usage, UsageEntry};
use serde_json::json;

fn now() -> Timestamp {
    "2026-10-15T12:00:00Z".parse().unwrap()
}

fn task() -> Task {
    serde_json::from_value(json!({
        "id": "T-047", "title": "Board task search", "status": "idea",
        "project": "Corral", "kind": "feature", "profile": "preferred",
        "created": "2026-10-01T00:00:00Z", "updated": "2026-10-10T12:00:00Z",
        "rounds": 2
    }))
    .unwrap()
}

fn run() -> Run {
    serde_json::from_value(json!({
        "id": "R1", "agent": "legacy-profile", "profile": "fast",
        "model": "vendor/strong-model", "harness": "omo",
        "worktree": "/not-read", "branch": "corral/T-047", "base": "main",
        "base_sha": "abc123", "started": "2026-10-09T00:00:00Z",
        "summary": "Recovered the kestrel"
    }))
    .unwrap()
}

fn artifact() -> Artifact {
    serde_json::from_value(json!({
        "run": "R1", "kind": "report", "title": "Quasar evidence",
        "path": "missing-report.md", "source": "/not-read", "sha256": "abc",
        "bytes": 4, "added": "2026-10-10T00:00:00Z"
    }))
    .unwrap()
}

fn matches(input: &str, task: &Task) -> bool {
    Query::parse(input, now())
        .unwrap()
        .score(task, SearchContext::default())
        .is_some()
}

#[test]
fn fields_and_negation_match_task_metadata() {
    // Given all queryable metadata, including a run and an artifact.
    let mut t = task();
    t.runs.push(run());
    t.artifacts.push(artifact());

    // When structured queries include positive and negated predicates.
    let cases = [
        ("status:idea project:cor kind:feat", true),
        ("status:done", false),
        ("status:dropped", false),
        ("profile:preferred", true),
        ("profile:fast model:strong harness:omo", true),
        ("profile:legacy-profile", false),
        ("id:T-04* has:artifact rounds>1", true),
        ("id:T-04", false),
        ("-status:done -project:other", true),
        ("-status:idea", false),
        ("-has:artifact", false),
        ("rounds>2", false),
        ("kestrel", true),
        ("quasar", true),
        ("-quasar", false),
    ];
    let results: Vec<_> = cases.iter().map(|(q, _)| matches(q, &t)).collect();

    // Then every predicate has the specified effect.
    assert_eq!(results, cases.map(|(_, expected)| expected));
}

#[test]
fn every_status_is_queryable() {
    // Given tasks at each lifecycle state.
    let statuses = [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
        Status::Dropped,
    ];

    // When querying each exact status.
    let results: Vec<_> = statuses
        .iter()
        .map(|&status| {
            let mut t = task();
            t.status = status;
            matches(&format!("status:{}", status.as_str()), &t)
        })
        .collect();

    // Then terminal states and ideas match just like active tasks.
    assert!(results.into_iter().all(|matched| matched));
}

#[test]
fn parser_rejects_unknown_fields_and_malformed_values() {
    // Given unsupported fields and incomplete or invalid field values.
    let cases = [
        ("wat:yes", "unknown query field"),
        ("cost:2", "unknown query field"),
        ("nonsense>2", "unknown query field"),
        ("status:missing", "unknown status"),
        ("project:", "needs a value"),
        ("has:report", "expected has:artifact"),
        ("cost>NaN", "finite"),
        ("cost>-1", "nonnegative"),
        ("rounds>1.2", "integer"),
        ("tokens>-4", "integer"),
        ("since:tomorrow", "invalid query date"),
        ("before:99999999999999999999d", "invalid query date"),
        ("since:-1d", "invalid query date"),
        ("\"unfinished", "unclosed quote"),
        ("unfinished\\", "unclosed quote"),
        ("\"\"", "empty quoted"),
        ("-", "missing query term"),
    ];

    // When parsing them.
    let errors: Vec<_> = cases
        .iter()
        .map(|(input, _)| format!("{:#}", Query::parse(input, now()).unwrap_err()))
        .collect();

    // Then each error explains the rejected input class.
    for ((input, expected), error) in cases.iter().zip(errors) {
        assert!(error.contains(expected), "{input}: {error}");
    }
}

#[test]
fn quoted_values_escapes_and_unicode_search_work() {
    // Given text with punctuation, whitespace and non-ASCII letters.
    let mut t = task();
    t.title = "Café's search".into();
    t.project = Some("two words".into());
    t.body = "https://example \"quoted\"".into();

    // When parsing quoted literals and field values.
    let results: Vec<_> = [
        "project:\"two words\"",
        "\"Café's search\"",
        "CAFÉ",
        "\"https://example\"",
        "\\\"quoted\\\"",
        "-\"missing phrase\"",
    ]
    .map(|input| matches(input, &t))
    .into();

    // Then quoting preserves values and case-insensitive text is Unicode aware.
    assert!(results.into_iter().all(|matched| matched));
}

#[test]
fn relative_and_absolute_dates_use_updated_time_with_fixed_clock() {
    // Given a task updated five days before the injected clock.
    let t = task();
    let cases = [
        ("since:7d", true),
        ("since:1w", true),
        ("since:120h", true),
        ("since:4d", false),
        ("before:4d", true),
        ("before:5d", false),
        ("since:2026-10-10", true),
        ("before:2026-10-10", false),
        ("since:2026-10-10T12:00:00Z", true),
        ("before:2026-10-10T12:00:00Z", false),
    ];

    // When evaluating boundaries and relative durations.
    let results: Vec<_> = cases.iter().map(|(q, _)| matches(q, &t)).collect();

    // Then since is inclusive, before exclusive, independent of wall time.
    assert_eq!(results, cases.map(|(_, expected)| expected));
}

#[test]
fn cost_and_tokens_include_all_runs_and_reviews() {
    // Given reported spend, estimated spend and unpriced tokens across reviews.
    let mut t = task();
    let mut r = run();
    r.usage = Some(Usage::from_entries(vec![UsageEntry {
        reported_usd: Some(1.5),
        input: 100,
        ..UsageEntry::default()
    }]));
    let mut review = run();
    review.usage = Some(Usage::from_entries(vec![
        UsageEntry {
            estimated_usd: Some(1.0),
            input: 50,
            ..UsageEntry::default()
        },
        UsageEntry {
            input: 25,
            ..UsageEntry::default()
        },
    ]));
    r.review_runs.push(review);
    t.runs.push(r);
    let cases = [
        ("cost>2 tokens>174", true),
        ("cost>2.5", false),
        ("tokens>175", false),
        ("-cost>3", true),
    ];

    // When applying strict numeric comparisons.
    let results: Vec<_> = cases.iter().map(|(q, _)| matches(q, &t)).collect();

    // Then spend counts reported plus estimate-only, and token totals include reviews.
    assert_eq!(results, cases.map(|(_, expected)| expected));
}

#[test]
fn relevance_orders_exact_titles_substrings_summaries_artifacts_body_and_fuzzy() {
    // Given the same word in distinct fields or as a fuzzy title match.
    let mut title = task();
    title.title = "search".into();
    let mut substring = task();
    substring.title = "Search the board".into();
    let mut summary = task();
    summary.title = "other".into();
    let mut r = run();
    r.summary = Some("search".into());
    summary.runs.push(r);
    let mut artifact_task = task();
    artifact_task.title = "other".into();
    let mut a = artifact();
    a.title = "search".into();
    artifact_task.artifacts.push(a);
    let mut body = task();
    body.title = "other".into();
    body.body = "search".into();
    let mut fuzzy = task();
    fuzzy.title = "s_e_a_r_c_h".into();
    let query = Query::parse("search", now()).unwrap();

    // When scoring the tasks.
    let scores: Vec<_> = [title, substring, summary, artifact_task, body, fuzzy]
        .iter()
        .map(|t| query.score(t, SearchContext::default()).unwrap())
        .collect();

    // Then direct content matches outrank word subsequences, with deterministic weights.
    assert!(scores.windows(2).all(|w| w[0] > w[1]), "{scores:?}");
}

#[test]
fn supplemental_text_supports_pins_and_legacy_history_sha() {
    // Given store-owned landed history metadata.
    let t = task();
    let context = SearchContext {
        pinned: true,
        extra_text: "abc1234 fast_forward ff",
    };

    // When querying text, a pin and a history sha together.
    let score = Query::parse("pinned sha:abc ff", now())
        .unwrap()
        .score(&t, context);

    // Then all terms match without opening files or reading a live session.
    assert!(score.is_some());
}

#[test]
fn comment_only_text_matches_and_negates_across_task_statuses() {
    // Given durable owner/PM comments whose words appear nowhere else.
    for status in [Status::Idea, Status::Done, Status::Dropped] {
        let mut t = task();
        t.status = status;
        t.comments = vec![
            TaskComment {
                author: CommentAuthor::Owner,
                at: now(),
                text: "zqxcommentonly047".into(),
            },
            TaskComment {
                author: CommentAuthor::Pm,
                at: now(),
                text: "pmcontext047".into(),
            },
        ];
        let mut title = task();
        title.title = "zqxcommentonly047".into();
        let query = Query::parse("zqxcommentonly047", now()).unwrap();

        // When scoring positive, conjunctive, fuzzy and negated comment text.
        let score = query.score(&t, SearchContext::default());
        let conjunction = matches("ZQXCOMMENTONLY047 pmcontext047", &t);
        let fuzzy = matches("zqxcmntonly047", &t);
        let negated = matches("-zqxcommentonly047", &t);

        // Then comments participate in the shared semantics, below title relevance.
        assert!(score.is_some(), "{status:?}: comment-only text was missed");
        assert!(score < query.score(&title, SearchContext::default()));
        assert!(conjunction && fuzzy, "{status:?}");
        assert!(!negated, "{status:?}: negated comment text was ignored");
    }
}

#[test]
fn id_globs_are_anchored_and_support_multiple_stars() {
    // Given exact and wildcard task-id patterns.
    let t = task();
    let cases = [
        ("id:T-047", true),
        ("id:t-*7", true),
        ("id:*04*", true),
        ("id:*", true),
        ("id:T**0*7", true),
        ("id:T-04", false),
        ("id:047", false),
        ("id:T-047*7", false),
    ];

    // When evaluating each pattern.
    let results: Vec<_> = cases.iter().map(|(q, _)| matches(q, &t)).collect();

    // Then only '*' expands and the whole id must match.
    assert_eq!(results, cases.map(|(_, expected)| expected));
}

#[test]
fn empty_queries_and_and_terms_have_defined_behavior() {
    // Given a task whose body spans unrelated words.
    let mut t = task();
    t.body = "a b c d e f".into();

    // When combining text terms and evaluating an empty query.
    let empty = Query::parse("   ", now())
        .unwrap()
        .score(&t, SearchContext::default());
    let conjunction = matches("board absentword", &t);
    let scattered = matches("abcdef", &t);

    // Then the empty query matches with zero score, and every term must match.
    assert_eq!(empty, Some(0));
    assert!(!conjunction);
    assert!(!scattered);
}
