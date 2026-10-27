use corral_core::attention::{
    AttentionFact, AttentionRecord, ConditionKey, NoticeClaim, NoticeCoverage, NoticeOutcome,
    Responsible, SinceSource,
};
use corral_core::{Event, Status, Store, Task};
use serde_json::json;

use super::*;

fn t(seconds: i64) -> Timestamp {
    "2099-01-01T00:00:00Z"
        .parse::<Timestamp>()
        .unwrap()
        .checked_add(SignedDuration::from_secs(seconds))
        .unwrap()
}

fn window(start: i64, end: i64) -> Window {
    Window {
        start: t(start),
        end: t(end),
    }
}

fn event(kind: &str, task: Option<&str>, detail: Value) -> Event {
    serde_json::from_value(json!({"ts": t(0), "task": task, "kind": kind, "detail": detail}))
        .unwrap()
}

fn owner(kind: &str, task: &str, detail: Value) -> Event {
    serde_json::from_value(
        json!({"ts": t(0), "task": task, "kind": kind, "actor": "owner", "detail": detail}),
    )
    .unwrap()
}

fn opened(key: &str, responsible: &str, since: i64, live: Value) -> Event {
    let kind = key.split('/').nth(1).unwrap();
    event(
        "attention_opened",
        key.split('/').next(),
        json!({"key": key, "kind": kind, "responsible": responsible, "since": t(since),
            "opened_at": t(since), "pm_live": live, "wake": responsible == "pm",
            "fact_id": format!("{key}/attention_opened")}),
    )
}

fn resolved(key: &str, at: i64) -> Event {
    event(
        "attention_resolved",
        key.split('/').next(),
        json!({"key": key, "resolved_at": t(at), "fact_id": format!("{key}/attention_resolved")}),
    )
}

fn task() -> Task {
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path())
        .unwrap()
        .create("Metrics", None, Status::Review, "")
        .unwrap()
}

const PRODUCERS: [&str; 17] = [
    "obligation_transition",
    "provider_exhausted",
    "resource_escalation",
    "provider_wait",
    "post_land_blocked",
    "infra_hold",
    "autopilot_escalate",
    "landing_blocked",
    "legacy_review_fallback",
    "legacy_review_watchdog",
    "native_review_watchdog",
    "availability_exhausted",
    "native_review_host_lost",
    "needs_me_entry",
    "stuck",
    "question_needs_me",
    "attention_ladder",
];

fn claim(producer: &str, index: usize, outcome: Option<NoticeOutcome>) -> NoticeClaim {
    let batch_id = format!("batch-{index}");
    let coverage = match producer {
        "provider_wait" | "post_land_blocked" | "resource_escalation" => Vec::new(),
        "needs_me_entry" | "question_needs_me" => vec![
            NoticeCoverage::Episode {
                task: "T-001".into(),
                run: None,
                stage: "needs_me".into(),
                at: t(0),
            },
            NoticeCoverage::Exact {
                condition_key: format!("T-001/question_open/Q{index}@1"),
            },
        ],
        _ => vec![NoticeCoverage::Exact {
            condition_key: format!("T-001/{producer}/{index}"),
        }],
    };
    NoticeClaim {
        version: 1,
        batch_id: batch_id.clone(),
        producer: producer.into(),
        source: format!("source-{index}"),
        claimed_at: t(100),
        claim_ids: vec![format!("{batch_id}/a"), format!("{batch_id}/b")]
            [..if producer == "attention_ladder" { 2 } else { 1 }]
            .to_vec(),
        coverage,
        exemption_basis: (producer == "needs_me_entry").then(|| "owner_responsible_entry".into()),
        outcome,
        pending: Vec::new(),
    }
}

fn published(claim: &NoticeClaim) -> Vec<Event> {
    let detail = |claim: &NoticeClaim| {
        json!({"batch_id": claim.batch_id, "producer": claim.producer, "source": claim.source,
            "claimed_at": claim.claimed_at, "claim_ids": claim.claim_ids,
            "coverage": claim.coverage, "exemption_basis": claim.exemption_basis})
    };
    let mut detail_claimed = detail(claim);
    detail_claimed["fact_id"] = json!(format!("{}/claimed", claim.batch_id));
    let mut out = vec![event("notification_claimed", Some("T-001"), detail_claimed)];
    let kind = match &claim.outcome {
        Some(NoticeOutcome::Notified { .. }) => "owner_notified",
        Some(NoticeOutcome::Failed { .. }) => "owner_notify_failed",
        Some(NoticeOutcome::NotInvoked { .. }) => "notification_not_invoked",
        None => return out,
    };
    let mut outcome = detail(claim);
    outcome["fact_id"] = json!(format!("{}/outcome", claim.batch_id));
    out.push(event(kind, Some("T-001"), outcome));
    out
}

#[test]
fn nearest_rank_percentiles_are_exact() {
    assert_eq!(percentile(&[], 50), None);
    assert_eq!(percentile(&[7], 95), Some(7));
    let values: Vec<i64> = (1..=20).collect();
    assert_eq!(percentile(&values, 50), Some(10));
    assert_eq!(percentile(&values, 95), Some(19));
}

#[test]
fn every_producer_counts_once_across_published_pruned_and_retained_claims() {
    // Given each production producer in a fixed outcome state, split between pruned
    // (events only), retained (record only) and both (replayed after restart).
    let outcomes = [
        Some(NoticeOutcome::Notified { at: t(100) }),
        Some(NoticeOutcome::Failed {
            at: t(100),
            error: "transport".into(),
        }),
        None,
        Some(NoticeOutcome::NotInvoked {
            at: t(100),
            reason: "disabled".into(),
        }),
    ];
    let mut events = Vec::new();
    let mut retained = Vec::new();
    for (index, producer) in PRODUCERS.iter().enumerate() {
        let claim = claim(producer, index, outcomes[index % 4].clone());
        match index % 3 {
            0 => events.extend(published(&claim)),
            1 => retained.push(claim),
            _ => {
                events.extend(published(&claim));
                events.extend(published(&claim));
                retained.push(claim);
            }
        }
    }
    // When stats are computed from the union.
    let stats = compute(&[], &events, &retained, window(0, 200));
    // Then each physical batch counts once, by producer, with uncertain outcomes kept apart.
    let n = &stats.notifications;
    assert_eq!(n.batches, 17);
    assert_eq!(
        n.claims, 18,
        "attention_ladder covers two claims in one batch"
    );
    assert_eq!(
        (
            n.success,
            n.failure,
            n.outcome_unknown,
            n.certainly_not_invoked
        ),
        (5, 4, 4, 4)
    );
    assert_eq!(n.calls, [9, 13]);
    assert_eq!(n.outside_ladder, 3);
    assert_eq!(n.exempt_owner_entry, 1);
    assert_eq!(n.mixed, 2);
    assert_eq!(n.budget_batches, 16);
    for producer in PRODUCERS {
        assert_eq!(n.by_producer[producer].batches, 1, "{producer}");
    }
    assert_eq!(n.by_producer["attention_ladder"].claims, 2);
}

#[test]
fn populations_carry_in_cohorts_censoring_and_hourly_backlog_are_exact() {
    // Given a carried-in PM condition, a PM-absent one, a handled-open one and an owner one.
    let carried = "T-001/unexpected_stall/a";
    let absent = "T-001/agent_lost/b";
    let handled = "T-001/question_open/Q1@1";
    let owner_key = "T-001/escalation/c";
    let events = vec![
        opened(carried, "pm", -1800, json!(true)),
        opened(absent, "pm", 600, Value::Null),
        opened(handled, "pm", 1800, json!(true)),
        opened(owner_key, "owner", 0, json!(true)),
        resolved(carried, 3600),
        resolved(owner_key, 900),
        event(
            "attention_handled",
            Some("T-001"),
            json!({"key": handled, "handling": {"at": t(2400), "by": "owner"},
                "fact_id": format!("{handled}/attention_handled/1")}),
        ),
    ];
    // When a 0..7200 window is read.
    let stats = compute(&[], &events, &[], window(0, 7200));
    // Then the population, cohorts, latencies and samples match the enumeration.
    let p = &stats.population;
    assert_eq!(
        (p.total, p.carry_in, p.opened, p.resolved, p.open_at_end),
        (4, 1, 3, 2, 2)
    );
    assert_eq!((p.pm_responsible, p.owner_responsible), (3, 1));
    assert_eq!((p.pm_available, p.pm_absent), (3, 1));
    assert_eq!((stats.handling.observed, stats.handling.censored), (2, 0));
    assert_eq!(stats.handling.p50_secs, Some(600));
    assert_eq!(stats.handling.p95_secs, Some(5400));
    assert_eq!(stats.handling_pm_absent.censored, 1);
    assert_eq!(
        (stats.resolution.observed, stats.resolution.censored),
        (2, 2)
    );
    assert_eq!(stats.backlog.samples, 2);
    assert_eq!(stats.backlog.peak_open, 2);
    assert_eq!(stats.backlog.end_open, 2);
    assert_eq!(stats.backlog.end_max_secs, Some(6600));
    assert_eq!(stats.backlog.peak_max_secs, Some(6600));
    assert_eq!(
        stats.actions.owner_rescues, 1,
        "owner attend on a PM condition"
    );
}

#[test]
fn retained_ledger_facts_and_records_merge_with_appended_facts_by_identity() {
    // Given a record whose opening was appended, then replayed, while its re-wake is pending.
    let key = ConditionKey("T-001/unexpected_stall/a".into());
    let mut task = task();
    task.id = "T-001".into();
    task.attention.records.insert(
        key.clone(),
        AttentionRecord {
            kind: "unexpected_stall".into(),
            responsible: Responsible::Pm,
            since: t(0),
            since_source: SinceSource::Canonical,
            first_recorded: t(0),
            opened_at: t(0),
            pm_live_at_open: Some(true),
            levels: Default::default(),
            handling: None,
            first_handled_at: None,
            resolved_at: None,
        },
    );
    task.attention.outbox.push(AttentionFact {
        key: key.clone(),
        fact_id: format!("{key}/attention_overdue/rewake"),
        kind: "attention_overdue".into(),
        detail: json!({"key": key, "level": "rewake", "wake": true, "at": t(900)}),
    });
    let opening = opened(&key.0, "pm", 0, json!(true));
    // When stats read both sources.
    let stats = compute(&[task], &[opening.clone(), opening], &[], window(0, 900));
    // Then one condition carries both wakes and stays censored.
    assert_eq!(stats.population.total, 1);
    assert_eq!(stats.conditions[&key.0].wakes, 2);
    assert_eq!(stats.handling.censored, 1);
}

#[test]
fn action_joins_count_duplicates_terminal_repeats_and_ambiguity() {
    // Given two open delivery conditions and one open question on one task.
    let first = "T-001/delivery_failed/T-001_R1#1";
    let second = "T-001/delivery_failed/T-001_R1#2";
    let question = "T-001/question_open/T-001/R1/Q1@1";
    let mut events = vec![
        opened(first, "pm", 0, json!(true)),
        opened(second, "pm", 0, json!(true)),
        opened(question, "pm", 0, json!(true)),
    ];
    // When the PM answers twice while unresolved, retries one message, replies after a
    // failed reply, and posts a fresh message.
    events.extend([
        event(
            "question_answered",
            Some("T-001"),
            json!({"key": "T-001/R1/Q1"}),
        ),
        event(
            "question_answered",
            Some("T-001"),
            json!({"key": "T-001/R1/Q1"}),
        ),
        event(
            "message_posted",
            Some("T-001"),
            json!({"id": "T-001_R1#3", "key": "T-001_R1"}),
        ),
        event(
            "message_retried",
            Some("T-001"),
            json!({"from": "T-001_R1#1", "to": "T-001_R1#3"}),
        ),
        owner(
            "message_posted",
            "T-001",
            json!({"id": "T-001_R1#4", "key": "T-001_R1"}),
        ),
        resolved(question, 60),
    ]);
    let stats = compute(&[], &events, &[], window(0, 60));
    // Then each identity join is exact and the fresh post is ambiguous.
    let a = &stats.actions;
    assert_eq!(a.by_condition.get(question), Some(&2));
    assert_eq!(a.by_condition.get(first), Some(&1));
    assert_eq!(a.by_condition.get(second), None);
    assert_eq!((a.attributed, a.ambiguous, a.duplicates), (3, 1, 1));
    assert_eq!(
        a.owner_rescues, 0,
        "an ambiguous owner post rescues nothing"
    );
    let stall = vec![
        opened("T-001/unexpected_stall/a", "pm", 0, json!(true)),
        event(
            "replied",
            Some("T-001"),
            json!({"run": "R1", "submitted": false}),
        ),
        event("replied", Some("T-001"), json!({"run": "R1"})),
    ];
    let stats = compute(&[], &stall, &[], window(0, 60));
    assert_eq!(
        (stats.actions.after_terminal, stats.actions.duplicates),
        (1, 0)
    );
}
