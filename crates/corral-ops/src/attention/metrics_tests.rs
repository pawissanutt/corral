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
            "coverage": claim.coverage, "exemption_basis": claim.exemption_basis,
            "outcome": claim.outcome})
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

fn at(mut event: Event, seconds: i64) -> Event {
    event.ts = t(seconds);
    event
}

#[test]
fn action_joins_require_exact_question_revision_and_message_identities() {
    // Given two open delivery conditions and one open revision-2 question on one task.
    let first = "T-001/delivery_failed/T-001_R1#1";
    let second = "T-001/delivery_failed/T-001_R1#2";
    let question = "T-001/question_open/T-001/R1/Q1@2";
    let mut events = vec![
        opened(first, "pm", 0, json!(true)),
        opened(second, "pm", 0, json!(true)),
        opened(question, "pm", 0, json!(true)),
    ];
    // When answers arrive for revisions 1 and 2, one message is retried by ID, and a fresh
    // owner post reaches the recipient.
    let answer = |revision: u64| {
        event(
            "question_answered",
            Some("T-001"),
            json!({"key": "T-001/R1/Q1", "revision": revision}),
        )
    };
    events.extend([
        answer(1),
        answer(2),
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
    // Then only the matching revision and message join; the fresh post is ambiguous.
    let a = &stats.actions;
    assert_eq!(a.by_condition.get(question), Some(&1));
    assert_eq!(a.by_condition.get(first), Some(&1));
    assert_eq!(a.by_condition.get(second), None);
    assert_eq!((a.attributed, a.ambiguous, a.unattributed), (2, 1, 1));
    assert_eq!(
        a.owner_rescues, 0,
        "an ambiguous owner post rescues nothing"
    );
}

#[test]
fn run_level_joins_match_the_condition_run_and_include_review_reruns() {
    // Given a lost R1 agent on one task.
    let lost = "T-001/agent_lost/R1:2099-01-01T00:00:00Z";
    let run = |kind: &str, run: &str| event(kind, Some("T-001"), json!({"run": run}));
    let events = vec![
        opened(lost, "pm", 0, json!(true)),
        run("replied", "R2"),
        run("review_started", "R1"),
        run("dispatched", "R2"),
    ];
    // When actions name R2 and R1, then only the R1 review rerun joins.
    let stats = compute(&[], &events, &[], window(0, 60));
    assert_eq!(stats.conditions[lost].run.as_deref(), Some("R1"));
    assert_eq!(stats.actions.by_condition.get(lost), Some(&1));
    assert_eq!(
        (stats.actions.attributed, stats.actions.unattributed),
        (1, 2)
    );
}

#[test]
fn repeats_are_duplicates_only_while_the_earlier_effect_is_outstanding() {
    // Given an R1 condition and production-shaped inbox change requests to that run.
    let lost = "T-001/agent_lost/R1:2099-01-01T00:00:00Z";
    let change = |seq: u64| {
        event(
            "changes_requested",
            Some("T-001"),
            json!({"message": format!("T-001_R1#{seq}"), "submitted": false, "via": "inbox"}),
        )
    };
    let case = |between: Option<Event>| {
        let mut events = vec![opened(lost, "pm", 0, json!(true)), change(1)];
        events.extend(between);
        events.push(change(2));
        compute(&[], &events, &[], window(0, 60)).actions
    };
    // When nothing settles the first request, then the repeat is a duplicate.
    let outstanding = case(None);
    assert_eq!((outstanding.duplicates, outstanding.after_terminal), (1, 0));
    // When the first was observed, then the repeat is a legitimate new action.
    let observed = case(Some(event(
        "message_observed",
        Some("T-001"),
        json!({"id": "T-001_R1#1"}),
    )));
    assert_eq!((observed.duplicates, observed.after_terminal), (0, 0));
    // When the first failed terminally, then the repeat is reported after-terminal.
    let failed = case(Some(event(
        "message_failed",
        Some("T-001"),
        json!({"key": "T-001_R1", "seq": 1, "reason": "rejected"}),
    )));
    assert_eq!((failed.duplicates, failed.after_terminal), (0, 1));
}

#[test]
fn window_activity_respects_both_bounds_while_history_rebuilds_carry_in() {
    // Given a carried-in PM condition announced at 0 and re-woken at 900, then activity
    // after the window ends.
    let lost = "T-001/agent_lost/R1:2099-01-01T00:00:00Z";
    let events = vec![
        opened(lost, "pm", 0, json!(true)),
        event(
            "attention_overdue",
            Some("T-001"),
            json!({"key": lost, "level": "rewake", "wake": true, "at": t(900),
                "fact_id": format!("{lost}/attention_overdue/rewake")}),
        ),
        at(event("replied", Some("T-001"), json!({"run": "R1"})), 8000),
        at(
            event(
                "pm_event_delivery",
                Some("T-001"),
                json!({"submitted": true, "event_kind": "needs_me", "cursor": {"offset": 1}}),
            ),
            8000,
        ),
        event(
            "question_answered",
            Some("T-001"),
            json!({"key": "Q", "revision": 1, "at": t(9000)}),
        ),
    ];
    // When the 3600..7200 window is read.
    let stats = compute(&[], &events, &[], window(3600, 7200));
    // Then the condition and its lifetime wakes remain, but no window activity is counted.
    let row = &stats.conditions[lost];
    assert!(row.carry_in);
    assert_eq!((row.wakes, row.lifetime_wakes), (0, 2));
    assert_eq!(stats.wakes.attention_levels, 0);
    assert_eq!(stats.wakes.max_levels_per_condition, 2);
    assert_eq!(stats.wakes.raw_push.submitted, 0);
    assert_eq!(stats.actions.attributed + stats.actions.unattributed, 0);
}

#[test]
fn carry_in_batches_count_identically_whether_retained_or_pruned() {
    // Given a condition open across the window and three batches claimed before it.
    let key = "T-001/agent_lost/R1:x";
    let outcomes = [
        Some(NoticeOutcome::Notified { at: t(100) }),
        Some(NoticeOutcome::Failed {
            at: t(100),
            error: "transport".into(),
        }),
        None,
    ];
    let claims: Vec<_> = (outcomes.into_iter().enumerate())
        .map(|(index, outcome)| {
            let mut claim = claim("attention_ladder", index, outcome);
            claim.coverage = vec![NoticeCoverage::Exact {
                condition_key: key.into(),
            }];
            claim
        })
        .collect();
    let mut events = vec![opened(key, "pm", 0, json!(true)), resolved(key, 5000)];
    events.extend(claims.iter().flat_map(published));
    // When stats read the window before and after the claim records are pruned.
    let retained = compute(&[], &events, &claims, window(3600, 7200)).notifications;
    let pruned = compute(&[], &events, &[], window(3600, 7200)).notifications;
    // Then both report the same carry-in batches, none charged to the window's calls.
    assert_eq!(retained, pruned);
    assert_eq!((pruned.carry_in, pruned.carry_in_calls), (3, [2, 3]));
    assert_eq!(
        (pruned.batches, pruned.calls, pruned.budget_calls),
        (0, [0, 0], [0, 0])
    );
}

#[test]
fn raw_question_pushes_resolve_their_source_event_by_cursor() {
    // Given two open questions on one task and a push receipt for the second's opening.
    let one = "T-001/question_open/T-001/R1/Q1@1";
    let two = "T-001/question_open/T-001/R1/Q2@1";
    let opening = event(
        "question_opened",
        Some("T-001"),
        json!({"key": "T-001/R1/Q2", "revision": 1}),
    );
    let receipt = |offset: u64| {
        event(
            "pm_event_delivery",
            Some("T-001"),
            json!({"submitted": true, "event_kind": "question_opened", "pane": "pm:p1",
                "cursor": {"offset": offset}}),
        )
    };
    let events = vec![
        opened(one, "pm", 0, json!(true)),
        opening,
        opened(two, "pm", 0, json!(true)),
        receipt(200),
        resolved(two, 30),
        receipt(200),
    ];
    // When the receipt, and a delayed replay of it, name the source's cursor.
    let stats = compute_logged(
        &[],
        &events,
        &[10, 200, 300, 400, 500, 600],
        &[],
        window(0, 60),
    );
    // Then only that question is attributed, once, with the replay deduplicated.
    let raw = &stats.wakes.raw_push;
    assert_eq!(raw.attributed.get(two), Some(&1));
    assert_eq!(raw.attributed.get(one), None);
    assert_eq!((raw.submitted, raw.replayed, raw.ambiguous), (1, 1, 0));
    // When cursors are unavailable, then nothing is attributed by task alone.
    let blind = compute(&[], &events, &[], window(0, 60));
    assert!(blind.wakes.raw_push.attributed.is_empty());
}

fn runs(id: &str, started: &[(&str, i64)]) -> Task {
    let mut task = task();
    task.id = id.into();
    for (run, at) in started {
        task.runs.push(
            serde_json::from_value(json!({"id": run, "agent": "fixture", "worktree": "/w",
                "branch": "b", "base": "main", "base_sha": "abc", "started": t(*at)}))
            .unwrap(),
        );
    }
    task
}

#[test]
fn escalation_and_stall_runs_survive_resolution_pruning_and_newer_runs() {
    for kind in ["escalation", "infra_escalation", "unexpected_stall"] {
        // Given an R1 condition since 100, an R1 review rerun at 200 and its resolution.
        let key = format!("T-001/{kind}/{}", t(100));
        let review = at(
            event("review_started", Some("T-001"), json!({"run": "R1"})),
            200,
        );
        let opening = || {
            let mut opening = opened(&key, "pm", 100, json!(true));
            opening.detail["since"] = json!(t(100));
            opening
        };
        let resolved = resolved(&key, 300);
        for (case, tasks, events) in [
            (
                "open",
                vec![runs("T-001", &[("R1", 0)])],
                vec![opening(), review.clone()],
            ),
            (
                "pruned",
                vec![runs("T-001", &[("R1", 0)])],
                vec![opening(), review.clone(), resolved.clone()],
            ),
            (
                "newer run",
                vec![runs("T-001", &[("R1", 0), ("R2", 400)])],
                vec![opening(), review.clone(), resolved.clone()],
            ),
        ] {
            // When the read sees only canonical task runs and lifecycle facts.
            let stats = compute(&tasks, &events, &[], window(0, 500));
            // Then the historical run R1 still joins exactly that action.
            assert_eq!(
                stats.conditions[&key].run.as_deref(),
                Some("R1"),
                "{kind} {case}"
            );
            assert_eq!(
                stats.actions.by_condition.get(&key),
                Some(&1),
                "{kind} {case}"
            );
        }
        // And an R2 action never joins the R1 condition.
        let r2 = at(
            event("review_started", Some("T-001"), json!({"run": "R2"})),
            200,
        );
        let tasks = [runs("T-001", &[("R1", 0), ("R2", 400)])];
        let stats = compute(&tasks, &[opening(), r2], &[], window(0, 500));
        assert_eq!(stats.actions.attributed, 0, "{kind}");
    }
}

fn change(seq: u64, seconds: i64) -> Event {
    at(
        event(
            "changes_requested",
            Some("T-001"),
            json!({"message": format!("T-001_R1#{seq}"), "submitted": false}),
        ),
        seconds,
    )
}

#[test]
fn prior_effects_before_the_window_still_decide_in_window_repeats() {
    // Given an R1 condition opened at -100 and an unsettled request just before the window.
    let lost = "T-001/agent_lost/R1:x";
    let base = || vec![opened(lost, "pm", -100, json!(true)), change(1, -1)];
    let settle = |kind: &str, detail: Value| at(event(kind, Some("T-001"), detail), -1);
    for (case, between, duplicates, terminal) in [
        ("outstanding", None, 1, 0),
        (
            "observed",
            Some(settle("message_observed", json!({"id": "T-001_R1#1"}))),
            0,
            0,
        ),
        (
            "terminal",
            Some(settle(
                "message_failed",
                json!({"key": "T-001_R1", "seq": 1}),
            )),
            0,
            1,
        ),
    ] {
        let mut events = base();
        events.extend(between);
        events.push(change(2, 1));
        // When the 0..100 window holds only the repeat.
        let actions = compute(&[], &events, &[], window(0, 100)).actions;
        // Then only the repeat is charged, judged against the earlier effect.
        assert_eq!(actions.attributed, 1, "{case}");
        assert_eq!(
            (actions.duplicates, actions.after_terminal),
            (duplicates, terminal),
            "{case}"
        );
    }
    // And the window bounds are inclusive at both ends.
    let mut edges = vec![opened(lost, "pm", -100, json!(true))];
    edges.extend([change(1, 0), change(2, 100), change(3, 101)]);
    let actions = compute(&[], &edges, &[], window(0, 100)).actions;
    assert_eq!((actions.attributed, actions.duplicates), (2, 1));
}

#[test]
fn a_post_and_the_reply_naming_it_are_one_action() {
    // Given an open delivery condition and production-shaped post+reply pairs.
    let delivery = "T-001/delivery_failed/T-001_R1#1";
    let reply = |seq: u64| {
        [
            event(
                "message_posted",
                Some("T-001"),
                json!({"id": format!("T-001_R1#{seq}"), "key": "T-001_R1", "seq": seq}),
            ),
            owner(
                "replied",
                "T-001",
                json!({"run": "R1", "message": format!("T-001_R1#{seq}"), "submitted": false}),
            ),
        ]
    };
    let mut events = vec![opened(delivery, "pm", 0, json!(true))];
    events.extend(reply(2));
    // When one reply is read, then it counts once.
    let one = compute(&[], &events, &[], window(0, 60)).actions;
    assert_eq!((one.attributed, one.owner_rescues), (1, 1));
    // When a second reply follows while the first is unobserved, then two actions, one duplicate.
    events.extend(reply(3));
    let two = compute(&[], &events, &[], window(0, 60)).actions;
    assert_eq!(
        (two.attributed, two.duplicates, two.owner_rescues),
        (2, 1, 2)
    );
}

#[test]
fn membership_follows_canonical_times_not_publication_order() {
    // Given an answer canonically at 60 whose condition's opening (since 0) is published later.
    let question = "T-001/question_open/T-001/R1/Q1@1";
    let answer = |at: i64| {
        event(
            "question_answered",
            Some("T-001"),
            json!({"key": "T-001/R1/Q1", "revision": 1, "at": t(at)}),
        )
    };
    let delayed = vec![
        answer(60),
        opened(question, "pm", 0, json!(true)),
        resolved(question, 100),
    ];
    // When the opening was delayed by a crash, or is still only pending in the ledger.
    let published = compute(&[], &delayed, &[], window(0, 200)).actions;
    let mut pending = task();
    pending.id = "T-001".into();
    pending.attention.records.insert(
        ConditionKey(question.into()),
        AttentionRecord {
            kind: "question_open".into(),
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
    let unpublished = compute(&[pending], &[answer(60)], &[], window(0, 200)).actions;
    // Then the answer joins either way; an answer before since never joins.
    assert_eq!(published.by_condition.get(question), Some(&1));
    assert_eq!(unpublished.by_condition.get(question), Some(&1));
    let mut early = vec![opened(question, "pm", 100, json!(true))];
    early[0].detail["since"] = json!(t(100));
    early.push(answer(50));
    let early = compute(&[], &early, &[], window(0, 200)).actions;
    assert_eq!((early.attributed, early.unattributed), (0, 1));
    // And a retry naming a message is causally after it, even if a wall-clock step stamps the
    // retry before the condition's since; its resolution bound still applies.
    let failed = "T-001/delivery_failed/T-001_R1#1";
    let mut opening = opened(failed, "pm", 0, json!(true));
    opening.detail["since"] = json!(t(100));
    let retry = at(
        event(
            "message_retried",
            Some("T-001"),
            json!({"from": "T-001_R1#1", "to": "T-001_R1#2"}),
        ),
        98,
    );
    let stepped = compute(&[], &[opening.clone(), retry.clone()], &[], window(0, 200)).actions;
    assert_eq!(stepped.by_condition.get(failed), Some(&1));
    let late = at(retry, 150);
    let resolved = resolved(failed, 120);
    let late = compute(&[], &[opening, resolved, late], &[], window(0, 200)).actions;
    assert_eq!((late.attributed, late.unattributed), (0, 1));
}

#[test]
fn a_raw_push_joins_the_condition_its_source_event_announced() {
    // Given Q1@1 opened at 0, then escalated at 50 and unattended at 80.
    let suffix = "T-001/R1/Q1@1";
    let open = format!("T-001/question_open/{suffix}");
    let escalated = format!("T-001/question_escalated/{suffix}");
    let unattended = format!("T-001/question_unattended/{suffix}");
    let source = event(
        "question_opened",
        Some("T-001"),
        json!({"key": "T-001/R1/Q1", "revision": 1, "at": t(0)}),
    );
    let receipt = || {
        event(
            "pm_event_delivery",
            Some("T-001"),
            json!({"submitted": true, "event_kind": "question_opened", "pane": "pm:p1",
                "cursor": {"offset": 20}}),
        )
    };
    let events = vec![
        opened(&open, "pm", 0, json!(true)),
        source,
        resolved(&open, 50),
        opened(&escalated, "pm", 50, json!(true)),
        opened(&unattended, "pm", 80, json!(true)),
        receipt(),
        receipt(),
    ];
    // When the delayed receipt and its replay are read with their cursors.
    let cursors = [10, 20, 30, 40, 50, 60, 70];
    let raw = compute_logged(&[], &events, &cursors, &[], window(0, 100))
        .wakes
        .raw_push;
    // Then only the opening condition is attributed, once; nothing is ambiguous.
    assert_eq!(raw.attributed.get(&open), Some(&1));
    assert_eq!(raw.attributed.len(), 1);
    assert_eq!((raw.submitted, raw.replayed, raw.ambiguous), (1, 1, 0));
}

#[cfg(unix)]
#[path = "../../tests/attention_eval/crashes.rs"]
mod eval_crashes;
#[cfg(unix)]
#[path = "../../tests/attention_eval/producers.rs"]
mod eval_producers;
#[cfg(unix)]
#[path = "../../tests/attention_eval/retire.rs"]
mod eval_retire;
