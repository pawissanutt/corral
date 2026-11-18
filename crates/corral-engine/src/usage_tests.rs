use crate::{
    Engine, Role, Store, Usage,
    tests::{run, task, tempdir},
};

use super::{UsageBy, UsageSummary, UsageTotal};

fn request(at: &str) -> Usage {
    Usage {
        task: None,
        run: None,
        at: at.parse().unwrap(),
        profile: None,
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
    for id in ["T-001", "T-002", "T-003"] {
        engine.store.insert_task(&task(id)).unwrap();
        let mut fixture = run(id, "R1");
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
