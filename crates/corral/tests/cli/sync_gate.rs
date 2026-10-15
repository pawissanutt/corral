use super::*;
use corral_core::{GateResult, Run, Status, Store};
use serde_json::json;

const LEGACY: &str = include_str!("../fixtures/legacy-gate.json");

fn gate_task(ws: &Ws, status: Status, bound: bool) -> (String, PathBuf) {
    let store = Store::open(&ws.root).unwrap();
    let task = store
        .create("Gate fixture", Some("app"), status, "")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id": "R1",
        "agent": "fixture",
        "worktree": ws.root.join("app"),
        "branch": format!("corral/{}", task.id),
        "base": "main",
        "base_sha": "fixture",
        "started": "2026-10-15T09:00:00Z",
        "gate": {
            "result": "running",
            "at": "2026-10-15T09:00:00Z",
            "sha": "fixture"
        }
    }))
    .unwrap();
    store
        .update(&task.id, |t| {
            t.runs.push(run.clone());
            Ok(())
        })
        .unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    if bound {
        let identity = corral_ops::completion_check(&ctx, &task.id, "")
            .unwrap()
            .identity
            .unwrap();
        ctx.store
            .update(&task.id, |t| {
                t.current_run_mut().unwrap().gate.as_mut().unwrap().identity =
                    Some(identity.clone());
                Ok(())
            })
            .unwrap();
    }
    let path = corral_ops::gate_paths(&ctx, &task.id, "R1").result;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    (task.id, path)
}

#[test]
fn sync_gate_skips_terminal_tasks_without_touching_records() {
    // Given Done/Dropped tasks with legacy and broken current gate results.
    let ws = init_ws();
    let store = Store::open(&ws.root).unwrap();
    let mut records = Vec::new();
    for status in [Status::Done, Status::Dropped] {
        for (bound, result) in [(false, LEGACY), (true, "{")] {
            let (id, path) = gate_task(&ws, status, bound);
            std::fs::write(&path, result).unwrap();
            let task_path = store.get(&id).unwrap().path;
            let before = std::fs::read(&task_path).unwrap();
            records.push((task_path, before, path, result));
        }
    }
    // When the real CLI syncs an isolated store with no terminal available.
    let output = corral(&ws.root, &["sync"]);
    // Then obsolete records produce no errors or mutations.
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    for (task_path, before, result_path, result) in records {
        assert_eq!(std::fs::read(task_path).unwrap(), before);
        assert_eq!(std::fs::read_to_string(result_path).unwrap(), result);
    }
    println!("terminal gate records: sync succeeded; stderr empty; records unchanged");
}

#[test]
fn sync_gate_summarizes_legacy_records_once_without_accepting_passes() {
    // Given multiple unbound gates and bound results without schema_version.
    let ws = init_ws();
    let store = Store::open(&ws.root).unwrap();
    let mut ids = Vec::new();
    for status in [Status::Running, Status::Review] {
        for bound in [false, true] {
            let (id, path) = gate_task(&ws, status, bound);
            std::fs::write(path, LEGACY).unwrap();
            ids.push(id);
        }
    }
    // When sync encounters all legacy results in one run.
    let output = corral(&ws.root, &["sync"]);
    // Then it emits only one summary and cannot turn legacy passes into verified gates.
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    for id in ids {
        assert!(!stderr.contains(&id), "{stderr}");
        let task = store.get(&id).unwrap();
        assert_eq!(
            task.current_run().unwrap().gate.as_ref().unwrap().result,
            GateResult::Running
        );
        assert!(task.artifacts.is_empty());
    }
    assert!(!store.events().unwrap().iter().any(|e| e.kind == "gate"));
    println!("{stderr}");
}

#[test]
fn sync_gate_preserves_active_errors_alongside_legacy_records() {
    // Given a harmless legacy gate alongside malformed versioned active results.
    let ws = init_ws();
    let (legacy, path) = gate_task(&ws, Status::Running, false);
    std::fs::write(path, LEGACY).unwrap();
    let mut active = Vec::new();
    for (status, result) in [
        (Status::Running, "{"),
        (Status::NeedsMe, r#"{"schema_version":2}"#),
        (Status::Review, r#"{"schema_version":null}"#),
    ] {
        let (id, path) = gate_task(&ws, status, true);
        std::fs::write(path, result).unwrap();
        active.push(id);
    }
    // When sync collects all gates in one run.
    let output = corral(&ws.root, &["sync"]);
    // Then each active error remains visible beside a single legacy summary.
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), active.len() + 1, "{stderr}");
    assert!(!stderr.contains(&legacy), "{stderr}");
    for id in active {
        assert!(stderr.contains(&id), "{stderr}");
    }
    println!("{stderr}");
}
