//! Headless regressions for the mounted Gate view's scroll: the real `modal` (its scroll's own
//! `on_scroll` callback pins the held window; nothing here preloads `GateUi::held`), driven by
//! controlled facts injected into the feed as the daemon would deliver them, and the queued
//! `queue_end` race.

use std::time::Duration;

use super::*;
use crate::model::Id;
use crate::model::daemon::{DetailCache, detail_key};
use corral_types::{Actor, Check, CheckKind, CheckResult, GateAttempt, GateAttemptState};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

fn open() -> GateOpen {
    GateOpen {
        task: Id::new("T-1"),
        attempt: "gate-one".into(),
    }
}

/// `rows` lines, one with distinguishable markers stretching over multiple display rows.
fn log_text(rows: usize) -> String {
    (0..rows)
        .map(|n| {
            if n == 3 {
                (0..300).map(|marker| format!("mark{marker:03}")).collect()
            } else {
                format!("row {n:03}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn attempt(state: GateAttemptState, heartbeat: i64) -> GateAttempt {
    let at = |second| jiff::Timestamp::from_second(second).unwrap();
    GateAttempt {
        id: "gate-one".into(),
        task: "T-1".into(),
        run: "R1".into(),
        sha: "0123456789abcdef".into(),
        round: 0,
        started: at(100),
        heartbeat: Some(at(heartbeat)),
        last_output: Some(at(101)),
        command: Some("just gate".into()),
        step: Some(1),
        steps: 3,
        state,
        finished: (state == GateAttemptState::Passed).then(|| at(heartbeat)),
        check: (state == GateAttemptState::Passed).then(|| Check {
            task: "T-1".into(),
            kind: CheckKind::Gate,
            key: "0123456789abcdef".into(),
            result: CheckResult::Pass,
            notes: None,
            log: Some("T-1:1".into()),
            by: Actor::Auto,
            at: at(heartbeat),
            round: Some(0),
            run: Some("R1".into()),
        }),
    }
}

fn page(state: GateAttemptState, heartbeat: i64, rows: usize) -> GateAttemptPage {
    let text = log_text(rows);
    let end = text.len() as u64;
    GateAttemptPage {
        attempt: attempt(state, heartbeat),
        start: 0,
        end,
        total: end,
        text,
    }
}

/// Delivers `page` for the newest window the way a daemon read does.
fn deliver(app: AppState, link: Link, now: u64, page: GateAttemptPage) {
    let open = open();
    let target = target(open.task.as_str(), &open.attempt, None);
    app.feed.update(|feed| {
        feed.link = link;
        feed.now = now;
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                target,
                transcript_seams: Default::default(),
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::GateAttempt(page)),
            },
        );
    });
}

/// The facts the daemon delivers, one step per press of `advance`:
/// 1 output grows while following; 2 the runner is quiet but heartbeats; 3 output resumes;
/// 4 the link drops; 5 it returns with the final output and the result.
fn script(app: AppState, step: u32) {
    use GateAttemptState::*;
    match step {
        1 => deliver(app, Link::Up, 106, page(Running, 105, 64)),
        2 => deliver(app, Link::Up, 111, page(Running, 111, 64)),
        3 => deliver(app, Link::Up, 113, page(Running, 112, 80)),
        4 => deliver(app, Link::Down, 116, page(Running, 112, 80)),
        _ => deliver(app, Link::Up, 121, page(Passed, 120, 96)),
    }
}

#[component]
fn GateFixture() -> impl IntoElement {
    let app = AppState::install(Form::Terminal);
    // Keep the real client's startup/disconnect events out of the scripted feed.
    app.data.set(crate::model::DataState::Fixture);
    deliver(app, Link::Up, 104, page(GateAttemptState::Running, 103, 48));
    let ui = signal(GateUi::default());
    let step = signal(0u32);
    let gate = modal(app, open(), ui);
    view()
        .w(px(400.0))
        .h(px(600.0))
        .child(
            view()
                .test_id("advance")
                .on_press(move || {
                    let next = step.get_untracked() + 1;
                    step.set(next);
                    script(app, next);
                })
                .child(text("Advance")),
        )
        .child(gate.body)
}

fn start<C: Component<Props: Default>>() -> Headless {
    let mut config = EngineConfig::new(Root::of::<C>());
    config.environment.idiom = nsh_core::platform::Idiom::Terminal;
    config.environment.cell = Some(nsh_core::Size::new(px(8.0), px(16.0)));
    Headless::start(config).unwrap()
}

fn bounds(app: &mut Headless, id: &str) -> nsh_core::Bounds<Points> {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no node {id}"))
        .bounds
}

fn content(app: &mut Headless, log: &str) -> nsh_core::Bounds<Points> {
    bounds(app, &format!("{log}-content"))
}

// Header/result changes can shift the scroll surface but must not change its content offset.
fn offset(app: &mut Headless, log: &str) -> f32 {
    content(app, log).origin.y.0 - bounds(app, log).origin.y.0
}

fn tail_gap(app: &mut Headless, log: &str) -> f32 {
    content(app, log).bottom().0 - bounds(app, log).bottom().0
}

#[test]
fn a_held_gate_viewport_survives_quiet_resume_reconnect_and_the_final_result() {
    let mut app = start::<GateFixture>();
    app.wait_for("id:gate-log").unwrap();
    app.wait_for("text:/following the newest output/").unwrap();
    let initial_height = content(&mut app, "gate-log").size.height.0;
    assert!(initial_height > 48.0 * 16.0, "long output did not wrap");

    // Following: the same log growing keeps its last line in view.
    app.press("id:advance").unwrap();
    let held_height = content(&mut app, "gate-log").size.height.0;
    assert!(held_height > initial_height);
    assert!(
        tail_gap(&mut app, "gate-log").abs() <= 0.5,
        "follow lost the tail"
    );

    // The owner wheels up: the view's own scroll callback pins the window.
    let top = offset(&mut app, "gate-log");
    app.wheel("id:gate-log", nsh_core::Point::new(px(0.0), px(-80.0)))
        .unwrap();
    app.wait_for("id:btn-gate-newest").unwrap();
    app.wait_for("text:/paused/").unwrap();
    let reading = offset(&mut app, "gate-log");
    assert!(reading > top, "the wheel must scroll toward older output");
    let still = |app: &mut Headless, when: &str| {
        let now = offset(app, "gate-log");
        assert_eq!(content(app, "gate-log").size.height.0, held_height);
        assert!(
            (now - reading).abs() <= 0.5,
            "held viewport moved {reading} -> {now} {when}"
        );
    };

    // Quiet: heartbeat advances, no new output; the held lines stay.
    app.press("id:advance").unwrap();
    app.wait_for("text:/◐ running/").unwrap();
    still(&mut app, "while quiet");

    // Resume: the runner writes more; the held window does not slide.
    app.press("id:advance").unwrap();
    app.wait_for("text:/paused.*new/").unwrap();
    still(&mut app, "on resume");

    // Offline: liveness is unknown, the held lines remain.
    app.press("id:advance").unwrap();
    app.wait_for("text:/liveness unknown/").unwrap();
    still(&mut app, "offline");

    // Reconnect with the final output: the head says the result, the viewport has not moved.
    app.press("id:advance").unwrap();
    app.wait_for("text:/check passed/").unwrap();
    app.wait_for("id:btn-gate-newest").unwrap();
    app.wait_for("id:btn-gate-cite").unwrap();
    still(&mut app, "after the final result");

    // Newest returns to the live end: the final window, at its tail.
    app.press("id:btn-gate-newest").unwrap();
    app.wait_for_gone("id:btn-gate-newest", Duration::from_secs(2))
        .unwrap();
    assert!(content(&mut app, "gate-log").size.height.0 > held_height);
    assert!(
        tail_gap(&mut app, "gate-log").abs() <= 0.5,
        "Newest must scroll to the end"
    );
}

/// A scroll with `queue_end` queued from presses, the way an answer queues its scroll to the
/// end: the first press queues it alone; the second also holds the window before it runs.
#[component]
fn QueueFixture() -> impl IntoElement {
    let ui = signal(GateUi::default());
    let scroller = element_ref();
    view()
        .w(px(200.0))
        .h(px(400.0))
        .child(
            view()
                .test_id("queue-free")
                .on_press(move || {
                    ui.set(GateUi::default());
                    queue_end(ui, scroller);
                })
                .child(text("Queue")),
        )
        .child(
            view()
                .test_id("queue-held")
                .on_press(move || {
                    queue_end(ui, scroller);
                    // The owner scrolled back before the queued callback ran.
                    ui.update(|ui| ui.held = Some(page(GateAttemptState::Running, 105, 4)));
                })
                .child(text("Queue then hold")),
        )
        .child(
            scroll()
                .test_id("queue-log")
                .element_ref(&scroller)
                .h(px(160.0))
                .child(
                    view()
                        .test_id("queue-log-content")
                        .children((0..100).map(|n| text(format!("row {n}")).into_element())),
                ),
        )
}

#[test]
fn a_queued_end_scroll_rechecks_a_hold_taken_before_it_runs() {
    let mut app = start::<QueueFixture>();
    app.wait_for("id:queue-log").unwrap();
    let top = content(&mut app, "queue-log").origin.y.0;
    app.press("id:queue-held").unwrap();
    let held = content(&mut app, "queue-log").origin.y.0;
    assert!(
        (held - top).abs() <= 0.5,
        "a hold taken before the queued scroll ran was overridden: {top} -> {held}"
    );
    // Control: with nothing held the same queued scroll does reach the end.
    app.press("id:queue-free").unwrap();
    assert!(
        tail_gap(&mut app, "queue-log").abs() <= 0.5,
        "the queued scroll must still follow when nothing is held"
    );
}
