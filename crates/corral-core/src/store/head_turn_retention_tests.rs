use super::*;
use crate::{AgentHostHead, Status, Store};
use serde_json::json;

fn turn(generation: u32, seq: u64, ended: bool) -> NativeRecord {
    NativeRecord::Turn(
        serde_json::from_value(json!({
            "generation": generation, "seq": seq, "origin": {"kind":"bridge"},
            "started": "2026-01-01T00:00:00Z",
            "ended": if ended { json!("2026-01-01T00:00:01Z") } else { json!(null) }
        }))
        .unwrap(),
    )
}

#[test]
fn oldest_ended_eviction_repairs_maxima_and_keeps_append_positions_after_reopen() {
    assert_eq!(
        exercise(1),
        exercise(2),
        "exact retention work is history-independent"
    );
}

fn exercise(extra: u64) -> Vec<u64> {
    let now = "2026-01-01T00:00:00Z".parse().unwrap();
    crate::clock::pin_thread(Some(now));
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    store.fixture_pin_now(now).unwrap();
    let limit = 2 + extra;
    let mut task = store
        .create("retention", None, Status::Running, "")
        .unwrap();
    let mut state = AgentRunState::new(
        serde_json::from_value(json!({
            "role":"worker", "store":"fixture", "project":"p", "generation":3
        }))
        .unwrap(),
    );
    for seq in 0..extra {
        turn(1, seq, false).put(&mut state);
    }
    task.runs.push(
        serde_json::from_value(json!({
            "id":"R1", "agent":"worker", "worktree":"/tmp/retention", "branch":"b",
            "base":"main", "base_sha":"a", "started":"2026-01-01T00:00:00Z",
            "agent_host": state
        }))
        .unwrap(),
    );
    store.fixture_put_task(&task).unwrap();
    let container = ContainerRef::Worker {
        task: task.id.clone(),
        run: "R1".into(),
    };
    let before = crate::work::snapshot();
    let hot = crate::work::HotScope::enter();
    store
        .update_head(&task.id, |head| {
            for record in [
                turn(1, 9, true),
                turn(1, 2, false),
                turn(2, 3, true),
                turn(2, 1, false),
            ] {
                head.put_agent_record(&container, record)?;
            }
            head.retain_agent_turns_to(&container, limit)
        })
        .unwrap();
    let delta = crate::work::snapshot().since(&before);
    drop(hot);
    delta.check_identity().unwrap();
    assert_eq!(delta.total(Counter::FullTaskHydrations), 0);
    assert_eq!(delta.total(Counter::HistoryRowDecodes), 0);
    let work = Counter::ALL.map(|counter| delta.total(counter)).to_vec();
    let head = store.head(&task.id).unwrap();
    for generation in [1, 2] {
        assert_eq!(
            head.latest_agent_turn(&container, generation)
                .unwrap()
                .unwrap()
                .seq,
            if generation == 1 { 2 } else { 1 }
        );
    }
    assert_eq!(
        head.agent_head(&container).unwrap().unwrap().records.live[3],
        limit
    );
    drop(head);
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    store
        .update_head(&task.id, |head| {
            head.put_agent_record(&container, turn(3, 1, false))?;
            head.retain_agent_turns_to(&container, limit)?; // all open: may exceed cap
            assert_eq!(
                head.agent_head(&container)?.unwrap().records.live[3],
                3 + extra
            );
            head.put_agent_record(&container, turn(2, 1, true))?;
            head.retain_agent_turns_to(&container, limit)?;
            assert!(head.overlaid()?.latest_agent_turn(&container, 2)?.is_none());
            head.put_agent_record(&container, turn(1, 9, false))?;
            head.retain_agent_turns_to(&container, limit)
        })
        .unwrap();
    let head = store.head(&task.id).unwrap();
    let counts = head.agent_head(&container).unwrap().unwrap().records;
    assert_eq!((counts.counts[3], counts.live[3]), (6 + extra, 3 + extra));
    assert_eq!(
        head.latest_agent_turn(&container, 1).unwrap().unwrap().seq,
        9
    );
    assert_eq!(
        head.latest_agent_turn_any_generation(&container)
            .unwrap()
            .unwrap()
            .generation,
        3
    );
    let full = store.get(&task.id).unwrap();
    let retained = &full.runs[0].agent_host.as_ref().unwrap().turns;
    assert_eq!(
        retained
            .iter()
            .map(|t| (t.generation, t.seq))
            .collect::<Vec<_>>(),
        (0..extra)
            .map(|seq| (1, seq))
            .chain([(1, 2), (3, 1), (1, 9)])
            .collect::<Vec<_>>()
    );
    assert_eq!(
        AgentHostHead::of(full.runs[0].agent_host.as_ref().unwrap())
            .records
            .live[3],
        3 + extra
    );
    crate::clock::pin_thread(None);
    work
}
