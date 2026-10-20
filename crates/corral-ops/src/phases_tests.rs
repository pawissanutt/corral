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
    let mut totals = PhaseAccumulator::default();
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
    // owner approves (keeping the branch) before any verdict, then an owner verdict.
    let mut fx = Fixture::new();
    let spans = fx.all(vec![
        Fixture::at(0, "dispatched", run("R1")),
        Fixture::at(5, "done", run("R1")),
        Fixture::at(6, "review_started", run("R1")),
        Fixture::at(8, "review_escalated", json!({"run":"R1","reason":"idle"})),
        Fixture::at(20, "changes_requested", json!({"by":"owner"})),
        Fixture::at(30, "done", run("R1")),
        Fixture::at(31, "review_started", run("R1")),
        Fixture::at(35, "approved", json!({"run":"R1","strategy":"keep_branch"})),
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
            (Some(240.0), "owner_finished", Some("owner")),
        ]
    );
    let landing = one(&spans, Phase::Landing);
    assert_eq!(
        (landing.outcome.as_str(), landing.seconds),
        ("kept_branch", Some(0.0))
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
