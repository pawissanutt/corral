// Invoked by fake-engine hooks at the exact interleaving boundary; each returns at once
// when the test harness runs it without its hook environment.

/// Stores the staged task document at `CORRAL_TASK_FIXTURE`, then consumes it.
#[test]
fn put_task() {
    let Some(path) = std::env::var_os("CORRAL_TASK_FIXTURE") else {
        return;
    };
    let root = std::env::var_os("CORRAL_ROOT").unwrap();
    let store = corral_core::Store::open_unobserved(std::path::Path::new(&root)).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let task = corral_core::Store::parse_task_markdown(&text).unwrap();
    store.fixture_put_task(&task).unwrap();
    std::fs::remove_file(path).unwrap();
}

/// Changes only the `kind` of task `CORRAL_TASK_ID` as it is stored right now.
#[test]
fn set_kind() {
    let (Some(id), Some(kind)) = (
        std::env::var_os("CORRAL_TASK_ID"),
        std::env::var_os("CORRAL_TASK_KIND"),
    ) else {
        return;
    };
    let root = std::env::var_os("CORRAL_ROOT").unwrap();
    let store = corral_core::Store::open_unobserved(std::path::Path::new(&root)).unwrap();
    let mut task = store.get(id.to_str().unwrap()).unwrap();
    task.kind = Some(kind.to_str().unwrap().into());
    store.fixture_put_task(&task).unwrap();
}
