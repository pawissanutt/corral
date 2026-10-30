use super::*;
use corral_core::Store;
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

fn rows(
    task: &dyn TaskFieldsRef,
    neighborhood: &Neighborhood<'_>,
    now: Timestamp,
) -> Vec<Option<Condition>> {
    vec![
        Some(row(
            task,
            now,
            "source_wait",
            WaitClass::PmAction,
            Responsible::Pm,
            (Some("source".into()), Some(now)),
            "Source is pending".into(),
        )),
        Some(wait(
            task,
            now,
            "prerequisite",
            WaitClass::Prerequisite,
            Responsible::Executor,
            "Dependency".into(),
        )),
        Some(keyed_status(
            task,
            now,
            "owner",
            WaitClass::OwnerDecision,
            "Disposition".into(),
        )),
        authority(task, neighborhood, now, Action::Dispatch),
    ]
}

#[cfg(unix)]
#[test]
fn delivery_recipients_match_full_facade_without_settled_history_reads() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = handling_tests::fixture::Fx::new();
        let mut task = fx.questions(0);
        task.runs[0].pending_reply = Some("Owner reply".into());
        task.runs[0].inbox.next_seq = 2;
        // Main's closure contract keeps an ended-only last reviewer open. Its retained
        // header still needs assessment even though receipt collection excludes ended runs.
        task.runs[0].review_runs.push(
            serde_json::from_value(json!({
                "id":"R1-review1","agent":"fixture","worktree":"/missing-fixture-worktree",
                "branch":"corral/T","base":"main","base_sha":"abc",
                "started":handling_tests::fixture::at(0),"ended":handling_tests::fixture::at(1),
                "pending_reply":"Review reply","delivery":"inbox","inbox":{"next_seq":2}
            }))
            .unwrap(),
        );
        let mut old = task.runs[0].clone();
        old.outcome = Some(corral_core::RunOutcome::Done);
        old.ended = Some(handling_tests::fixture::at(1));
        for n in 0..2 * scale {
            old.id = format!("old-{n:06}");
            task.runs.insert(0, old.clone());
        }
        fx.ctx.store.fixture_put_task(&task).unwrap();
        let expected = recipients(&task).unwrap();
        assert_eq!(expected.len(), 1);
        let policy = fx.ctx.store.effective_operating_policy().unwrap();
        let neighborhood =
            Neighborhood::new(std::slice::from_ref(&task), &fx.ctx.store.config, &policy);
        let now = handling_tests::fixture::at(700);
        let mut legacy = Vec::new();
        delivery::conditions(&task, &neighborhood, now, &mut legacy);
        let reviewer = corral_core::inbox::Recipient::ReviewRun {
            task: task.id.clone(),
            run: "R1".into(),
            review: "R1-review1".into(),
        }
        .key()
        .unwrap();
        assert!(
            legacy
                .iter()
                .any(|condition| condition.next_action.recipient.as_deref()
                    == Some(reviewer.as_str()))
        );
        let head = fx.ctx.store.head(&task.id).unwrap();
        let _hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(recipients(&head).unwrap(), expected);
        assert_eq!(
            delivery::conditions_ref(&head, &neighborhood, now).unwrap(),
            legacy
        );
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
    }
    assert_eq!(measured[0], measured[1]);
}

#[test]
fn scalar_attention_helpers_match_full_facade_without_history_reads() {
    let at = "2026-01-01T00:00:00Z".parse().unwrap();
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        store.fixture_pin_now(at).unwrap();
        let mut task = store.create("Task", None, Status::Ready, "").unwrap();
        for _ in 0..3 * scale {
            task.comments.push(
                serde_json::from_value(json!({"author":"owner","at":at,"text":"History"})).unwrap(),
            );
        }
        store.fixture_put_task(&task).unwrap();
        let head = store.head(&task.id).unwrap();
        let policy = store.effective_operating_policy().unwrap();
        let neighborhood = Neighborhood::new(std::slice::from_ref(&task), &store.config, &policy);
        let expected = rows(&task, &neighborhood, at);
        let _hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(rows(&head, &neighborhood, at), expected);
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
    }
    assert_eq!(measured[0], measured[1]);
}
