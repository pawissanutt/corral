use corral_core::attention::{
    AttentionHandling, AttentionLevels, AttentionRecord, Responsible, SinceSource,
};
use corral_core::{Density, InfraWait, InfraWaitClass, Status};
use jiff::SignedDuration;
use ratatui::{Terminal, backend::TestBackend};
use unicode_width::UnicodeWidthStr;

use crate::attention;
use crate::board::tests::{Fixture, buffer_text, seeded};
use crate::{HitMap, View};

fn lifecycle() -> Fixture {
    let fx = seeded();
    let now = fx.now.get();
    for mut task in fx.ctx.store.list().unwrap() {
        task.status_since = Some(now - SignedDuration::from_secs(120));
        if task.id == "T-003" {
            task.depends_on = vec!["T-002".into()];
        }
        if task.id == "T-004" {
            // Running without a run is an unmatched state, not an invented lost pane.
            task.runs.clear();
            task.status_since = Some(now - SignedDuration::from_secs(1000));
        }
        if task.id == "T-006" {
            task.infra_wait = Some(InfraWait {
                version: 1,
                class: InfraWaitClass::Operation,
                marker: "MERGE_HEAD".into(),
                path: "/fixture/.git/MERGE_HEAD".into(),
                first_seen: now - SignedDuration::from_secs(2000),
                first_mtime: None,
                held_at: Some(now - SignedDuration::from_secs(1700)),
            });
        }
        std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();
    }
    fx
}

fn render(view: &mut dyn View, fx: &Fixture, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| view.draw(frame, frame.area(), &fx.ctx, &mut HitMap::default()))
        .unwrap();
    buffer_text(terminal.backend().buffer())
}

#[test]
fn attention_lifecycle_board_renders_only_actionable_rows_and_the_projection_count() {
    // Given a progress-free stall, a young owner decision, a held Git operation and quiet waits.
    let mut fx = lifecycle();
    let projection = corral_ops::attention::board(&fx.ctx, fx.now.get()).unwrap();
    let tasks = fx.ctx.store.list().unwrap();
    let snapshot = attention::load(&fx.ctx, fx.now.get()).unwrap();
    assert_eq!(
        projection
            .tasks
            .iter()
            .map(|row| (&row.task, &row.assessment))
            .collect::<Vec<_>>(),
        snapshot.iter().collect::<Vec<_>>(),
    );
    // Independent fixture expectation: the 1000s PM stall and 2000s owner Git hold are overdue.
    assert_eq!(
        attention::header(
            &Ok(snapshot.clone()),
            &tasks,
            &fx.ctx.store.config,
            fx.now.get()
        ),
        Some("overdue 2".into()),
    );
    for task in &tasks {
        let row = attention::card_line(
            &snapshot[&task.id],
            task,
            &fx.ctx.store.config,
            28,
            fx.now.get(),
        );
        assert_eq!(
            row.is_some(),
            ["T-004", "T-005", "T-006"].contains(&task.id.as_str()),
            "{}: {:?}",
            task.id,
            snapshot[&task.id],
        );
        if let Some(row) = row {
            assert!(row.width() <= 28);
        }
    }
    // When the real board renders both card densities at narrow and wide terminal sizes.
    for density in [Density::Comfortable, Density::Compact] {
        fx.ctx.store.config.board.card.density = density;
        let mut board = fx.board();
        for (width, height) in [(80, 24), (160, 45)] {
            let screen = render(&mut board, &fx, width, height);
            println!("ATTENTION BOARD {density:?} {width}x{height}\n{screen}");
            // Then the count is independent of card visibility and every rendered row fits.
            assert!(screen.contains("overdue 2"), "{screen}");
            assert!(screen.contains("Stall"), "{screen}");
            assert!(
                screen
                    .lines()
                    .all(|line| line.width() <= usize::from(width))
            );
            if density == Density::Compact && width == 160 {
                assert!(screen.contains("Witness s"), "{screen}");
                let title = screen
                    .lines()
                    .position(|line| line.contains("Witness s"))
                    .unwrap();
                let attention = screen
                    .lines()
                    .position(|line| line.contains("Stall"))
                    .unwrap();
                assert!(title < attention, "{screen}");
            }
        }
    }
}

#[test]
fn attention_waiting_lists_primary_first_with_all_resume_and_handling_evidence() {
    // Given a task with independent owner and infrastructure conditions.
    let fx = lifecycle();
    let now = fx.now.get();
    let snapshot = attention::load(&fx.ctx, now).unwrap();
    let assessment = &snapshot["T-006"];
    assert!(assessment.conditions.len() >= 2, "{assessment:?}");
    let key = assessment
        .conditions
        .iter()
        .find(|row| row.kind == "infra_hold")
        .unwrap()
        .key
        .clone()
        .unwrap();
    fx.ctx
        .store
        .update_attention("T-006", |ledger| {
            ledger.records.insert(
                key.clone(),
                AttentionRecord {
                    kind: "infra_hold".into(),
                    responsible: Responsible::Owner,
                    since: now - SignedDuration::from_secs(2000),
                    since_source: SinceSource::Canonical,
                    first_recorded: now,
                    opened_at: now,
                    pm_live_at_open: None,
                    levels: AttentionLevels::default(),
                    handling: Some(AttentionHandling {
                        revision: 1,
                        request: "fixture-handling".into(),
                        note: "Checking the Git operation".into(),
                        at: now,
                        next_check: now + SignedDuration::from_secs(1800),
                        by: Responsible::Owner,
                    }),
                    first_handled_at: Some(now),
                    resolved_at: None,
                },
            );
            Ok(())
        })
        .unwrap();
    let task = fx.ctx.store.get("T-006").unwrap();
    // When the shared projection is presented as the detail Waiting block.
    let lines = attention::waiting(&task, &Ok(snapshot.clone()), now);
    let text = lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    println!("ATTENTION WAITING\n{text}");
    // Then primary order never hides secondary rows or their evidence.
    assert_eq!(
        lines[1].to_string(),
        format!(
            "{} - {}",
            assessment.primary.as_ref().unwrap().class.label(),
            assessment.primary.as_ref().unwrap().responsible.label()
        ),
    );
    for condition in &assessment.conditions {
        assert!(text.contains(&condition.reason), "{text}");
        for resume in &condition.resume {
            assert!(text.contains(&resume.description), "{text}");
        }
        if let Some(command) = &condition.next_action.command {
            assert!(text.contains(command), "{text}");
        }
    }
    assert!(text.contains("Resume [false]"), "{text}");
    assert!(text.contains("canonical, age"), "{text}");
    assert!(
        text.contains("Checking the Git operation | next_check"),
        "{text}"
    );
    assert_eq!(
        attention::header(
            &Ok(snapshot),
            &fx.ctx.store.list().unwrap(),
            &fx.ctx.store.config,
            now
        ),
        Some("overdue 1".into()),
    );
    let mut detail = crate::detail::DetailView::new(&fx.ctx, "T-006").unwrap();
    let screen = render(&mut detail, &fx, 120, 45);
    println!("ATTENTION DETAIL MULTIPLE CONDITIONS\n{screen}");
    assert!(screen.contains("Waiting"), "{screen}");
    assert!(screen.contains("External operation"), "{screen}");
    assert!(screen.contains("Checking the Git operation"), "{screen}");
    assert!(screen.contains("next_check"), "{screen}");
}

#[test]
fn attention_projection_decode_failure_stays_unavailable_in_board_and_detail() {
    // Given already-open views and an invalid attention ledger on a different task.
    let fx = lifecycle();
    let mut board = fx.board();
    let mut detail = crate::detail::DetailView::new(&fx.ctx, "T-006").unwrap();
    let task = fx.ctx.store.get("T-002").unwrap();
    let raw = corral_core::render_task(&task);
    std::fs::write(
        &task.path,
        raw.replacen("task:\n", "task:\n  attention: [invalid-ledger]\n", 1),
    )
    .unwrap();
    // When the views refresh from the broken canonical record.
    board.refresh(&fx.ctx);
    detail.refresh(&fx.ctx);
    // Then neither view turns the failure into an empty or zero-count assessment.
    for (name, view) in [
        ("BOARD", &mut board as &mut dyn View),
        ("DETAIL", &mut detail as &mut dyn View),
    ] {
        let screen = render(view, &fx, 120, 45);
        println!("ATTENTION UNAVAILABLE {name}\n{screen}");
        assert!(screen.contains("attention unavailable"), "{screen}");
        assert!(!screen.contains("No waits."), "{screen}");
        assert!(!screen.contains("overdue 0"), "{screen}");
    }
}

#[test]
fn attention_paint_does_not_read_store_or_hide_zero_count_failures() {
    // Given a loaded board and a zero-overdue projection.
    let fx = seeded();
    let mut board = fx.board();
    let before = corral_core::work::Work::ALL.map(corral_core::work::count);
    // When the board paints repeatedly without a refresh.
    for _ in 0..3 {
        render(&mut board, &fx, 160, 45);
    }
    // Then painting does not classify again or read store records.
    let after = corral_core::work::Work::ALL.map(corral_core::work::count);
    assert_eq!(before, after);
    assert_eq!(
        attention::header(
            &Ok(Default::default()),
            &[],
            &fx.ctx.store.config,
            fx.now.get()
        ),
        None,
    );
}

#[test]
fn attention_boundary_and_discovered_ages_remain_honest() {
    // Given an unmatched legacy task with no recorded discovery time.
    let fx = lifecycle();
    let mut task = fx.ctx.store.get("T-004").unwrap();
    task.status_since = None;
    let tasks = vec![task.clone()];
    let policy = fx.ctx.store.effective_operating_policy().unwrap();
    let neighborhood =
        corral_ops::attention::Neighborhood::new(&tasks, &fx.ctx.store.config, &policy);
    let assessment = corral_ops::attention::assess(&task, &neighborhood, fx.now.get());
    // When its card renders before reconciliation has recorded a discovery base.
    let line =
        attention::card_line(&assessment, &task, &fx.ctx.store.config, 100, fx.now.get()).unwrap();
    // Then no age is fabricated from the most recent unrelated task write.
    assert!(line.to_string().contains("Stall unknown"));
    assert_eq!(
        assessment.primary.unwrap().since_source,
        SinceSource::Discovered
    );
    assert_eq!(task.status, Status::Running);
}

#[test]
fn attention_refresh_keeps_warm_decodes_and_active_work_independent_of_settled_history() {
    // Given 1x and 10x settled history with identical active conditions.
    let mut visits = Vec::new();
    for history in [10, 100] {
        let mut fx = lifecycle();
        for _ in 0..history {
            fx.ctx
                .store
                .create("Settled history", None, Status::Done, "")
                .unwrap();
        }
        fx.ctx.store = corral_core::Store::open(fx.ctx.store.root()).unwrap();
        fx.ctx
            .store
            .set_task_settle_window(std::time::Duration::ZERO);
        let before = corral_core::work::Work::ALL.map(corral_core::work::count);
        // When cold and warm refreshes project the same unchanged records.
        let cold = attention::load(&fx.ctx, fx.now.get()).unwrap();
        let after_cold = corral_core::work::Work::ALL.map(corral_core::work::count);
        let warm = attention::load(&fx.ctx, fx.now.get()).unwrap();
        let after_warm = corral_core::work::Work::ALL.map(corral_core::work::count);
        // Then settled records add cold decodes only, never active classification or warm decodes.
        assert_eq!(cold, warm);
        for (index, work) in corral_core::work::Work::ALL.into_iter().enumerate() {
            let cold = after_cold[index] - before[index];
            let warm = after_warm[index] - after_cold[index];
            println!(
                "ATTENTION WORK history={history} {} cold={cold} warm={warm}",
                work.name()
            );
            if matches!(
                work,
                corral_core::work::Work::TaskRead | corral_core::work::Work::TaskParse
            ) {
                assert_eq!(warm, 0, "{work:?}");
            }
            if work == corral_core::work::Work::AttentionVisit {
                assert_eq!(cold, warm);
                visits.push(warm);
            }
        }
    }
    assert_eq!(visits, [6, 6]);
}
