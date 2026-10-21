use super::*;
use corral_core::{Status, Store};
use serde_json::{Value, json};

const BASE: i64 = 1_767_225_600; // 2026-01-01T00:00:00Z

struct Fixture {
    _dir: tempfile::TempDir,
    task: Task,
    events: Vec<Event>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Phases", Some("app"), Status::Running, "")
            .unwrap();
        Self {
            _dir: dir,
            task,
            events: Vec::new(),
        }
    }

    fn at(minute: i64, kind: &str, detail: Value) -> (i64, String, Value) {
        (minute, kind.to_owned(), detail)
    }

    fn push(&mut self, minute: i64, kind: &str, detail: Value) -> &mut Self {
        let ts = Timestamp::from_second(BASE + minute * 60).unwrap();
        self.events.push(
            serde_json::from_value(json!({
                "task": self.task.id, "kind": kind, "ts": ts.to_string(), "detail": detail
            }))
            .unwrap(),
        );
        self
    }

    fn all(&mut self, events: Vec<(i64, String, Value)>) -> Vec<Span> {
        for (minute, kind, detail) in events {
            self.push(minute, &kind, detail);
        }
        self.spans()
    }

    fn spans(&self) -> Vec<Span> {
        task_phases(&self.task, &self.events.iter().collect::<Vec<_>>())
    }
}

fn of(spans: &[Span], phase: Phase) -> Vec<&Span> {
    spans.iter().filter(|span| span.phase == phase).collect()
}

fn one(spans: &[Span], phase: Phase) -> &Span {
    let found = of(spans, phase);
    assert_eq!(found.len(), 1, "{phase:?}: {found:#?}");
    found[0]
}

fn run(id: &str) -> Value {
    json!({ "run": id })
}

#[test]
fn happy_path_pairs_every_phase_and_subtracts_needs_me_from_active() {
    // Given a task that queues, waits on the owner twice, gates, reviews, waits for
    // owner acceptance and lands.
    let mut fx = Fixture::new();
    let receipt = |command: &str, started: Option<&str>, ended: Option<&str>| {
        let mut r = json!({"command":command, "exit_code":0, "log_start":0, "log_end":1});
        if let (Some(s), Some(e)) = (started, ended) {
            r["started"] = json!(s);
            r["ended"] = json!(e);
        }
        r
    };
    let spans = fx.all(vec![
        Fixture::at(0, "created", json!({})),
        Fixture::at(1, "queued", json!({"reason":"slots full"})),
        Fixture::at(2, "queued", json!({"reason":"slots full"})),
        Fixture::at(5, "dispatched", run("R1")),
        Fixture::at(10, "needs_me", json!({"from":"running"})),
        Fixture::at(15, "resumed", json!({"from":"needs_me"})),
        Fixture::at(20, "needs_me", json!({"from":"running"})),
        Fixture::at(30, "replied", json!({"text":"go"})),
        Fixture::at(40, "done", run("R1")),
        Fixture::at(41, "gate_started", run("R1")),
        Fixture::at(
            45,
            "gate",
            json!({"run":"R1","result":"pass","commands":[
                receipt("fmt", Some("2026-01-01T00:41:00Z"), Some("2026-01-01T00:43:00Z")),
                receipt("test", None, None),
            ]}),
        ),
        Fixture::at(46, "review_started", run("R1")),
        Fixture::at(
            50,
            "verdict",
            json!({"run":"R1","verdict":"approve","reviewer":"opus"}),
        ),
        Fixture::at(
            51,
            "autopilot",
            json!({"run":"R1","from":"reviewing","to":"reviewed"}),
        ),
        Fixture::at(
            60,
            "autopilot",
            json!({"run":"R1","from":"reviewed","to":"landing"}),
        ),
        Fixture::at(62, "approved", json!({"run":"R1","strategy":"rebase"})),
    ]);

    // Then each phase is complete with the paired duration.
    let queue = one(&spans, Phase::Queue);
    assert_eq!(
        (queue.seconds, queue.run.as_deref()),
        (Some(240.0), Some("R1"))
    );
    let waits: Vec<_> = of(&spans, Phase::NeedsMe)
        .iter()
        .map(|s| s.seconds)
        .collect();
    assert_eq!(waits, [Some(300.0), Some(600.0)]);
    let active = one(&spans, Phase::Active);
    assert_eq!(
        (active.round, active.seconds),
        (Some(1), Some(35.0 * 60.0 - 900.0))
    );
    let gate = one(&spans, Phase::Gate);
    assert_eq!((gate.outcome.as_str(), gate.seconds), ("pass", Some(240.0)));
    let commands: Vec<_> = gate.commands.iter().map(|c| c.seconds).collect();
    assert_eq!(
        commands,
        [Some(120.0), None],
        "legacy receipts carry no command time"
    );
    let review = one(&spans, Phase::Review);
    assert_eq!(
        (
            review.outcome.as_str(),
            review.by.as_deref(),
            review.seconds
        ),
        ("approve", Some("reviewer"), Some(240.0))
    );
    assert_eq!(one(&spans, Phase::OwnerAccept).seconds, Some(540.0));
    let landing = one(&spans, Phase::Landing);
    assert_eq!(
        (landing.outcome.as_str(), landing.seconds),
        ("landed", Some(120.0))
    );
    assert_eq!(one(&spans, Phase::Lead).seconds, Some(2400.0));
    assert_eq!(one(&spans, Phase::Cycle).seconds, Some(57.0 * 60.0));
    assert!(spans.iter().all(|span| span.complete));
}

#[test]
fn retries_split_rounds_and_restarted_or_released_attempts_stay_incomplete() {
    // Given two change rounds, a failed gate, a gate restarted before its result, a
    // reviewer whose process was lost, and a third round still running.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "created", json!({})),
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(10, "done", run("R1")),
        Fixture::at(11, "gate_started", run("R1")),
        Fixture::at(13, "gate", json!({"run":"R1","result":"fail"})),
        Fixture::at(14, "changes_requested", json!({"by":"autopilot"})),
        Fixture::at(30, "done", run("R1")),
        Fixture::at(31, "gate_started", run("R1")),
        Fixture::at(33, "gate_started", run("R1")),
        Fixture::at(40, "gate", json!({"run":"R1","result":"pass"})),
        Fixture::at(41, "review_started", run("R1")),
        Fixture::at(
            42,
            "review_released",
            json!({"run":"R1","kind":"reviewer","reason":"pane gone"}),
        ),
        Fixture::at(43, "review_started", run("R1")),
        Fixture::at(
            45,
            "verdict",
            json!({"run":"R1","verdict":"changes","reviewer":"opus"}),
        ),
        Fixture::at(46, "changes_requested", json!({"by":"autopilot"})),
    ]);

    // Then rounds are numbered and timed separately; open round 3 has no duration.
    let active: Vec<_> = of(&spans, Phase::Active)
        .iter()
        .map(|s| (s.round, s.seconds, s.outcome.as_str()))
        .collect();
    assert_eq!(
        active,
        [
            (Some(1), Some(600.0), "done"),
            (Some(2), Some(960.0), "done"),
            (Some(3), None, "open")
        ]
    );
    let gates: Vec<_> = of(&spans, Phase::Gate)
        .iter()
        .map(|s| (s.round, s.seconds, s.outcome.as_str(), s.complete))
        .collect();
    assert_eq!(
        gates,
        [
            (Some(1), Some(120.0), "fail", true),
            (Some(2), None, "no_result", false),
            (Some(2), Some(420.0), "pass", true),
        ]
    );
    let reviews: Vec<_> = of(&spans, Phase::Review)
        .iter()
        .map(|s| (s.seconds, s.outcome.as_str()))
        .collect();
    assert_eq!(reviews, [(None, "process_lost"), (Some(120.0), "changes")]);
    // A pending round means the task is not done; it was never landed either.
    assert_eq!(one(&spans, Phase::Lead).outcome, "open");
    assert!(one(&spans, Phase::Lead).seconds.is_none());
    assert_eq!(one(&spans, Phase::Cycle).outcome, "not_landed");
}

#[test]
fn gate_without_result_is_incomplete_never_a_long_duration() {
    // Given one gate whose process was lost ten hours later and one never collected.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(5, "done", run("R1")),
        Fixture::at(6, "gate_started", run("R1")),
        Fixture::at(
            606,
            "review_released",
            json!({"run":"R1","kind":"gate","reason":"pane gone"}),
        ),
        Fixture::at(607, "gate_started", run("R1")),
    ]);

    // Then both are incomplete without a duration, and analytics totals exclude them.
    let gates = of(&spans, Phase::Gate);
    assert_eq!(gates.len(), 2);
    assert_eq!(
        (
            gates[0].outcome.as_str(),
            gates[0].seconds,
            gates[0].complete
        ),
        ("process_lost", None, false)
    );
    assert!(gates[0].end.is_some());
    assert_eq!(
        (gates[1].outcome.as_str(), gates[1].seconds, gates[1].end),
        ("open", None, None)
    );
    let mut totals = crate::analytics::PhaseAccumulator::default();
    for span in &spans {
        totals.add(span);
    }
    let gate = &totals.finish()[&Phase::Gate];
    assert_eq!(
        (gate.complete, gate.incomplete, gate.total_seconds),
        (0, 2, None)
    );
}

#[test]
fn owner_finished_reviews_close_on_owner_decision() {
    // Given an escalated review the owner answers with changes, then a review the
    // owner approves (keeping the branch, first attempt) before any verdict, then an
    // owner verdict.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(5, "done", run("R1")),
        Fixture::at(6, "review_started", run("R1")),
        Fixture::at(8, "review_escalated", json!({"run":"R1","reason":"idle"})),
        Fixture::at(20, "changes_requested", json!({"by":"owner"})),
        Fixture::at(30, "done", run("R1")),
        Fixture::at(31, "review_started", run("R1")),
        Fixture::at(
            35,
            "approved",
            json!({"run":"R1","strategy":"keep_branch","started":"2026-01-01T00:34:00Z"}),
        ),
    ]);

    // Then the reviews are complete, owner-finished spans; keep_branch lands nothing.
    let reviews: Vec<_> = of(&spans, Phase::Review)
        .iter()
        .map(|s| (s.seconds, s.outcome.as_str(), s.by.as_deref()))
        .collect();
    assert_eq!(
        reviews,
        [
            (Some(840.0), "owner_finished", Some("owner")),
            (Some(180.0), "owner_finished", Some("owner")),
        ]
    );
    let landing = one(&spans, Phase::Landing);
    assert_eq!(
        (landing.outcome.as_str(), landing.seconds),
        ("kept_branch", Some(60.0))
    );
    assert_eq!(one(&spans, Phase::Cycle).outcome, "not_landed");

    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(1, "review_started", run("R1")),
        Fixture::at(
            3,
            "verdict",
            json!({"run":"R1","verdict":"approve","reviewer":"owner"}),
        ),
    ]);
    let review = one(&spans, Phase::Review);
    assert_eq!(
        (review.by.as_deref(), review.outcome.as_str()),
        (Some("owner"), "approve")
    );
}

#[test]
fn interruptions_and_runless_events_follow_their_dispatch_interval() {
    // Given an unanswered needs-me inside a finished round, a superseded run with a
    // running gate, a dequeued wait, and a runless needs-me after the next dispatch.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(5, "needs_me", json!({})),
        Fixture::at(10, "done", run("R1")),
        Fixture::at(11, "gate_started", run("R1")),
        Fixture::at(12, "superseded", run("R1")),
        Fixture::at(13, "queued", json!({})),
        Fixture::at(14, "dequeued", json!({})),
        Fixture::at(15, "queued", json!({})),
        Fixture::at(16, "dispatched", run("R2")),
        Fixture::at(20, "needs_me", json!({})),
        Fixture::at(26, "resumed", json!({})),
    ]);

    // Then unknown waiting leaves R1's active time unknown instead of guessing.
    let active = of(&spans, Phase::Active);
    assert_eq!(
        (active[0].outcome.as_str(), active[0].seconds),
        ("needs_me_incomplete", None)
    );
    let gate = one(&spans, Phase::Gate);
    assert_eq!((gate.outcome.as_str(), gate.seconds), ("superseded", None));
    let queues: Vec<_> = of(&spans, Phase::Queue)
        .iter()
        .map(|s| (s.outcome.as_str(), s.seconds, s.run.as_deref()))
        .collect();
    assert_eq!(
        queues,
        [
            ("dequeued", None, None),
            ("dispatched", Some(60.0), Some("R2"))
        ]
    );
    let waits: Vec<_> = of(&spans, Phase::NeedsMe)
        .iter()
        .map(|s| (s.run.as_deref(), s.seconds, s.outcome.as_str()))
        .collect();
    assert_eq!(
        waits,
        [
            (Some("R1"), None, "done"),
            (Some("R2"), Some(360.0), "resumed")
        ]
    );
    assert!(spans.iter().all(|s| s.complete == s.seconds.is_some()));
}

#[test]
fn failed_reply_delivery_keeps_waiting_until_the_resend() {
    // Given a reply whose delivery failed, followed by a successful resend.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(2, "needs_me", json!({})),
        Fixture::at(
            4,
            "replied",
            json!({"submitted":false,"error":"not delivered"}),
        ),
        Fixture::at(8, "replied", json!({"submitted":true})),
        Fixture::at(10, "done", run("R1")),
    ]);
    // Then the wait runs to the delivered reply and active time excludes all of it.
    assert_eq!(one(&spans, Phase::NeedsMe).seconds, Some(360.0));
    assert_eq!(one(&spans, Phase::Active).seconds, Some(240.0));
}

#[test]
fn failed_change_request_neither_finishes_review_nor_opens_a_round() {
    // Given a change request that was never delivered, then its successful resend.
    let mut fx = Fixture::new();
    let failed = fx.all(vec![
        Fixture::at(0, "created", json!({})),
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(5, "done", run("R1")),
        Fixture::at(6, "review_started", run("R1")),
        Fixture::at(
            8,
            "changes_requested",
            json!({"by":"owner","submitted":false}),
        ),
    ]);
    // Then round 1 stays the only round, the review stays open and lead is done.
    assert_eq!(of(&failed, Phase::Active).len(), 1);
    assert_eq!(one(&failed, Phase::Review).outcome, "open");
    assert_eq!(one(&failed, Phase::Lead).seconds, Some(300.0));

    fx.push(
        9,
        "changes_requested",
        json!({"by":"owner","submitted":true}),
    );
    let resent = fx.spans();
    let review = one(&resent, Phase::Review);
    assert_eq!(
        (review.outcome.as_str(), review.seconds),
        ("owner_finished", Some(180.0))
    );
    let rounds: Vec<_> = of(&resent, Phase::Active).iter().map(|s| s.round).collect();
    assert_eq!(rounds, [Some(1), Some(2)]);
    assert_eq!(one(&resent, Phase::Lead).outcome, "open");
}

#[test]
fn approval_without_a_recorded_start_is_incomplete_not_zero() {
    // Given a legacy owner approval event without `started`, while owner accept is open.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(5, "done", run("R1")),
        Fixture::at(6, "autopilot", json!({"run":"R1","to":"reviewed"})),
        Fixture::at(
            20,
            "approved",
            json!({"run":"R1","strategy":"fast_forward"}),
        ),
    ]);
    // Then neither landing nor the owner's wait claims a measured duration.
    let landing = one(&spans, Phase::Landing);
    assert_eq!(
        (landing.complete, landing.seconds, landing.outcome.as_str()),
        (false, None, "unknown_start")
    );
    let accept = one(&spans, Phase::OwnerAccept);
    assert_eq!(
        (accept.complete, accept.outcome.as_str()),
        (false, "unknown_decision_time")
    );
    assert_eq!(one(&spans, Phase::Cycle).seconds, Some(1200.0));
}

#[test]
fn first_attempt_owner_landing_splits_owner_wait_from_landing() {
    // Given an owner approval that started at minute 18 and landed at minute 20.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(5, "done", run("R1")),
        Fixture::at(6, "autopilot", json!({"run":"R1","to":"reviewed"})),
        Fixture::at(
            20,
            "approved",
            json!({"run":"R1","strategy":"rebase","started":"2026-01-01T00:18:00Z"}),
        ),
    ]);
    // Then the owner waited 12 minutes and the landing itself took 2.
    assert_eq!(one(&spans, Phase::OwnerAccept).seconds, Some(720.0));
    let landing = one(&spans, Phase::Landing);
    assert_eq!(
        (landing.outcome.as_str(), landing.seconds),
        ("landed", Some(120.0))
    );
}

#[test]
fn portable_decision_resume_closes_only_its_own_wait() {
    // Given a wait raising decisions D1 and D2, an unrelated wait receipt, and
    // producer-shaped answered receipts for each decision.
    let mut fx = Fixture::new();
    let answered = |id: &str| json!({"run":"R1","receipt":id,"decision":id,"outcome":"answered","delivery":"acknowledged"});
    fx.push(0, "dispatched", run("R1"))
        .push(2, "needs_me", json!({"run":"R1","decision":"D1"}))
        .push(3, "needs_me", json!({"run":"R1","decision":"D2"}))
        .push(
            4,
            "run_resumed",
            json!({"run":"R1","receipt":"W1","outcome":"completed","delivery":"acknowledged"}),
        )
        .push(5, "run_resumed", answered("D1"));
    // Then D2 still holds the wait open.
    let pending = fx.spans();
    assert_eq!(one(&pending, Phase::NeedsMe).outcome, "open");

    fx.push(8, "run_resumed", answered("D2"))
        .push(10, "done", run("R1"));
    let spans = fx.spans();
    let wait = one(&spans, Phase::NeedsMe);
    assert_eq!(
        (wait.outcome.as_str(), wait.seconds),
        ("run_resumed", Some(360.0))
    );
    assert_eq!(one(&spans, Phase::Active).seconds, Some(240.0));
}

#[test]
fn legacy_run_resumed_without_decision_matches_its_receipt_id() {
    // Given an older run_resumed whose receipt id names the decision.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(2, "needs_me", json!({"run":"R1","decision":"D1"})),
        Fixture::at(
            8,
            "run_resumed",
            json!({"run":"R1","receipt":"D1","outcome":"answered","delivery":"acknowledged"}),
        ),
        Fixture::at(10, "done", run("R1")),
    ]);
    // Then the wait and active time are both measured.
    assert_eq!(one(&spans, Phase::NeedsMe).seconds, Some(360.0));
    assert_eq!(one(&spans, Phase::Active).seconds, Some(240.0));
}

#[test]
fn ordinary_replies_keep_a_multi_decision_wait_until_each_receipt() {
    // Given decisions D1 and D2 answered by ordinary replies, each followed by the
    // run_resumed receipt the reply producer writes for the selected decision.
    let mut fx = Fixture::new();
    let answered = |id: &str| json!({"run":"R1","receipt":id,"decision":id,"outcome":"answered","delivery":"acknowledged"});
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(2, "needs_me", json!({"run":"R1","decision":"D1"})),
        Fixture::at(3, "needs_me", json!({"run":"R1","decision":"D2"})),
        Fixture::at(5, "replied", json!({"submitted":true})),
        Fixture::at(5, "run_resumed", answered("D1")),
        Fixture::at(8, "replied", json!({"submitted":true})),
        Fixture::at(8, "run_resumed", answered("D2")),
        Fixture::at(10, "done", run("R1")),
    ]);
    // Then the wait ends with the last decision's receipt.
    let wait = one(&spans, Phase::NeedsMe);
    assert_eq!(
        (wait.outcome.as_str(), wait.seconds),
        ("run_resumed", Some(360.0))
    );
    assert_eq!(one(&spans, Phase::Active).seconds, Some(240.0));
}

#[test]
fn unfinished_fresh_retry_reopens_lead_until_its_own_done() {
    // Given R1 done, a failed gate, supersession, a queued wait and an unfinished R2.
    let mut fx = Fixture::new();
    fx.push(0, "created", json!({}))
        .push(1, "dispatched", run("R1"))
        .push(5, "done", run("R1"))
        .push(6, "gate_started", run("R1"))
        .push(10, "gate", json!({"run":"R1","result":"fail"}))
        .push(11, "superseded", run("R1"));
    let lead_of = |fx: &Fixture| one(&fx.spans(), Phase::Lead).clone();
    let analytics_lead = |fx: &Fixture, minute: i64| {
        let mut task = fx.task.clone();
        task.runs = ["R1", "R2"]
            .iter()
            .zip([1, 12])
            .filter(|(_, at)| *at <= minute)
            .map(|(id, at)| {
                serde_json::from_value(json!({
                    "id":id, "agent":"fixture", "worktree":"/unused", "branch":"b",
                    "base":"main", "base_sha":"abc",
                    "started":Timestamp::from_second(BASE + at * 60).unwrap()
                }))
                .unwrap()
            })
            .collect();
        let now = Timestamp::from_second(BASE + minute * 60).unwrap();
        let report = crate::collect_analytics(&[task], &fx.events, None, now).unwrap();
        let row = &report.groups[&crate::StatsBy::Project][0];
        let lead = &row.analytics.phases[&Phase::Lead];
        (lead.complete, lead.incomplete, lead.total_seconds)
    };
    // Then the queued interval after supersession already reopens lead time.
    let queued = lead_of(&fx);
    assert_eq!((queued.complete, queued.seconds), (false, None));
    assert_eq!(analytics_lead(&fx, 11), (0, 1, None));

    fx.push(12, "queued", json!({}))
        .push(13, "dispatched", run("R2"));
    let unfinished = lead_of(&fx);
    assert_eq!(
        (
            unfinished.complete,
            unfinished.seconds,
            unfinished.outcome.as_str()
        ),
        (false, None, "open")
    );
    assert_eq!(analytics_lead(&fx, 15), (0, 1, None));
    // R1's completed round is still measured.
    let active: Vec<_> = of(&fx.spans(), Phase::Active)
        .iter()
        .map(|s| (s.run.clone(), s.seconds))
        .collect();
    assert_eq!(active[0], (Some("R1".into()), Some(240.0)));

    // When R2 reports done, lead completes at R2's done.
    fx.push(20, "done", run("R2"));
    assert_eq!(lead_of(&fx).seconds, Some(1200.0));
    assert_eq!(analytics_lead(&fx, 21), (1, 0, Some(1200.0)));
}

#[test]
fn discard_before_first_dispatch_interrupts_the_queue_and_requeue_starts_fresh() {
    // Given a never-dispatched queued task discarded with a null run (the real
    // producer shape), then requeued and first dispatched.
    let mut fx = Fixture::new();
    fx.push(1, "queued", json!({}))
        .push(2, "discarded", json!({"run": null}));
    let discarded = fx.spans();
    let queue = one(&discarded, Phase::Queue);
    assert_eq!(
        (
            queue.complete,
            queue.seconds,
            queue.outcome.as_str(),
            queue.end
        ),
        (
            false,
            None,
            "discarded",
            Some(Timestamp::from_second(BASE + 120).unwrap())
        )
    );

    fx.push(10, "queued", json!({}))
        .push(11, "dispatched", run("R1"));
    let spans = fx.spans();
    // Then the discarded interval stays incomplete and the requeue measures 60 seconds.
    let queues: Vec<_> = of(&spans, Phase::Queue)
        .iter()
        .map(|s| (s.run.as_deref(), s.seconds, s.outcome.as_str()))
        .collect();
    assert_eq!(
        queues,
        [
            (None, None, "discarded"),
            (Some("R1"), Some(60.0), "dispatched")
        ]
    );
    // And analytics counts one complete 60-second sample and one incomplete wait.
    let mut task = fx.task.clone();
    task.runs = vec![
        serde_json::from_value(json!({
            "id":"R1", "agent":"fixture", "worktree":"/unused", "branch":"b",
            "base":"main", "base_sha":"abc",
            "started":Timestamp::from_second(BASE + 11 * 60).unwrap()
        }))
        .unwrap(),
    ];
    let now = Timestamp::from_second(BASE + 3600).unwrap();
    let report = crate::collect_analytics(&[task], &fx.events, None, now).unwrap();
    let queue = &report.groups[&crate::StatsBy::Project][0].analytics.phases[&Phase::Queue];
    assert_eq!(
        (queue.complete, queue.incomplete, queue.total_seconds),
        (1, 1, Some(60.0))
    );
}
