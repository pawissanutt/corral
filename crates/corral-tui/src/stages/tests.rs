use corral_core::{GateRecord, ReviewRecord};
use corral_ops::watch::{CommandProgress, GateAttempt, ProgressState, ReviewerAttempt};
use ratatui::crossterm::event::KeyCode;

use super::{Stage, StageView};
use crate::artifacts::panels_fixture::{Fixture, fixture, key, render_at};
use crate::{Action, Hit, View};

fn gate(fx: &Fixture, attempt: &str, failed: bool) -> GateAttempt {
    let started = "2026-01-01T12:00:00Z".parse().unwrap();
    let ended = "2026-01-01T12:00:03Z".parse().unwrap();
    let log = fx.root.join(format!("{attempt}.log"));
    let first = "\n$ fmt\nformat clean\n";
    let second = if failed {
        "\n$ clippy\nchecking app\nerror[E0001]: bad widget\ntrailing context\n"
    } else {
        "\n$ clippy\nchecking app\nclippy clean\n"
    };
    std::fs::write(&log, format!("{first}{second}")).unwrap();
    let record: GateRecord = serde_json::from_value(serde_json::json!({
        "result": if failed { "fail" } else { "pass" },
        "at": ended, "sha": "abc", "failed_cmd": if failed { Some("clippy") } else { None },
    }))
    .unwrap();
    GateAttempt {
        run: "R1".into(),
        attempt: attempt.into(),
        round: u32::from(!failed),
        started,
        ended: Some(ended),
        duration_ms: Some(3000),
        state: if failed {
            ProgressState::Failed
        } else {
            ProgressState::Passed
        },
        commands: vec![
            CommandProgress {
                command: "fmt".into(),
                state: ProgressState::Passed,
                started: Some(started),
                ended: Some(ended),
                duration_ms: Some(3000),
                exit_code: Some(0),
                log_start: 0,
                log_end: first.len() as u64,
            },
            CommandProgress {
                command: "clippy".into(),
                state: if failed {
                    ProgressState::Failed
                } else {
                    ProgressState::Passed
                },
                started: Some(started),
                ended: Some(ended),
                duration_ms: Some(3000),
                exit_code: Some(if failed { 1 } else { 0 }),
                log_start: first.len() as u64,
                log_end: (first.len() + second.len()) as u64,
            },
        ],
        log,
        live_pane: None,
        record,
    }
}

#[test]
fn gate_navigation_selects_commands_and_historical_attempts() {
    // Given two completed rounds and a gate monitor.
    let fx = fixture();
    let mut view = StageView::new(&fx.ctx, &fx.id, Stage::Gate).unwrap();
    view.history.gates = vec![gate(&fx, "first", true), gate(&fx, "second", false)];
    view.load_content(&fx.ctx);

    // When keyboard navigation selects the previous round, then its first command.
    view.on_key(key(KeyCode::Char('[')), &fx.ctx);
    assert!(view.text.contains("error[E0001]"));
    assert!(!view.follow);
    assert_eq!(view.step_index(), 1);
    view.on_key(key(KeyCode::Up), &fx.ctx);

    // Then only that command's output is shown, not a later round or command.
    assert!(view.text.contains("format clean"));
    assert!(!view.text.contains("clippy"));
    assert_eq!(view.gate_index(), 0);
    assert_eq!(view.step_index(), 0);
    for (width, height) in [(160, 45), (80, 24)] {
        let (text, hits) = render_at(&mut view, &fx.ctx, width, height);
        assert!(
            text.contains("[passed]") && text.contains("[failed]"),
            "{text}"
        );
        assert!(text.contains("format clean"), "{text}");
        assert!(hits.iter().any(|(_, h)| *h == Hit::Key(key(KeyCode::F(3)))));
        println!("GATE {width}x{height}\n{text}");
    }
}

#[test]
fn stage_shortcuts_and_mouse_hits_match_in_detail_and_review() {
    // Given the two entry screens.
    let fx = fixture();
    let mut detail = crate::detail::DetailView::new(&fx.ctx, &fx.id).unwrap();
    let mut review = crate::review::ReviewView::new(&fx.ctx, &fx.id).unwrap();
    review.on_key(key(KeyCode::Esc), &fx.ctx); // close the report preview

    // When their stage strips are rendered and shortcuts used.
    for view in [&mut detail as &mut dyn View, &mut review as &mut dyn View] {
        let (_, hits) = render_at(view, &fx.ctx, 80, 24);
        for (index, stage) in Stage::ALL.into_iter().enumerate() {
            let press = key(KeyCode::F(u8::try_from(index + 1).unwrap()));
            assert!(hits.iter().any(|(_, h)| *h == Hit::Key(press)));
            // Then clicks resolve through the same key to the same stage.
            assert_eq!(
                view.on_key(press, &fx.ctx),
                Action::OpenStage(fx.id.clone(), stage)
            );
        }
    }
}

#[test]
fn reviewer_history_renders_verdict_notes_and_never_forwards_typing() {
    // Given separate reviewer accounting runs with their own verdicts.
    let fx = fixture();
    let mut view = StageView::new(&fx.ctx, &fx.id, Stage::Review).unwrap();
    let mut first = view.task.runs[0].clone();
    first.id = "R1-review-1".into();
    first.agent = "sonnet".into();
    first.model = Some("claude-sonnet".into());
    let mut second = first.clone();
    second.id = "R1-review-2".into();
    second.model = Some("claude-opus".into());
    let record = |verdict: &str, notes: &str| -> ReviewRecord {
        serde_json::from_value(serde_json::json!({
            "reviewer": "sonnet", "sha": "abc", "at": first.started,
            "verdict": verdict, "notes": notes,
        }))
        .unwrap()
    };
    view.history.reviews = vec![
        ReviewerAttempt {
            run: "R1".into(),
            round: 0,
            state: ProgressState::Failed,
            live_pane: None,
            accounting: first.clone(),
            record: Some(record("changes", "## Fix the widget\n\n- Keep its shape.")),
            artifacts: vec![],
        },
        ReviewerAttempt {
            run: "R1".into(),
            round: 1,
            state: ProgressState::Passed,
            live_pane: None,
            accounting: second,
            record: Some(record("approve", "## Ready to land")),
            artifacts: vec![],
        },
    ];

    // When the earlier round opens and arbitrary typing is sent.
    view.on_key(key(KeyCode::Char('[')), &fx.ctx);
    let before = view.text.clone();
    assert_eq!(view.on_key(key(KeyCode::Char('x')), &fx.ctx), Action::None);

    // Then the earlier model and rendered verdict stay readable without changing the transcript.
    assert_eq!(view.text, before);
    assert!(!view.captures_text());
    for (width, height) in [(160, 45), (80, 24)] {
        let (text, _) = render_at(&mut view, &fx.ctx, width, height);
        assert!(
            text.contains("claude-sonnet") && text.contains("Verdict: changes"),
            "{text}"
        );
        assert!(
            text.contains("Fix the widget") && !text.contains("## Fix"),
            "{text}"
        );
        assert!(!text.contains("claude-opus"), "{text}");
        println!("REVIEWER {width}x{height}\n{text}");
    }
}

#[test]
fn failure_jump_keeps_its_window_until_follow_is_resumed() {
    // Given a failed command with its first error older than the tail window.
    let fx = fixture();
    let mut attempt = gate(&fx, "long", true);
    let output = format!(
        "\n$ clippy\nerror[E0001]: original failure\n{}last output\n",
        "context\n".repeat(40_000)
    );
    std::fs::write(&attempt.log, &output).unwrap();
    attempt.commands.remove(0);
    attempt.commands[0].log_start = 0;
    attempt.commands[0].log_end = u64::try_from(output.len()).unwrap();
    let mut view = StageView::new(&fx.ctx, &fx.id, Stage::Gate).unwrap();
    view.history.gates.push(attempt);

    // When failure arrives and content refreshes again.
    view.load_content(&fx.ctx);
    let position = view.scroll.offset;
    view.load_content(&fx.ctx);

    // Then the first error remains the viewport anchor, until End resumes the tail.
    assert!(!view.follow);
    assert_eq!(view.scroll.offset, position);
    assert!(view.text.contains("original failure"));
    view.on_key(key(KeyCode::End), &fx.ctx);
    assert!(view.follow);
    assert!(view.text.contains("last output"));
    assert!(!view.text.contains("original failure"));
    view.on_key(key(KeyCode::Char('e')), &fx.ctx);
    assert!(!view.follow);
    assert!(view.text.contains("original failure"));
    assert!(!view.text.starts_with("\n$ clippy"));
}

#[test]
fn active_stage_labels_survive_normal_and_compact_cards() {
    // Given a review card and a cached active gate label.
    let fx = fixture();
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let label = "gating 2/4: clippy";
    let mut config = fx.ctx.store.config.clone();
    for density in [
        corral_core::Density::Comfortable,
        corral_core::Density::Compact,
    ] {
        config.board.card.density = density;
        // When the card renders at desktop width.
        let lines = crate::board::card_lines(
            &task,
            std::slice::from_ref(&task),
            &config,
            64,
            false,
            task.updated,
            crate::board::CardExtra {
                stage: Some(label),
                ..Default::default()
            },
        );
        // Then the active command is visible in either density.
        let text = crate::render::to_plain(&lines);
        assert!(text.contains(label), "{text}");
    }
}

#[test]
fn stage_monitor_handles_empty_history_and_all_stage_keys() {
    // Given no review pipeline attempts.
    let fx = fixture();
    let mut view = StageView::new(&fx.ctx, &fx.id, Stage::Gate).unwrap();
    assert!(
        render_at(&mut view, &fx.ctx, 80, 24)
            .0
            .contains("No gate attempts yet")
    );
    // When all stage keys are pressed.
    for (index, stage) in Stage::ALL.into_iter().enumerate() {
        assert_eq!(
            view.on_key(key(KeyCode::F(u8::try_from(index + 1).unwrap())), &fx.ctx),
            Action::None
        );
        // Then the selected stage changes locally, without an agent input action.
        assert_eq!(view.stage, stage);
    }
    assert_eq!(view.on_key(key(KeyCode::Esc), &fx.ctx), Action::Back);
}
