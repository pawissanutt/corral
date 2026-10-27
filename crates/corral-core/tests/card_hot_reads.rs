mod store_support;

use corral_core::work::{self, Counter, HotScope};
use corral_core::{CardField, HistorySummary, ResumeDelivery, Status, TaskCost, Usage, WaitState};
use store_support::*;

#[test]
fn card_fields_match_full_tasks_over_random_histories() {
    let (_dir, store) = store();
    let mut seed = 0x329_u64;
    for _ in 0..32 {
        // Given independently varied top-level and nested histories.
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let runs = 1 + usize::try_from(seed % 5).unwrap();
        let nested = 1 + usize::try_from((seed >> 8) % 7).unwrap();
        let mut task = store.create("Card", None, Status::Running, "Spec").unwrap();
        grow(&mut task, runs, nested + 1, 2, nested);
        task.status = if seed.is_multiple_of(2) {
            Status::NeedsMe
        } else {
            Status::Review
        };
        for (i, run) in task.runs.iter_mut().enumerate() {
            if i.is_multiple_of(2) {
                run.usage = Some(Usage::default());
            }
        }
        store.fixture_put_task(&task).unwrap();
        let full = store.get(&task.id).unwrap();
        // When the card reads only the head and the current run's artifact page.
        let before = work::snapshot();
        let _hot = HotScope::enter();
        let head = store.head(&task.id).unwrap();
        let artifacts = head
            .artifacts_of(&head.current_run.as_ref().unwrap().id)
            .unwrap();
        let shell = head.to_shell_task().unwrap();
        assert_eq!(shell.body, full.body);
        assert_eq!(shell.runs.len(), 1);
        assert_eq!(shell.runs[0].review_runs.len(), 1);
        assert!(shell.runs[0].checkpoints.is_empty());
        assert!(shell.runs[0].switches.is_empty());
        assert!(shell.runs[0].obligations.is_empty());
        assert!(
            shell.runs[0]
                .usage
                .as_ref()
                .is_none_or(|u| u.entries.is_empty())
        );
        assert_eq!(shell.plan_runs.len(), 1);
        assert_eq!(shell.plans[0].reviews.len(), 1);
        assert_eq!(shell.aggregate_attempts[0].reviews.len(), 1);
        assert!(
            shell.comments.is_empty()
                && shell.artifacts.is_empty()
                && shell.approved_scope_history.is_empty()
        );
        // Then every catalog value and formatted cost equal the full-task oracle.
        for field in CardField::ALL {
            assert_eq!(
                field.text_head_with(&head, &artifacts, &|_| false, at(9000), true),
                field.text_with(&full, &|_| false, at(9000), true),
                "{field:?}, seed {seed}"
            );
        }
        let expected = TaskCost::of(&full);
        let actual = TaskCost::of_head(&head);
        assert!((actual.total() - expected.total()).abs() < 1e-10);
        assert_eq!(actual.text(), expected.text());
        assert_eq!(actual.unpriced_tokens, expected.unpriced_tokens);
        assert_eq!(head.summary, HistorySummary::of(&full).unwrap());
        let delta = work::snapshot().since(&before);
        assert_eq!(delta.unattributed_hydrations(), 0);
        delta.check_identity().unwrap();
    }
}

#[test]
fn card_tokens_preserve_zero_usage_in_earlier_runs() {
    // Given zero usage in an earlier run and no usage in the current one.
    let (_dir, store) = store();
    let mut task = store.create("Zero", None, Status::Running, "").unwrap();
    task.runs = vec![
        coding_run(&task.id, "R1", 0, 0, 0, true),
        coding_run(&task.id, "R2", 10, 0, 0, false),
    ];
    task.runs[0].usage = Some(Usage::default());
    task.runs[1].usage = None;
    store.fixture_put_task(&task).unwrap();
    // When tokens are rendered from summary and facts.
    let head = store.head(&task.id).unwrap();
    // Then presence is retained even though every token counter is zero.
    assert_eq!(
        CardField::Tokens.text_head_with(&head, &[], &|_| false, at(0), false),
        Some("0 tok".into())
    );
}

#[test]
fn card_obligation_page_matches_full_predicate_including_uncertain_receipts() {
    let (_dir, store) = store();
    let mut task = store.create("Wait", None, Status::Running, "").unwrap();
    let mut run = coding_run(&task.id, "R1", 0, 0, 4, false);
    // Given acknowledged history, a Ready wait with an unknown receipt, and an answered
    // DeliveryUnknown decision with a receipt.
    run.obligations.waits[0].receipt.as_mut().unwrap().delivery = ResumeDelivery::Unknown;
    run.obligations.decisions[0].observation.state = WaitState::DeliveryUnknown;
    task.runs = vec![run.clone()];
    store.fixture_put_task(&task).unwrap();
    let mut expected = run.obligations.clone();
    expected.waits.retain(|w| {
        !(w.observation.state == WaitState::Cancelled
            || w.observation.state == WaitState::Ready
                && w.receipt
                    .as_ref()
                    .is_some_and(|r| r.delivery == ResumeDelivery::Acknowledged))
    });
    expected
        .decisions
        .retain(|d| d.pending() || d.observation.state == WaitState::DeliveryUnknown);
    // When the card-specific page is read.
    let head = store.head(&task.id).unwrap();
    // Then it contains exactly the rows the baseline renderer can show, in original order.
    assert_eq!(head.card_obligations("R1").unwrap(), expected);
    assert_ne!(head.open_obligations("R1").unwrap(), expected);
    // Keyed edits maintain the same card index as full writes.
    store
        .update_head(&task.id, |head| {
            head.edit_wait("R1", &run.obligations.waits[0].request.id, |w| {
                w.receipt.as_mut().unwrap().delivery = ResumeDelivery::Acknowledged;
                Ok(())
            })
        })
        .unwrap();
    expected.waits.remove(0);
    assert_eq!(
        store
            .head(&task.id)
            .unwrap()
            .card_obligations("R1")
            .unwrap(),
        expected
    );
}

#[test]
fn card_reads_have_identical_work_with_tenfold_nested_history() {
    let mut reports = Vec::new();
    for nested in [2, 20] {
        // Given the same current inputs with additional terminal nested records.
        let (_dir, store) = store();
        let mut task = store.create("Bounded", None, Status::Running, "").unwrap();
        grow(&mut task, 2, nested + 1, 2, nested);
        task.created = at(0);
        task.updated = at(0);
        // Hold the open question/wait identities and checkpoint metadata equal.
        for run in &mut task.runs {
            let last = run.obligations.waits.last_mut().unwrap();
            last.request.id = "W-open".into();
            last.registered_at = at(100);
            last.observation.at = at(100);
            let last = run.obligations.decisions.last_mut().unwrap();
            last.request.id = "D-open".into();
            last.registered_at = at(100);
            last.observation.at = at(100);
        }
        store.fixture_put_task(&task).unwrap();
        let before = work::snapshot();
        {
            let _hot = HotScope::enter();
            let head = store.head(&task.id).unwrap();
            let _cost = TaskCost::of_head(&head);
            let _tokens = CardField::Tokens.text_head_with(&head, &[], &|_| false, at(100), false);
            let _obligations = head
                .card_obligations(&head.current_run.as_ref().unwrap().id)
                .unwrap();
        }
        let report = work::snapshot().since(&before);
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        reports.push(report);
    }
    // Then unrelated nested rows are neither decoded nor hydrated.
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}
