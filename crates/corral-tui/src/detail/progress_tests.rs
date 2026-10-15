use corral_core::{
    Checkpoint, CheckpointRecord, CheckpointUpdate, ProgressPhase, ProgressStep,
    ResourceObligation, RunOutcome, RunProgress, Status, StepState, Store, WaitObligation,
};
use corral_ops::Ctx;
use jiff::Timestamp;
use ratatui::{Terminal, backend::TestBackend, crossterm::event::KeyCode};
use unicode_width::UnicodeWidthStr;

use super::{DetailView, progress};
use crate::board::BoardView;
use crate::board::tests::{Fixture, buffer_text, key, seeded};
use crate::{HitMap, View};

const NOW: Timestamp = Timestamp::constant(1_767_269_110, 0);

fn plan(label: &str) -> RunProgress {
    RunProgress {
        revision: 2,
        spec_revision: "spec-1".into(),
        phases: vec![
            ProgressPhase {
                id: "p-read".into(),
                label: "Prepare".into(),
                steps: vec![ProgressStep {
                    id: "s-read".into(),
                    label: "Read contracts".into(),
                    status: StepState::Done,
                    criteria: vec!["A1".into()],
                }],
            },
            ProgressPhase {
                id: "p-build".into(),
                label: label.into(),
                steps: vec![
                    ProgressStep {
                        id: "s-edit".into(),
                        label: "Render progress".into(),
                        status: StepState::InProgress,
                        criteria: vec!["A1".into(), "A2".into()],
                    },
                    ProgressStep {
                        id: "s-old".into(),
                        label: "Old approach".into(),
                        status: StepState::Dropped {
                            reason: "replaced by s-edit".into(),
                        },
                        criteria: vec!["A2".into()],
                    },
                    ProgressStep {
                        id: "s-check".into(),
                        label: "Check output".into(),
                        status: StepState::Pending,
                        criteria: vec![],
                    },
                ],
            },
        ],
    }
}

fn checkpoint(progress: &RunProgress) -> CheckpointRecord {
    CheckpointRecord {
        artifact: "artifacts/checkpoint-r1.json".into(),
        checkpoint: Checkpoint {
            version: 1,
            revision: 1,
            input: CheckpointUpdate {
                task: "T-004".into(),
                run: "R1".into(),
                spec_revision: "spec-1".into(),
                expected_revision: 0,
                progress_revision: progress.revision,
                head: "1".repeat(40),
                base: "0".repeat(40),
                tree: "2".repeat(64),
                criterion_ids: vec!["A1".into()],
                evidence_refs: vec!["artifacts/render.txt".into()],
                next_action: "Inspect the rendered board".into(),
                decisions: vec!["Keep the worker?".into()],
                waits: vec![WaitObligation {
                    id: "w-check".into(),
                    owner: "R1".into(),
                    handle: "monitor://check-7".into(),
                    condition: "check exited".into(),
                    resolution: None,
                }],
                resources: vec![ResourceObligation {
                    id: "res-worker".into(),
                    owner: "R1".into(),
                    description: "fixture worker".into(),
                    cleanup: "stop worker-7".into(),
                    resolution: None,
                }],
            },
            progress: Some(progress.clone()),
            run_obligations: serde_json::from_value(serde_json::json!({
                "decisions": [{
                    "request": {
                        "version": 1, "id": "native-question", "run": "R1",
                        "question": "Which branch?", "requested_action": "Choose a branch",
                        "owner": "owner", "source": "worker", "deadline": null
                    },
                    "registered_at": NOW,
                    "observation": {"at": NOW, "state": "pending", "detail": "awaiting answer"},
                    "receipt": null
                }]
            }))
            .unwrap(),
            completed_steps: vec!["s-read".into()],
            current_step: Some("s-edit".into()),
            captured: NOW,
        },
    }
}

fn seed_progress(fx: &Fixture, label: &str) {
    let progress = plan(label);
    fx.ctx
        .store
        .update("T-004", |task| {
            let run = task.current_run_mut().unwrap();
            run.checkpoints = vec![checkpoint(&progress)];
            run.progress = Some(progress);
            Ok(())
        })
        .unwrap();
}

fn reopen(ctx: &Ctx) -> Ctx {
    Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store: Store::open(ctx.store.root()).unwrap(),
        herdr: ctx.herdr.clone(),
    }
}

fn render(view: &mut dyn View, ctx: &Ctx, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|f| view.draw(f, f.area(), ctx, &mut HitMap::default()))
        .unwrap();
    buffer_text(terminal.backend().buffer())
}

fn board(ctx: &Ctx, width: u16) -> String {
    let mut view = BoardView::with_clock(ctx, Box::new(|| NOW)).unwrap();
    render(&mut view, ctx, width, 45)
}

fn finish_fixture(fx: &Fixture) {
    fx.ctx
        .store
        .update("T-004", |task| {
            task.status = Status::Review;
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(RunOutcome::Done);
            run.ended = Some(NOW);
            Ok(())
        })
        .unwrap();
}

fn detail(ctx: &Ctx) -> DetailView {
    let mut view = DetailView::new(ctx, "T-004").unwrap();
    view.on_key(key('2'), ctx);
    view
}

#[test]
fn board_reserves_counts_before_long_phase_labels_at_narrow_widths() {
    let fx = seeded();
    let legacy = board(&fx.ctx, 160);
    seed_progress(&fx, "Implementation phase with a very long label");
    for width in [66, 96, 160, 360] {
        let text = board(&reopen(&fx.ctx), width);
        let row = text
            .lines()
            .find(|line| line.contains("1/4"))
            .unwrap_or_else(|| panic!("missing progress count at {width}\n{text}"));
        assert_eq!(row.width(), usize::from(width));
        if width == 360 {
            assert!(row.contains("Implementation phase with a very long label 1/4"));
        } else {
            assert!(row.contains("… 1/4"), "{row}");
        }
    }
    fx.ctx
        .store
        .update("T-004", |task| {
            let run = task.current_run_mut().unwrap();
            run.progress = None;
            run.checkpoints.clear();
            Ok(())
        })
        .unwrap();
    assert_eq!(board(&reopen(&fx.ctx), 160), legacy);
}

#[test]
fn progress_and_checkpoint_render_identically_after_restart_and_keep_ids_on_rename() {
    let fx = seeded();
    seed_progress(&fx, "Build");
    finish_fixture(&fx);
    let before_board = board(&fx.ctx, 160);
    let before_detail = render(&mut detail(&fx.ctx), &fx.ctx, 260, 80);
    let ctx = reopen(&fx.ctx);
    let before = ctx.store.get("T-004").unwrap().runs.remove(0);
    assert_eq!(before.progress, Some(plan("Build")));
    assert_eq!(before.checkpoints, vec![checkpoint(&plan("Build"))]);
    assert_eq!(board(&ctx, 160), before_board);
    assert_eq!(render(&mut detail(&ctx), &ctx, 260, 80), before_detail);

    ctx.store
        .update("T-004", |task| {
            let progress = task.current_run_mut().unwrap().progress.as_mut().unwrap();
            progress.phases[1].label = "Craft".into();
            progress.phases[1].steps[0].label = "Render new card".into();
            progress.revision += 1;
            Ok(())
        })
        .unwrap();
    let ctx = reopen(&ctx);
    let after = ctx.store.get("T-004").unwrap().runs.remove(0);
    let before_progress = before.progress.unwrap();
    let after_progress = after.progress.unwrap();
    let identities = |progress: &RunProgress| {
        progress
            .phases
            .iter()
            .flat_map(|phase| {
                phase.steps.iter().map(|step| {
                    (
                        phase.id.clone(),
                        step.id.clone(),
                        step.status.clone(),
                        step.criteria.clone(),
                    )
                })
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&after_progress), identities(&before_progress));
    assert_eq!(after.checkpoints, before.checkpoints);
    assert_eq!(
        board(&ctx, 160),
        before_board.replace("Build 1/4", "Craft 1/4")
    );
    let renamed = render(&mut detail(&ctx), &ctx, 260, 80);
    assert_eq!(
        renamed,
        before_detail
            .replace("Progress r2", "Progress r3")
            .replace("Build", "Craft")
            .replace("Render progress", "Render new card")
    );
}

#[test]
fn runs_detail_renders_all_statuses_criteria_and_latest_obligations() {
    let fx = seeded();
    seed_progress(&fx, "Build");
    finish_fixture(&fx);
    let ctx = reopen(&fx.ctx);
    let text = render(&mut detail(&ctx), &ctx, 260, 80);
    for expected in [
        "Prepare [p-read] · 1/1 done",
        "Build [p-build] · 0/3 done",
        "[done] s-read: Read contracts",
        "[in_progress] s-edit: Render progress",
        "[dropped] s-old: Old approach",
        "reason: replaced by s-edit",
        "criteria: A1, A2",
        "[pending] s-check: Check output",
        "Checkpoint r1 · progress r2 · artifacts/checkpoint-r1.json",
        "Next action: Inspect the rendered board",
        "Checkpoint criteria: A1",
        "Unresolved decision: Keep the worker?",
        "Wait [w-check] · owner R1 · handle monitor://check-7",
        "condition: check exited",
        "Resource [res-worker] · owner R1: fixture worker",
        "cleanup: stop worker-7",
    ] {
        assert!(
            text.contains(expected),
            "missing shipped UI {expected:?}\n{text}"
        );
    }

    // A later handoff replaces the unresolved obligations, but retains the old record.
    ctx.store
        .update("T-004", |task| {
            let run = task.current_run_mut().unwrap();
            let mut next = run.checkpoints[0].clone();
            next.checkpoint.revision = 2;
            next.checkpoint.input.next_action = "Finish".into();
            next.checkpoint.input.decisions.clear();
            next.checkpoint.input.waits.clear();
            next.checkpoint.input.resources.clear();
            run.checkpoints.push(next);
            run.progress = None;
            Ok(())
        })
        .unwrap();
    let ctx = reopen(&ctx);
    let text = render(&mut detail(&ctx), &ctx, 260, 80);
    assert!(text.contains("Checkpoint r2 · progress r2"));
    assert!(text.contains("Next action: Finish"));
    for stale in ["Progress r2", "w-check", "res-worker", "Keep the worker?"] {
        assert!(!text.contains(stale), "{text}");
    }
}

#[test]
fn narrow_detail_wraps_progress_without_losing_ids_handles_or_cleanup() {
    let fx = seeded();
    seed_progress(&fx, "Build");
    finish_fixture(&fx);
    let ctx = reopen(&fx.ctx);
    let task = ctx.store.get("T-004").unwrap();
    let run = task.current_run().unwrap();
    for width in [12, 24, 48] {
        let lines = progress::lines(run, width);
        assert!(lines.iter().all(|line| line.width() <= width));
        let mut terminal = Terminal::new(TestBackend::new(
            u16::try_from(width).unwrap(),
            u16::try_from(lines.len()).unwrap(),
        ))
        .unwrap();
        terminal
            .draw(|f| {
                f.render_widget(ratatui::widgets::Paragraph::new(lines.clone()), f.area());
            })
            .unwrap();
        let rendered = buffer_text(terminal.backend().buffer());
        let joined: String = rendered.lines().map(str::trim_end).collect();
        for token in [
            "s-edit",
            "monitor://check-7",
            "stop worker-7",
            "A1, A2",
            "native-question",
        ] {
            assert!(joined.contains(token), "{width}: {rendered}");
        }
    }
    // Exercise the actual scroll owner as well as the wrapped rows.
    let mut view = detail(&ctx);
    view.on_key(key('g'), &ctx);
    let first = render(&mut view, &ctx, 96, 24);
    assert!(first.contains("Progress r2"), "{first}");
    // PageDown updates the scroll state synchronously; no process or clock waits.
    for _ in 0..8 {
        view.on_key(
            ratatui::crossterm::event::KeyEvent::new(
                KeyCode::PageDown,
                ratatui::crossterm::event::KeyModifiers::NONE,
            ),
            &ctx,
        );
    }
    let last = render(&mut view, &ctx, 96, 24);
    assert!(last.contains("cleanup: stop worker-7"), "{last}");
}
