//! Action-join regressions for park edges, effect windows and duplicate identity (in-crate,
//! reusing the reducer fixtures of `attention::metrics::tests`).
use serde_json::json;

use super::*;

#[test]
fn park_creation_never_answers_its_own_later_edges() {
    // Given a park created at 0 whose resume-ready and review-due edges open later.
    let ready = "T-001/park_resume_ready/1";
    let due = "T-001/park_review_due/1";
    let park = |revision: u64, at: i64| {
        event(
            "parked",
            Some("T-001"),
            json!({"park": {"revision": revision, "at": t(at)}, "by": "pm"}),
        )
    };
    let unpark = |at: i64| {
        event(
            "unparked",
            Some("T-001"),
            json!({"revision": 1, "by": "pm", "at": t(at)}),
        )
    };
    let edges = |due_too: bool| {
        let mut events = vec![park(1, 0), opened(ready, "pm", 60, json!(true))];
        events.extend(due_too.then(|| opened(due, "pm", 80, json!(true))));
        events
    };
    // When only the initial park precedes one or both edges, then it joins nothing.
    for due_too in [false, true] {
        let actions = compute(&[], &edges(due_too), &[], window(0, 200)).actions;
        assert_eq!((actions.attributed, actions.ambiguous), (0, 0), "{due_too}");
    }
    // When an unpark follows the ready edge, then it answers that edge; with both open it is
    // genuinely ambiguous.
    let mut after = edges(false);
    after.push(unpark(100));
    let after = compute(&[], &after, &[], window(0, 200)).actions;
    assert_eq!(after.by_condition.get(ready), Some(&1));
    let mut both = edges(true);
    both.push(unpark(100));
    assert_eq!(
        compute(&[], &both, &[], window(0, 200)).actions.ambiguous,
        1
    );
    // When the park is replaced by revision 2, then the replacement answers revision 1's edge.
    let mut replaced = edges(false);
    replaced.push(park(2, 100));
    let replaced = compute(&[], &replaced, &[], window(0, 200)).actions;
    assert_eq!(replaced.by_condition.get(ready), Some(&1));
}

#[test]
fn a_later_naming_event_never_moves_an_effect_across_the_window() {
    // Given a failed delivery at 0 and a post at 60 inside the 0..100 window.
    let failed = "T-001/delivery_failed/T-001_R1#1";
    for naming in ["replied", "changes_requested", "message_retried"] {
        let mut events = vec![
            at(posted("T-001_R1", 1), 0),
            opened(failed, "pm", 0, json!(true)),
            at(posted("T-001_R1", 2), 60),
        ];
        let before = compute(&[], &events, &[], window(0, 100)).actions;
        // When the naming event for that post is logged after the window ends.
        let detail = match naming {
            "message_retried" => json!({"from": "T-001_R1#1", "to": "T-001_R1#2"}),
            _ => json!({"run": "R1", "message": "T-001_R1#2"}),
        };
        events.push(at(event(naming, Some("T-001"), detail), 120));
        let after = compute(&[], &events, &[], window(0, 100)).actions;
        // Then the same single action stays in the window.
        assert_eq!((before.attributed, after.attributed), (1, 1), "{naming}");
        assert_eq!(after.by_condition.get(failed), Some(&1), "{naming}");
    }
}

#[test]
fn a_standalone_post_and_a_later_reply_share_the_recipient_duplicate_identity() {
    // Given one failed delivery and an outstanding standalone post #2 (a crash before naming).
    let failed = "T-001/delivery_failed/T-001_R1#1";
    for naming in ["replied", "changes_requested"] {
        for (case, between, duplicates, terminal) in [
            ("outstanding", None, 1, 0),
            (
                "observed",
                Some(event(
                    "message_observed",
                    Some("T-001"),
                    json!({"id": "T-001_R1#2"}),
                )),
                0,
                0,
            ),
            (
                "terminal",
                Some(event(
                    "message_failed",
                    Some("T-001"),
                    json!({"key": "T-001_R1", "seq": 2}),
                )),
                0,
                1,
            ),
        ] {
            let mut events = vec![
                posted("T-001_R1", 1),
                opened(failed, "pm", 0, json!(true)),
                posted("T-001_R1", 2),
            ];
            events.extend(between);
            // When a normal post+naming pair for #3 follows on the same recipient.
            events.push(posted("T-001_R1", 3));
            events.push(event(
                naming,
                Some("T-001"),
                json!({"run": "R1", "message": "T-001_R1#3"}),
            ));
            // Then two actions, judged against #2 through the recipient identity.
            let actions = compute(&[], &events, &[], window(0, 60)).actions;
            assert_eq!(actions.attributed, 2, "{naming} {case}");
            assert_eq!(
                (actions.duplicates, actions.after_terminal),
                (duplicates, terminal),
                "{naming} {case}"
            );
        }
    }
}

#[test]
fn delayed_naming_keeps_two_effects_in_causal_order() {
    // Given a failed delivery, an older effect #2 and a newer post+naming pair #3 at 20/21.
    let failed = "T-001/delivery_failed/T-001_R1#1";
    let naming = |kind: &str, seq: u64, seconds: i64| {
        let message = format!("T-001_R1#{seq}");
        let detail = match kind {
            "message_retried" => json!({"from": "T-001_R1#1", "to": message}),
            _ => json!({"run": "R1", "message": message}),
        };
        at(event(kind, Some("T-001"), detail), seconds)
    };
    for kind in ["replied", "changes_requested", "message_retried"] {
        for (case, duplicates, terminal) in [
            ("outstanding", 1, 0),
            ("observed", 0, 0),
            ("terminal", 0, 1),
        ] {
            for start in [-10, 10] {
                let settle = match case {
                    "observed" => Some(event(
                        "message_observed",
                        Some("T-001"),
                        json!({"id": "T-001_R1#2"}),
                    )),
                    "terminal" => Some(event(
                        "message_failed",
                        Some("T-001"),
                        json!({"key": "T-001_R1", "seq": 2}),
                    )),
                    _ => None,
                };
                let mut events = vec![
                    at(posted("T-001_R1", 1), -100),
                    opened(failed, "pm", -100, json!(true)),
                    at(posted("T-001_R1", 2), start),
                ];
                events.extend(settle.map(|settle| at(settle, start + 5)));
                events.push(at(posted("T-001_R1", 3), 20));
                events.push(naming(kind, 3, 21));
                let before = compute(&[], &events, &[], window(0, 100)).actions;
                // When #2's own naming event is appended only after the window, at 120.
                events.push(naming(kind, 2, 120));
                let after = compute(&[], &events, &[], window(0, 100)).actions;
                // Then #2 still precedes #3: the repeat is judged against it, and the charged
                // in-window actions stay those whose effects fall in the window.
                let label = format!("{kind} {case} {start}");
                let charged = if start < 0 { 1 } else { 2 };
                assert_eq!(after.attributed, charged, "{label}");
                assert_eq!(
                    (after.duplicates, after.after_terminal),
                    (duplicates, terminal),
                    "{label}"
                );
                // A reply or change request is a fresh post either way, so the delayed naming
                // event changes nothing; a retry's post only becomes a retry once named.
                if kind != "message_retried" {
                    assert_eq!(before, after, "{label}");
                }
            }
        }
    }
}
