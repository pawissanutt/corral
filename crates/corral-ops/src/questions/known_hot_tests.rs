//! PM Q8/T-623: preserved replay is a named exception, every cell checked from canonical docs.
use super::*;
use crate::agent::native_test_support::data;
use corral_core::agent::AgentRole;
use corral_core::backend::{TaskDoc, TaskNum, TaskPart, TaskParts};
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use std::collections::BTreeMap;
type Cost = BTreeMap<Counter, u64>;
fn bytes(doc: &serde_json::Value) -> u64 {
    serde_json::to_vec(doc).unwrap().len() as u64
}

// Independent expectation from stored documents, not a measured/learned counter baseline.
fn replay_cost(doc: &TaskDoc, events: &[corral_core::Event]) -> Cost {
    let mut expected: Cost = Counter::ALL
        .into_iter()
        .map(|counter| (counter, 0))
        .collect();
    expected.insert(Counter::FullTaskHydrations, 1);
    expected.insert(Counter::ViewsOpened, 2);
    expected.insert(Counter::TaskHeadDecodes, 1);
    expected.insert(Counter::EventDecodes, events.len() as u64);
    let mut doc_bytes = bytes(&doc.head.doc) + bytes(&doc.head.summary);
    for row in &doc.rows {
        let counter = match row.part {
            TaskPart::Run if row.current => Counter::RunDecodes,
            TaskPart::PlanRun | TaskPart::Plan | TaskPart::AggregateAttempt if row.current => {
                Counter::TaskHeadDecodes
            }
            TaskPart::Run
            | TaskPart::PlanRun
            | TaskPart::Plan
            | TaskPart::AggregateAttempt
            | TaskPart::Comment
            | TaskPart::Artifact
            | TaskPart::ScopeRevision
            | TaskPart::PmReceipt
            | TaskPart::MemberGateHistory
            | TaskPart::Recovery
            | TaskPart::RecoveryOutcome
            | TaskPart::PaneRef
            | TaskPart::Finding
            | TaskPart::Prompt
            | TaskPart::GateAttempt
            | TaskPart::ReviewEvidence => Counter::HistoryRowDecodes,
            _ => Counter::NestedRowDecodes,
        };
        *expected.get_mut(&counter).unwrap() += 1;
        doc_bytes += bytes(&row.doc);
    }
    doc_bytes += events.iter().map(|event| bytes(&event.detail)).sum::<u64>();
    expected.insert(Counter::DecodedDocBytes, doc_bytes);
    expected
}
/// Replay reads only the task's own question events, never unrelated history.
fn question_events(events: &[corral_core::Event], task: &str) -> Vec<corral_core::Event> {
    events
        .iter()
        .filter(|e| e.task.as_deref() == Some(task))
        .filter(|e| QUESTION_EVENT_KINDS.contains(&e.kind.as_str()))
        .cloned()
        .collect()
}

fn assert_site(delta: &WorkSnapshot, site: &str, expected: &Cost, multiple: u64) {
    assert_eq!(delta.followup_calls("T-623", site), multiple);
    for counter in Counter::ALL {
        let actual = delta
            .cells(counter)
            .followup
            .get(&("T-623".into(), site.into()))
            .copied()
            .unwrap_or(0);
        assert_eq!(
            actual,
            multiple * expected[&counter],
            "{site} {}",
            counter.name()
        );
    }
}

#[test]
fn t623_question_event_replay_has_exact_visible_cost_and_no_unattributed_work() {
    for scale in [1, 10] {
        let (_dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        let run = task.runs.last_mut().unwrap();
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        run.agent_host = Some(super::heads_tests::questions(
            &container,
            AgentRole::Worker,
            2 * scale,
        ));
        store.fixture_put_task(&task).unwrap();
        assert_eq!(reconcile_events(&store, &task.id).unwrap(), 2 * scale + 1);
        for n in 0..2 * scale {
            store
                .log_event(None, "unrelated_fixture_event", serde_json::json!({"n":n}))
                .unwrap();
        }
        let events = store.events().unwrap();
        let doc = store
            .view()
            .unwrap()
            .task(TaskNum(1), &TaskParts::All)
            .unwrap()
            .unwrap();
        let expected = replay_cost(&doc, &question_events(&events, &task.id));
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(
            reconcile_events(&store, &task.id).unwrap(),
            0,
            "replay writes no duplicate events"
        );
        let delta = work::snapshot().since(&before);
        delta.check_identity().unwrap();
        drop(hot);
        assert_eq!(delta.followup_calls.len(), 1);
        assert_site(&delta, "question_event_replay", &expected, 1);
        for counter in Counter::ALL {
            let cells = delta.cells(counter);
            assert_eq!(cells.unattributed, 0, "{}", counter.name());
            assert_eq!((cells.own_record, cells.emitted), (0, 0));
            assert_eq!(cells.total, expected[&counter]);
        }
        assert_eq!(store.events().unwrap(), events);
        eprintln!("KNOWN HOT EXCEPTION T-623 scale={scale}: {expected:?}");
    }
}

#[test]
fn automatic_deadline_head_transition_keeps_only_exact_t623_replay_work_history_proportional() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        task.status = corral_core::Status::Running;
        let run = task.runs.last_mut().unwrap();
        run.outcome = None;
        run.review = None;
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let mut state = super::heads_tests::questions(&container, AgentRole::Worker, 2 * scale);
        let question = state.questions.last_mut().unwrap();
        let key = question.key.clone();
        question.deadline=Some(serde_json::from_value(serde_json::json!({"set_by":"owner","at":data::at(1),"for_revision":1,"instruction":"Use fixed payload."})).unwrap());
        run.agent_host = Some(state);
        store.fixture_put_task(&task).unwrap();
        reconcile_events(&store, &task.id).unwrap();
        for n in 0..2 * scale {
            store
                .log_event(None, "unrelated_fixture_event", serde_json::json!({"n":n}))
                .unwrap();
        }
        let ctx = Ctx::from_store(store);
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(
            expire_deadlines(&ctx, &task.id, data::at(100_000)).unwrap(),
            vec![key.clone()]
        );
        assert!(
            expire_deadlines(&ctx, &task.id, data::at(100_000))
                .unwrap()
                .is_empty()
        );
        let delta = work::snapshot().since(&before);
        delta.check_identity().unwrap();
        drop(hot);
        let doc = ctx
            .store
            .view()
            .unwrap()
            .task(TaskNum(1), &TaskParts::All)
            .unwrap()
            .unwrap();
        let events = ctx.store.events().unwrap();
        assert_eq!(events.last().unwrap().kind, "question_expired");
        let replay = replay_cost(&doc, &question_events(&events, &task.id));
        let by_key = replay_cost(
            &doc,
            &question_events(&events[..events.len() - 1], &task.id),
        );
        assert_eq!(delta.followup_calls.len(), 2);
        assert_site(&delta, "question_event_by_key", &by_key, 1);
        assert_site(&delta, "question_event_replay", &replay, 2);
        assert_eq!(delta.unattributed_hydrations(), 0);
        measured.push(Counter::ALL.map(|counter| delta.get(counter)));
        for counter in Counter::ALL {
            let cells = delta.cells(counter);
            assert_eq!((cells.own_record, cells.emitted), (0, 0));
            assert_eq!(
                cells.total,
                cells.unattributed + by_key[&counter] + 2 * replay[&counter]
            );
        }
        let question = ctx
            .store
            .head(&task.id)
            .unwrap()
            .agent_question(&container, &key)
            .unwrap()
            .unwrap();
        assert_eq!(question.state, QuestionState::Expired);
        let messages = corral_core::inbox::Inbox::new(&ctx.store, recipient(&container).unwrap())
            .unwrap()
            .messages()
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, "question_answer");
    }
    assert_eq!(
        measured[0], measured[1],
        "unattributed deadline work is fixed; only named T-623 cells grow"
    );
}
