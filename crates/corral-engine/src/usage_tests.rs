use crate::{
    Engine, Role, Store, Usage,
    tests::{run, task, tempdir},
};

use super::{UsageBy, UsageSummary, UsageTotal, UsageWindow};

fn request(at: &str) -> Usage {
    Usage {
        task: None,
        run: None,
        at: at.parse().unwrap(),
        profile: None,
        harness: None,
        model: "model-a".into(),
        input: 10,
        output: 2,
        cache_read: 3,
        cache_write: 4,
        usd: None,
        billing: None,
    }
}

#[test]
fn usage_round_trips_without_creating_an_event() {
    let root = tempdir();
    let engine = Engine::init(root.path(), None).unwrap();
    assert_eq!(engine.usage_summary(None).unwrap(), UsageSummary::default());
    let associated = Usage {
        task: Some("T-001".into()),
        run: Some("R1".into()),
        profile: Some("worker".into()),
        harness: Some("reported".into()),
        usd: Some(0.125),
        billing: Some("api".into()),
        ..request("2026-11-18T00:00:00.012345678Z")
    };
    engine.store.insert_task(&task("T-001")).unwrap();
    engine.store.insert_run(&run("T-001", "R1")).unwrap();
    engine.record_usage(&associated).unwrap();
    let unassociated = Usage {
        input: u64::MAX,
        billing: Some("subscription".into()),
        ..request("2026-11-17T10:00:00Z")
    };
    engine.record_usage(&unassociated).unwrap();
    drop(engine);

    let store = Store::open(&root.path().join(".corral-next/corral.db")).unwrap();
    assert_eq!(store.list_usage().unwrap(), [associated, unassociated]);
    assert!(
        store
            .events_after(&Default::default(), 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn usage_rollups_keep_unassociated_requests_but_count_only_exact_run_matches() {
    let root = tempdir();
    let engine = Engine::init(root.path(), None).unwrap();
    for (id, profile, model, started) in [
        ("T-001", "fast", "model-a", "2026-11-17T21:00:00Z"),
        ("T-002", "slow", "model-b", "2026-11-18T00:00:00Z"),
        ("T-003", "fast", "model-a", "2026-11-18T00:00:00Z"),
    ] {
        engine.store.insert_task(&task(id)).unwrap();
        let mut fixture = run(id, "R1");
        fixture.profile = profile.into();
        fixture.model = Some(model.into());
        fixture.started = started.parse().unwrap();
        if id == "T-002" {
            fixture.role = Role::Reviewer;
            fixture.ended = Some(fixture.started);
        }
        engine.store.insert_run(&fixture).unwrap();
    }

    let first = Usage {
        task: Some("T-001".into()),
        run: Some("R1".into()),
        profile: Some("fast".into()),
        usd: Some(0.125),
        ..request("2026-11-17T23:15:00Z")
    };
    let later_same_run = Usage {
        at: "2026-11-18T01:30:00+02:00".parse().unwrap(),
        usd: Some(0.0),
        ..first.clone()
    };
    let other_run = Usage {
        task: Some("T-002".into()),
        run: Some("R1".into()),
        profile: Some("slow".into()),
        model: "model-b".into(),
        usd: Some(0.25),
        ..request("2026-11-18T00:00:00Z")
    };
    let run_only = Usage {
        run: Some("R1".into()),
        ..request("2026-11-18T00:00:00Z")
    };
    let mismatched = Usage {
        task: Some("T-003".into()),
        run: Some("R2".into()),
        ..request("2026-11-18T00:00:00Z")
    };
    for row in [first, later_same_run, other_run, run_only, mismatched] {
        engine.record_usage(&row).unwrap();
    }

    let ungrouped = engine.usage_summary(None).unwrap();
    assert_eq!(
        ungrouped,
        UsageSummary {
            total: UsageTotal {
                requests: 5,
                input: 50,
                output: 10,
                cache_read: 15,
                cache_write: 20,
                priced_requests: 3,
                usd: Some(0.375),
            },
            groups: vec![],
            covered_runs: 2,
            total_runs: 3,
        }
    );

    let summary = |by| {
        let summary = engine.usage_summary(Some(by)).unwrap();
        assert_eq!(summary.total, ungrouped.total);
        assert_eq!(summary.covered_runs, ungrouped.covered_runs);
        assert_eq!(summary.total_runs, ungrouped.total_runs);
        summary.groups
    };
    let group_requests = |by| {
        summary(by)
            .into_iter()
            .map(|group| (group.key, group.total.requests))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        group_requests(UsageBy::Profile),
        [
            (None, 2),
            (Some("fast".to_owned()), 2),
            (Some("slow".to_owned()), 1),
        ]
    );
    assert_eq!(
        group_requests(UsageBy::Model),
        [
            (Some("model-a".to_owned()), 4),
            (Some("model-b".to_owned()), 1)
        ]
    );
    assert_eq!(
        group_requests(UsageBy::Task),
        [
            (None, 1),
            (Some("T-001".to_owned()), 2),
            (Some("T-002".to_owned()), 1),
            (Some("T-003".to_owned()), 1),
        ]
    );
    assert_eq!(
        group_requests(UsageBy::Day),
        [
            (Some("2026-11-17".to_owned()), 2),
            (Some("2026-11-18".to_owned()), 3),
        ]
    );
    let by_profile = summary(UsageBy::Profile);
    assert_eq!(
        by_profile[0].total,
        UsageTotal {
            requests: 2,
            input: 20,
            output: 4,
            cache_read: 6,
            cache_write: 8,
            priced_requests: 0,
            usd: None,
        }
    );
    assert_eq!(by_profile[1].total.priced_requests, 2);
    assert_eq!(by_profile[1].total.usd, Some(0.125));
}

#[test]
fn group_coverage_counts_only_distinct_runs_in_the_request_group() {
    let root = tempdir();
    let engine = Engine::init(root.path(), None).unwrap();
    let day_one = "2026-11-17T22:00:00Z";
    let day_two = "2026-11-18T01:00:00Z";
    let day_three = "2026-11-19T01:00:00Z";
    for id in ["T-001", "T-002", "T-003"] {
        engine.store.insert_task(&task(id)).unwrap();
    }
    for (task, id, profile, model, harness, started) in [
        ("T-001", "R1", "fast", Some("model-a"), "pi", day_one),
        ("T-001", "R2", "slow", Some("model-b"), "omp", day_two),
        ("T-002", "R1", "fast", None, "generic", day_one),
        ("T-003", "R1", "silent", Some("model-c"), "omp", day_three),
    ] {
        let mut fixture = run(task, id);
        fixture.profile = profile.into();
        fixture.model = model.map(str::to_owned);
        fixture.harness = harness.into();
        fixture.started = started.parse().unwrap();
        engine.store.insert_run(&fixture).unwrap();
    }
    let matching = Usage {
        task: Some("T-001".into()),
        run: Some("R1".into()),
        profile: Some("fast".into()),
        ..request("2026-11-18T01:30:00+02:00")
    };
    let requests = [
        Usage {
            at: day_two.parse().unwrap(),
            profile: Some("slow".into()),
            harness: Some("reported".into()),
            model: "model-b".into(),
            ..matching.clone()
        },
        matching.clone(),
        matching,
        Usage {
            task: Some("T-002".into()),
            run: Some("R1".into()),
            model: "model-b".into(),
            ..request(day_two)
        },
        Usage {
            task: Some("T-002".into()),
            run: Some("R2".into()),
            profile: Some("slow".into()),
            model: "model-b".into(),
            ..request(day_two)
        },
        Usage {
            task: Some("T-001".into()),
            ..request(day_one)
        },
        Usage {
            run: Some("R1".into()),
            model: "model-d".into(),
            ..request(day_two)
        },
    ];
    for row in &requests {
        engine.record_usage(row).unwrap();
    }
    let ungrouped = engine.usage_summary(None).unwrap();
    assert_eq!(ungrouped.total.requests, requests.len() as u64);
    assert_eq!((ungrouped.covered_runs, ungrouped.total_runs), (2, 4));
    assert!(ungrouped.groups.is_empty());
    let counts = |by| {
        let grouped = engine.usage_summary(Some(by)).unwrap();
        assert_eq!(grouped.total, ungrouped.total);
        grouped
            .groups
            .into_iter()
            .map(|group| {
                (
                    group.key,
                    group.covered_runs.unwrap(),
                    group.total_runs.unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        counts(UsageBy::Profile),
        [
            (None, 0, 0),
            (Some("fast".into()), 1, 2),
            (Some("silent".into()), 0, 1),
            (Some("slow".into()), 0, 1),
        ]
    );
    assert_eq!(
        counts(UsageBy::Model),
        [
            (None, 0, 1),
            (Some("model-a".into()), 1, 1),
            (Some("model-b".into()), 0, 1),
            (Some("model-c".into()), 0, 1),
            (Some("model-d".into()), 0, 0),
        ]
    );
    assert_eq!(
        counts(UsageBy::Task),
        [
            (None, 0, 0),
            (Some("T-001".into()), 1, 2),
            (Some("T-002".into()), 1, 1),
            (Some("T-003".into()), 0, 1),
        ]
    );
    assert_eq!(
        counts(UsageBy::Day),
        [
            (Some("2026-11-17".into()), 1, 2),
            (Some("2026-11-18".into()), 0, 1),
            (Some("2026-11-19".into()), 0, 1),
        ]
    );

    let enriched = engine.store.list_usage().unwrap();
    assert_eq!(
        enriched
            .iter()
            .map(|row| row.harness.as_deref())
            .collect::<Vec<_>>(),
        [
            Some("reported"),
            Some("pi"),
            Some("pi"),
            Some("generic"),
            None,
            None,
            None,
        ]
    );
    assert_eq!(
        enriched
            .into_iter()
            .zip(&requests)
            .map(|(mut row, request)| {
                row.harness.clone_from(&request.harness);
                row
            })
            .collect::<Vec<_>>(),
        requests
    );
}

#[test]
fn usage_series_buckets_by_local_midnight_and_covers_only_same_day_cohorts() {
    let root = tempdir();
    let engine = Engine::init(root.path(), None).unwrap();
    engine.store.insert_task(&task("T-001")).unwrap();
    let from = "2026-11-16T22:00:00Z";
    let next_day = "2026-11-17T22:00:00Z";
    let before = "2026-11-16T21:59:59.999999999Z";
    let to = "2026-11-20T22:00:00Z";
    for (id, started, model) in [
        ("prior", before, "model-a"),
        ("matching", from, "model-a"),
        ("switched", next_day, "configured"),
        ("silent", "2026-11-18T22:00:00Z", "model-a"),
        ("later", to, "model-a"),
    ] {
        let mut run = run("T-001", id);
        run.started = started.parse().unwrap();
        run.model = Some(model.into());
        engine.store.insert_run(&run).unwrap();
    }
    for (id, at) in [
        ("prior", before),
        ("prior", from),
        ("matching", from),
        ("matching", "2026-11-17T21:59:59.999999999Z"),
        ("matching", next_day),
        ("switched", next_day),
        ("later", to),
    ] {
        engine
            .record_usage(&Usage {
                task: Some("T-001".into()),
                run: Some(id.into()),
                ..request(at)
            })
            .unwrap();
    }
    let window = || UsageWindow::parse("2026-11-17", "2026-11-21", 120).unwrap();
    let series = engine.usage_series(window(), Some(UsageBy::Model)).unwrap();
    let counts = |day: usize| {
        let summary = &series[day].summary;
        (
            summary.total.requests,
            summary.covered_runs,
            summary.total_runs,
        )
    };
    let groups = |day: usize| {
        series[day]
            .summary
            .groups
            .iter()
            .map(|group| {
                (
                    group.key.as_deref(),
                    group.total.requests,
                    group.covered_runs,
                    group.total_runs,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        series
            .iter()
            .map(|day| day.day.as_str())
            .collect::<Vec<_>>(),
        ["2026-11-17", "2026-11-18", "2026-11-19", "2026-11-20"]
    );
    assert_eq!(counts(0), (3, 1, 1));
    assert_eq!(groups(0), [(Some("model-a"), 3, Some(1), Some(1))]);
    assert_eq!(counts(1), (2, 1, 1));
    assert_eq!(
        groups(1),
        [
            (Some("configured"), 0, Some(0), Some(1)),
            (Some("model-a"), 2, Some(0), Some(0)),
        ]
    );
    assert_eq!(counts(2), (0, 0, 1));
    assert_eq!(groups(2), [(Some("model-a"), 0, Some(0), Some(1))]);
    assert_eq!(series[3].summary, UsageSummary::default());

    let by_day = engine.usage_series(window(), Some(UsageBy::Day)).unwrap();
    for day in &by_day[..3] {
        assert_eq!(day.summary.groups[0].key.as_deref(), Some(day.day.as_str()));
        assert_eq!(
            day.summary.groups[0].covered_runs,
            Some(day.summary.covered_runs)
        );
    }
}

#[test]
fn usage_window_requires_canonical_dates_and_a_bounded_forward_span() {
    assert!(UsageWindow::parse("2024-01-01", "2025-01-01", -240).is_ok());
    for (from, to, offset) in [
        ("2024-01-01", "2025-01-02", 0),
        ("2024-01-01", "2024-01-01", 0),
        ("2024-01-02", "2024-01-01", 0),
        ("2024-02-30", "2024-03-01", 0),
        ("2024-01-01T00:00:00Z", "2024-01-02", 0),
        ("2024-01-01", "2024-01-02", 1440),
        ("2024-01-01", "2024-01-02", -1440),
    ] {
        assert!(UsageWindow::parse(from, to, offset).is_err());
    }
}
