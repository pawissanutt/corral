use super::tests::{budget_seeded as seeded, run};
use super::*;
use corral_core::work::{self, Counter, HotScope};

#[path = "hot_read_fixture.rs"]
mod rows;

#[test]
fn done_page_and_header_sources_are_bounded_with_terminal_and_nested_growth() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given identical open work, a one-card Done viewport, and tenfold older history.
        let mut fx = seeded();
        fx.ctx.store.config.ui.done_max = 1;
        let mut active = fx.ctx.store.get("T-004").unwrap();
        rows::grow(&mut active, 2 * factor, 2 * factor, true);
        fx.ctx.store.fixture_put_task(&active).unwrap();
        for _ in 0..10 * factor {
            let mut task = fx.ctx.store.create("Old", None, Status::Done, "").unwrap();
            task.updated = rows::at(-100);
            fx.ctx.store.fixture_put_task(&task).unwrap();
        }
        let before = work::snapshot();
        let source;
        {
            let _hot = HotScope::enter();
            let ctx = Ctx {
                store: fx.ctx.store.read_only_snapshot().unwrap(),
                ..fx.ctx.clone()
            };
            source = sources::FrameSources::load(&ctx).unwrap();
        }
        // Then only the K most recent Done heads are decoded, while the aggregate sees all.
        assert_eq!(
            source
                .heads
                .iter()
                .filter(|h| h.status == Status::Done)
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>(),
            ["T-007"]
        );
        assert_eq!(
            source
                .status_counts
                .iter()
                .filter(|c| c.status == "done")
                .map(|c| c.n)
                .sum::<u64>(),
            1 + 10 * factor as u64
        );
        for status in [Status::NeedsMe, Status::Review, Status::Ready] {
            assert_eq!(
                source
                    .status_counts
                    .iter()
                    .filter(|c| Status::parse(&c.status) == Some(status))
                    .map(|c| c.n)
                    .sum::<u64>(),
                1
            );
        }
        let report = work::snapshot().since(&before);
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        reports.push(report);
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}

#[test]
fn head_cards_and_review_waits_equal_full_task_oracles() {
    let mut seed = 329_u64;
    for _ in 0..16 {
        let fx = seeded();
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        // Given random full tasks with nested approved reviews and final obligations.
        let mut task = fx.ctx.store.get("T-004").unwrap();
        rows::grow(
            &mut task,
            1 + usize::try_from(seed % 4).unwrap(),
            1 + usize::try_from((seed >> 8) % 5).unwrap(),
            true,
        );
        task.status = [
            Status::Running,
            Status::NeedsMe,
            Status::Review,
            Status::Done,
        ][usize::try_from((seed >> 16) % 4).unwrap()];
        fx.ctx.store.fixture_put_task(&task).unwrap();
        let head = fx.ctx.store.head(&task.id).unwrap();
        let mut shell = head.to_shell_task().unwrap();
        shell.artifacts = head
            .artifacts_of(&head.current_run.as_ref().unwrap().id)
            .unwrap();
        let obligations = head
            .card_obligations(&head.current_run.as_ref().unwrap().id)
            .unwrap();
        assert_eq!(
            crate::resources::waiting_review(&task),
            crate::resources::waiting_review_head(&head)
        );
        // When the same renderer consumes bounded inputs at each supported density.
        for density in [
            corral_core::Density::Comfortable,
            corral_core::Density::Rich,
            corral_core::Density::Compact,
        ] {
            let mut config = fx.ctx.store.config.clone();
            config.board.card.fields = corral_core::CardField::ALL
                .iter()
                .map(|f| f.key().to_owned())
                .collect();
            config.board.card.density = density;
            for width in [12, 40, 100] {
                let full = cards::card_lines(
                    &task,
                    &[],
                    &config,
                    width,
                    true,
                    fx.now.get(),
                    CardExtra::default(),
                );
                let bounded = cards::card_lines(
                    &shell,
                    &[],
                    &config,
                    width,
                    true,
                    fx.now.get(),
                    CardExtra {
                        head: Some(&head),
                        obligations: Some(&obligations),
                        ..Default::default()
                    },
                );
                // Then spans, colors, text, and wrapping equal the full-task oracle.
                assert_eq!(
                    bounded,
                    full,
                    "{density:?}, {}, {width}",
                    task.status.as_str()
                );
            }
        }
    }
}

#[test]
fn head_slot_counts_include_pending_reviews_in_older_plans() {
    let fx = seeded();
    let mut task = fx.ctx.store.get("T-004").unwrap();
    rows::grow(&mut task, 4, 10, true);
    // Given old pending plan reviews and old coding-run gate/reviewer holds.
    task.plans[0].reviews[0].verdict = None;
    task.plans.last_mut().unwrap().reviews[0].verdict = None;
    let mut old = run("old", "worker");
    old.gate = Some(
        serde_json::from_value(serde_json::json!({
            "sha": "abc", "result": "running", "at": rows::at(0), "commands": []
        }))
        .unwrap(),
    );
    old.review = Some(
        serde_json::from_value(serde_json::json!({
            "reviewer": "rev", "sha": "abc", "at": rows::at(0)
        }))
        .unwrap(),
    );
    task.runs.insert(0, old);
    fx.ctx.store.fixture_put_task(&task).unwrap();
    // When slot accounting reads the head's current facts and folded history.
    let head = fx.ctx.store.head(&task.id).unwrap();
    for count_reviews in [false, true] {
        for count_gates in [false, true] {
            let limits = corral_core::Limits {
                count_reviews,
                count_gates,
                ..Default::default()
            };
            // Then every component, including uncounted work, matches the baseline.
            assert_eq!(
                SlotUsage::from_heads(std::slice::from_ref(&head), &limits),
                SlotUsage::from_tasks(std::slice::from_ref(&task), &limits)
            );
        }
    }
}

#[test]
fn reviewed_path_excludes_every_matching_keep_artifact() {
    // Given the seeded Diff and Summary records share one snapshot path.
    let fx = seeded();
    let task = fx.ctx.store.get("T-006").unwrap();
    let summary = task.artifacts.last().unwrap();
    corral_ops::toggle_reviewed(&fx.ctx, summary).unwrap();
    // When the board computes unread Keep artifacts through the path index.
    let board = fx.board();
    // Then reviewing the path excludes its Summary even when the first record is a Diff.
    assert!(!board.unreviewed.contains_key("T-006"));
}

#[path = "../../../corral-core/tests/recovery_support/mod.rs"]
mod recovery_rows;

#[test]
fn recovered_done_card_keeps_labels_with_flat_reads_over_retained_recovery_history() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given a visible Done card with one applied override, then more uncertain operations.
        let fx = seeded();
        let store = &fx.ctx.store;
        let mut first = recovery_rows::recovery(store);
        first.preview.request.target.task = "T-007".into();
        first.preview.affected_targets = vec!["T-007".into()];
        let mut applied = first.clone();
        recovery_rows::append_outcome(&mut applied, corral_core::RecoveryState::Applied);
        applied.outcomes.last_mut().unwrap().actual_effect =
            Some(applied.preview.proposed_effect.clone());
        store.save_recovery_record("T-007", &applied).unwrap();
        for i in 0..factor {
            let mut later = first.clone();
            later.id = format!("REC-later-{i}");
            recovery_rows::append_outcome(&mut later, corral_core::RecoveryState::Applying);
            recovery_rows::append_outcome(&mut later, corral_core::RecoveryState::Unknown);
            store.save_recovery_record("T-007", &later).unwrap();
        }
        // When the board loads its frame sources and renders the card from the head alone.
        let before = work::snapshot();
        let (source, lines);
        {
            let _hot = HotScope::enter();
            let ctx = Ctx {
                store: store.read_only_snapshot().unwrap(),
                ..fx.ctx.clone()
            };
            source = sources::FrameSources::load(&ctx).unwrap();
            let head = source.heads.iter().find(|h| h.id == "T-007").unwrap();
            let shell = head.to_shell_task().unwrap();
            lines = cards::card_lines(
                &shell,
                &[],
                &fx.ctx.store.config,
                40,
                false,
                fx.now.get(),
                CardExtra {
                    head: Some(head),
                    ..Default::default()
                },
            );
        }
        let report = work::snapshot().since(&before);
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        reports.push(report);
        // Then the history labels are words, and the green done check is not plain success.
        let text: String = lines.iter().map(|l| l.to_string() + "\n").collect();
        for label in [
            "[owner recovery]",
            "[check overridden]",
            "[recovery uncertain]",
        ] {
            assert!(text.contains(label), "{label} missing\n{text}");
        }
        assert!(!text.contains("[review waived]"), "{text}");
        let check = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.starts_with('✓'))
            .unwrap();
        assert_eq!(check.style.fg, Some(crate::theme::YELLOW), "{text}");
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}
