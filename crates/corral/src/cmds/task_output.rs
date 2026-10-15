use corral_core::Task;

pub(super) fn task_json(t: &Task, with_body: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("path".into(), t.path.to_string_lossy().into_owned().into());
        if with_body {
            o.insert("body".into(), t.body.clone().into());
        }
    }
    v
}
