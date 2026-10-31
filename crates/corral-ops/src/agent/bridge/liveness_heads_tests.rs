//! Actual host probes and keyed health/sequence recording preserve the full-state algorithm.
use super::*;
use crate::agent::native_test_support::{self as native, data};
use corral_core::{
    agent::AgentRole,
    work::{self, Counter, HotScope},
};

#[test]
fn native_probe_and_record_match_full_with_fixed_work_and_one_loss_event() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        for case in ["lost", "older-sequence"] {
            corral_core::clock::pin_thread(Some(data::at(100_000)));
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
            let mut state = native::host(AgentRole::Worker, 2 * scale);
            let bound = state.generation_mut(999).unwrap().bound.as_mut().unwrap();
            bound.pid = std::process::id();
            bound.start_time = "not-the-kernel-start-time".into();
            run.agent_host = Some(Box::new(state.clone()));
            let container = ContainerRef::Worker {
                task: task.id.clone(),
                run: run.id.clone(),
            };
            let recipient = Recipient::Run {
                task: task.id.clone(),
                run: run.id.clone(),
            };
            store.save(&task).unwrap();
            let head = store.head(&task.id).unwrap();
            let event_count = store.events().unwrap().len();
            let expected_probe = probe(&store, &recipient, &container, &state).unwrap();
            assert!(matches!(
                expected_probe,
                Probe::Lost {
                    reason: LossReason::HostExited,
                    ..
                }
            ));
            let probe = if case == "lost" {
                expected_probe.clone()
            } else {
                Probe::Healthy {
                    generation: 1,
                    seq: 7,
                }
            };
            let mut expected = state;
            assert!(apply(&mut expected, &probe));
            if case == "lost" {
                // An actual backend event-publication fault must roll back the health edit,
                // not leave lost state without its recovery evidence. Do this before metering.
                let saved = store.get(&task.id).unwrap();
                let seq = store.view().unwrap().seq();
                store.fixture_break_events().unwrap();
                assert!(record(&store, &container, &probe).is_err());
                store.fixture_restore_events().unwrap();
                assert_eq!(store.get(&task.id).unwrap(), saved);
                assert_eq!(store.view().unwrap().seq(), seq);
            }
            let before = work::snapshot();
            let hot = HotScope::enter();
            assert_eq!(
                probe_ref(&store, &head, &recipient, &container).unwrap(),
                expected_probe
            );
            assert!(record(&store, &container, &probe).unwrap());
            assert!(!record(&store, &container, &probe).unwrap());
            let delta = work::snapshot().since(&before);
            drop(hot);
            delta.check_identity().unwrap();
            assert_eq!(delta.cells(Counter::FullTaskHydrations).total, 0);
            costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
            let actual = store.get(&task.id).unwrap();
            assert_eq!(agent::container_state(&actual, &container), Some(&expected));
            let events = store.events().unwrap();
            assert_eq!(
                events[event_count..]
                    .iter()
                    .filter(|event| event.kind == "agent_lost")
                    .count(),
                usize::from(case == "lost")
            );
        }
    }
    assert_eq!(costs[0], costs[2]);
    assert_eq!(costs[1], costs[3]);
}
