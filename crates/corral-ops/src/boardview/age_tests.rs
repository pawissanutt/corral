use anyhow::Result;
use corral_core::{Status, Store};

#[test]
fn card_age_uses_run_start_only_while_running() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = Store::init(root.path())?;
    let mut task = store.create("age source", None, Status::Ready, "")?;
    task.updated = "2026-11-14T12:00:00Z".parse()?;
    task.runs.push(serde_json::from_value(serde_json::json!({
        "id":"R1", "agent":"fixture", "worktree":"/unused", "branch":"fixture",
        "base":"main", "base_sha":"base", "started":"2026-11-10T00:00:00Z"
    }))?);
    for status in [Status::Ready, Status::Running, Status::Review] {
        task.status = status;
        store.fixture_put_task(&task)?;
        let head = store.head_if_exists(&task.id)?.unwrap();
        let row = super::row::compute(&store, &head)?.row;
        let expected = if status == Status::Running {
            head.current_run.as_ref().unwrap().started
        } else {
            head.updated
        };
        assert_eq!(
            row.age_since.as_deref(),
            Some(expected.to_string().as_str())
        );
    }
    task.status = Status::Running;
    task.runs.clear();
    store.fixture_put_task(&task)?;
    let head = store.head_if_exists(&task.id)?.unwrap();
    assert_eq!(
        super::row::compute(&store, &head)?.row.age_since,
        Some(head.updated.to_string())
    );
    Ok(())
}
