//! T-057 readback: rendered board cards and detail for waits, owner decisions and diagnostic wait states.
//! Set `CORRAL_T057_EVIDENCE=<dir>` to also write each rendered screen to `<dir>/<name>.txt`.

use corral_core::{
    DecisionRecord, DecisionRequest, ObligationSource, Run, StuckProcess, WaitHandle,
    WaitObservation, WaitRecord, WaitRequest, WaitState,
};
use jiff::Timestamp;
use ratatui::{Terminal, backend::TestBackend, crossterm::event::KeyCode};

use super::DetailView;
use crate::View;
use crate::artifacts::panels_fixture::{key as code, render};
use crate::board::tests::{Fixture, buffer_text, key, seeded};

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn wait(id: &str, state: WaitState, condition: &str, detail: &str) -> WaitRecord {
    WaitRecord {
        request: WaitRequest {
            version: 1,
            id: id.into(),
            run: "R1".into(),
            condition: condition.into(),
            owner: "worker".into(),
            source: ObligationSource::Worker,
            deadline: Some(ts("2026-01-01T12:30:00Z")),
            handle: WaitHandle::Gate {
                sha: "a1b2c3d".into(),
                pane: "p-7".into(),
            },
        },
        registered_at: ts("2026-01-01T12:00:00Z"),
        observation: WaitObservation {
            at: ts("2026-01-01T12:05:00Z"),
            state,
            detail: detail.into(),
        },
        receipt: None,
    }
}

fn decision() -> DecisionRecord {
    DecisionRecord {
        request: DecisionRequest {
            version: 1,
            id: "D1".into(),
            run: "R1".into(),
            question: "Ship the 0600 fallback?".into(),
            requested_action: "answer yes or no".into(),
            owner: "pawis".into(),
            source: ObligationSource::Worker,
            deadline: None,
        },
        registered_at: ts("2026-01-01T12:01:00Z"),
        observation: WaitObservation {
            at: ts("2026-01-01T12:01:00Z"),
            state: WaitState::Pending,
            detail: "asked by the worker".into(),
        },
        receipt: None,
    }
}

fn update(fx: &Fixture, id: &str, f: impl Fn(&mut Run)) {
    fx.ctx
        .store
        .update(id, |t| {
            f(t.current_run_mut().unwrap());
            Ok(())
        })
        .unwrap();
}

fn capture(name: &str, text: &str) {
    println!("{name}:\n{text}");
    if let Ok(dir) = std::env::var("CORRAL_T057_EVIDENCE") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.txt")), text).unwrap();
    }
}

/// The five-column board (Done hidden) so card meta has room.
fn board(fx: &Fixture) -> String {
    let mut b = fx.board();
    b.on_key(key('D'), &fx.ctx);
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    term.draw(|f| b.draw(f, f.area(), &fx.ctx, &mut crate::HitMap::default()))
        .unwrap();
    buffer_text(term.backend().buffer())
}

#[test]
fn running_card_shows_pending_wait_instead_of_idle_no_report() {
    // Given an idle running run that declared a pending wait.
    let fx = seeded();
    update(&fx, "T-004", |r| {
        r.agent_state = Some("idle".into());
        r.idle_no_report = true;
        r.obligations.waits = vec![wait("W1", WaitState::Pending, "CI green", "")];
    });
    // When the board renders.
    let text = board(&fx);
    capture("board-waiting", &text);
    // Then the card says it is waiting on that condition, not idle without a report.
    assert!(text.contains("⟳ 5m · ⧗ waiting"), "{text}");
    assert!(text.contains("⧗ pending CI green"), "{text}");
    assert!(!text.contains("idle, no report"), "{text}");
}

#[test]
fn running_card_shows_diagnostic_waits_and_keeps_the_stuck_badge() {
    // Given an idle run whose waits expired or lost their delivery.
    let fx = seeded();
    update(&fx, "T-004", |r| {
        r.agent_state = Some("idle".into());
        r.idle_no_report = true;
        r.obligations.waits = vec![
            wait("W1", WaitState::Expired, "CI green", "deadline passed"),
            wait("W2", WaitState::DeliveryUnknown, "review", ""),
        ];
    });
    // When the board renders, the diagnostic states show and do not excuse the idle run.
    let text = board(&fx);
    capture("board-expired", &text);
    assert!(text.contains("⚠ expired CI green"), "{text}");
    assert!(text.contains("⚠ delivery_unknown review"), "{text}");
    assert!(text.contains("⚠ idle, no report"), "{text}");
    assert!(!text.contains("⧗ waiting"), "{text}");

    // And a stuck process still shows even with a pending wait.
    update(&fx, "T-004", |r| {
        r.obligations.waits = vec![wait("W1", WaitState::Pending, "CI green", "")];
        r.stuck = Some(StuckProcess {
            pid: 4242,
            argv: vec!["sleep".into(), "9".into()],
            started: "2026-01-01T11:58:00Z".into(),
        });
    });
    let text = board(&fx);
    capture("board-stuck-waiting", &text);
    assert!(text.contains("⚠ stuck: sleep 9"), "{text}");
    assert!(text.contains("⧗ pending CI green"), "{text}");
}

#[test]
fn needs_me_card_shows_exact_decision_over_terminal_excerpt() {
    // Given a blocked run with a pane excerpt and an explicit owner decision.
    let fx = seeded();
    update(&fx, "T-005", |r| r.obligations.decisions = vec![decision()]);
    // When the board renders.
    let text = board(&fx);
    capture("board-decision", &text);
    // Then the exact question and requested action replace the excerpt heuristic.
    assert!(text.contains("\"Ship the 0600 fallback?\""), "{text}");
    assert!(text.contains("→ answer yes or no"), "{text}");
    assert!(!text.contains("Keep the 0600"), "{text}");
}

#[test]
fn detail_reads_back_full_decision_and_wait_records() {
    // Given a blocked run with a pending decision and an expired wait, and no live pane.
    let fx = seeded();
    update(&fx, "T-005", |r| {
        r.obligations.decisions = vec![decision()];
        r.obligations.waits = vec![wait(
            "W1",
            WaitState::Expired,
            "CI green",
            "deadline passed",
        )];
    });
    let mut v = DetailView::new(&fx.ctx, "T-005").unwrap();
    // When the Runs tab renders.
    v.on_key(code(KeyCode::Char('2')), &fx.ctx);
    v.on_key(code(KeyCode::Char('L')), &fx.ctx);
    let text = render(&mut v, &fx.ctx);
    capture("detail-decision-expired", &text);
    // Then the header, the Runs tab and the Live fallback read back the persistent records.
    assert!(text.contains("? decision D1"), "header badge\n{text}");
    for want in [
        "⧗ pending decision D1 · owner pawis · source worker",
        "question: Ship the 0600 fallback?",
        "action: answer yes or no",
        "registered: 2026-01-01T12:01:00Z",
        "deadline: none",
        "⚠ expired wait W1 · owner worker · source worker",
        "condition: CI green",
        "handle: gate a1b2c3d · pane p-7",
        "deadline: 2026-01-01T12:30:00Z",
        "observed: 2026-01-01T12:05:00Z expired: deadline passed",
        "receipt: none",
        "decision D1 for pawis (pane not live):",
    ] {
        assert!(text.contains(want), "missing {want:?}\n{text}");
    }
    assert!(
        !text.contains("blocked excerpt"),
        "decision beats excerpt\n{text}"
    );
}

#[test]
fn detail_header_keeps_stuck_badge_next_to_wait_badge() {
    // Given a running run with a pending wait and a stuck process.
    let fx = seeded();
    update(&fx, "T-004", |r| {
        r.obligations.waits = vec![wait("W1", WaitState::Pending, "CI green", "")];
        r.stuck = Some(StuckProcess {
            pid: 4242,
            argv: vec!["sleep".into(), "9".into()],
            started: "2026-01-01T11:58:00Z".into(),
        });
    });
    // When the detail renders.
    let mut v = DetailView::new(&fx.ctx, "T-004").unwrap();
    v.on_key(code(KeyCode::Char('1')), &fx.ctx);
    let text = render(&mut v, &fx.ctx);
    capture("detail-stuck-waiting", &text);
    // Then both badges show.
    assert!(
        text.contains("⧗ wait W1 pending  ⚠ stuck: sleep 9"),
        "{text}"
    );
}
