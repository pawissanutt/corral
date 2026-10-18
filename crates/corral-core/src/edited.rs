//! Bounded spec-only history; runtime changes have their own events.

use std::borrow::Cow;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::Task;

pub(crate) fn fields(before: &Task, after: &Task) -> Map<String, Value> {
    let mut fields = Map::new();
    for ((name, from), (_, to)) in values(before).into_iter().zip(values(after)) {
        // Compare complete values before truncating so suffix-only edits survive.
        if from != to {
            fields.insert(
                name.into(),
                json!({"from": bounded(from), "to": bounded(to)}),
            );
        }
    }
    // render_task adds a final newline. Compare that persisted representation so
    // saving a value returned by create/save does not invent a body edit.
    let from = rendered_body(&before.body);
    let to = rendered_body(&after.body);
    if from != to {
        fields.insert(
            "body".into(),
            json!({
                "lines_from": from.lines().count(),
                "lines_to": to.lines().count(),
                "sha_from": short_sha(&from),
                "sha_to": short_sha(&to),
            }),
        );
    }
    fields
}

fn values(task: &Task) -> [(&'static str, Value); 9] {
    [
        ("title", json!(task.title)),
        ("project", json!(task.project)),
        ("priority", json!(task.priority)),
        ("depends", json!(task.depends_on)),
        ("labels", json!(task.labels)),
        ("kind", json!(task.kind)),
        ("size", json!(task.size)),
        ("profile", json!(task.profile)),
        ("design", json!(task.design)),
    ]
}

fn bounded(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(text.chars().take(80).collect()),
        Value::Array(values) => Value::Array(values.into_iter().map(bounded).collect()),
        other => other,
    }
}

fn rendered_body(body: &str) -> Cow<'_, str> {
    if body.is_empty() || body.ends_with('\n') {
        Cow::Borrowed(body)
    } else {
        Cow::Owned(format!("{body}\n"))
    }
}

fn short_sha(text: &str) -> String {
    let mut sha = format!("{:x}", Sha256::digest(text.as_bytes()));
    sha.truncate(12);
    sha
}
