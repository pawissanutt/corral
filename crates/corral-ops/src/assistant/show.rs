//! Bounded host readback of `assistant show`. Host startup readers (Todo ledger, planning
//! bootstrap, scratch roots, relink) never parse receipt, budget or usage rows, which grow with
//! session history. Replace those arrays with explicit counts so the readback size does not
//! depend on history. Canonical records are untouched; full `--json` is unchanged.
use serde_json::{Value, json};

/// Arrays that grow with history, as JSON pointers to their parent object and field.
const UNBOUNDED: [(&str, &str); 3] = [
    ("/container/state", "receipts"),
    ("/container/state/budget", "requests"),
    ("/usage", "entries"),
];

pub fn host_projection(mut value: Value) -> Value {
    let mut omitted = serde_json::Map::new();
    for (parent, field) in UNBOUNDED {
        let Some(object) = value.pointer_mut(parent).and_then(Value::as_object_mut) else {
            continue;
        };
        let count = match object.get(field) {
            Some(Value::Array(rows)) => rows.len(),
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as usize,
            _ => continue,
        };
        object.remove(field);
        omitted.insert(format!("{parent}/{field}"), json!(count));
    }
    if let Some(root) = value.as_object_mut() {
        root.insert(
            "host_projection".into(),
            json!({"version": 1, "omitted": omitted}),
        );
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(receipts: usize) -> Value {
        let row = json!({"request_id":"rq-0123456789abcdef0123456789abcdef","op":"todo","result":{"x":"y".repeat(512)}});
        json!({
            "session": {"id":"A-1","scratch":[], "source_generation":1, "current_generation":2}, "directory": {"path":"/w","branch":null},
            "workflow": {"phase":"ordinary","workflow":null},
            "host": {"capabilities":["todo"], "authority":"assistant"},
            "history": {"cursor":"next"},
            "container": {"version":1,"state":{
                "receipts": vec![row.clone(); receipts],
                "budget": {"requests": vec![row.clone(); receipts]},
                "todo": {"steps":[]}, "todo_requests": null,
            }},
            "usage": {"cost_usd": 1.0, "entries": vec![row; receipts]},
        })
    }

    #[test]
    fn host_projection_size_is_independent_of_receipt_history() {
        let small = host_projection(show(2));
        let large = host_projection(show(8));
        // Every field a host reader parses survives; only counts differ.
        for value in [&small, &large] {
            assert!(value.pointer("/container/state/todo").is_some());
            assert!(value.pointer("/container/state/receipts").is_none());
            assert!(value.pointer("/container/version").is_some());
            assert!(value.pointer("/workflow/phase").is_some());
            assert!(value.pointer("/session/scratch").is_some());
        }
        assert_eq!(
            large["host_projection"]["omitted"]["/container/state/receipts"],
            8
        );
        let len = |v: &Value| serde_json::to_vec(v).unwrap().len();
        println!(
            "rows: 2 and 8; output bytes: {} and {}; omitted arrays inspected: 3 and 3",
            len(&small),
            len(&large)
        );
        assert_eq!(len(&small), len(&large));
        for (original, projected) in [(show(2), &small), (show(8), &large)] {
            let mut restored = projected.clone();
            restored.as_object_mut().unwrap().remove("host_projection");
            for (parent, field) in UNBOUNDED {
                restored
                    .pointer_mut(parent)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .insert(
                        field.into(),
                        original.pointer(parent).unwrap()[field].clone(),
                    );
                assert_eq!(
                    projected["host_projection"]["omitted"][format!("{parent}/{field}")],
                    original.pointer(parent).unwrap()[field]
                        .as_array()
                        .unwrap()
                        .len()
                );
            }
            assert_eq!(restored, original);
        }
    }
}
