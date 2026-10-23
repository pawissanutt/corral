use super::*;
use crate::{STORE_SCHEMA_VERSION, UPDATE_NOTICE};

#[test]
fn replacing_executable_refuses_mutations_and_reports_once() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    let executable = tmp.path().join("corral");
    fs::write(&executable, "old executable").unwrap();
    store.executable = Arc::new(Executable::fixture(&executable).unwrap());
    let task = store
        .create("Before update", None, Status::Ready, "")
        .unwrap();
    let replacement = tmp.path().join("new-corral");
    fs::write(&replacement, "new executable").unwrap();
    let modified = fs::metadata(&executable).unwrap().modified().unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(
        fs::metadata(&executable).unwrap().len(),
        fs::metadata(&replacement).unwrap().len()
    );
    // When
    fs::rename(&replacement, &executable).unwrap();
    // Then
    assert_eq!(
        store.check_runtime().unwrap_err().to_string(),
        UPDATE_NOTICE
    );
    assert!(store.save(&task).is_err());
    assert!(store.check_runtime().is_err());
    let events = store.events().unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == "version_skew").count(),
        1
    );
    assert_eq!(events.last().unwrap().detail["pid"], std::process::id());
}

#[test]
fn deleted_executable_is_skew_even_before_a_replacement_exists() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let executable = tmp.path().join("corral");
    fs::write(&executable, "executable").unwrap();
    let guard = Executable::fixture(&executable).unwrap();
    // When
    fs::remove_file(&executable).unwrap();
    // Then
    assert!(guard.check().is_err());
}

#[test]
fn new_task_envelope_is_rejected_by_an_old_flat_collector() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("New record", None, Status::Ready, "").unwrap();
    let text = render_task(&task);
    let yaml = text.split("---\n").nth(1).unwrap();
    // When: the pre-upgrade collector's exact decoding boundary.
    let legacy = serde_yaml_ng::from_str::<Task>(yaml);
    // Then: no legacy collector can advance a receipt from this record.
    assert!(legacy.is_err());
    assert_eq!(parse_task(&text, &task.path).unwrap(), task);
}

#[test]
fn unknown_task_schema_is_not_decoded_as_legacy() {
    // Given
    let yaml = "---\nschema_version: 999\ntask: {}\n---\n";
    // When
    let result = parse_task(yaml, Path::new("T-001.md"));
    // Then
    assert!(result.is_err());
}

#[test]
fn open_writer_refuses_schema_upgraded_by_another_process_at_check_runtime() {
    use crate::work::{Work, count};
    // Given an already-open writer of the current schema.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Existing task", None, Status::Ready, "")
        .unwrap();
    assert_eq!(store.get(&task.id).unwrap(), task);
    // When another process upgrades the store schema.
    fs::write(store.state_dir().join("schema-version"), "999\n").unwrap();
    // Then reads trust open and check_runtime, without re-reading the marker...
    let reads = count(Work::SchemaRead);
    assert_eq!(store.list().unwrap(), vec![task.clone()]);
    assert_eq!(store.get(&task.id).unwrap(), task);
    assert_eq!(count(Work::SchemaRead), reads);
    // ...while the next runtime check and every protected mutation refuse it.
    assert!(Store::open(tmp.path()).is_err());
    assert!(store.check_runtime().is_err());
    assert!(store.save(&task).is_err());
    assert!(store.check_runtime().is_err());
    assert_eq!(store.events().unwrap().last().unwrap().kind, "version_skew");
    assert_eq!(
        fs::read_to_string(store.state_dir().join("schema-version")).unwrap(),
        "999\n"
    );
}

#[test]
fn incompatible_older_store_schema_is_refused() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    // When
    fs::write(store.state_dir().join("schema-version"), "1\n").unwrap();
    // Then
    assert_ne!(STORE_SCHEMA_VERSION, 1);
    assert!(Store::open(tmp.path()).is_err());
}

#[test]
fn protected_discovery_stops_at_worktree_boundary() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    let worktree = tmp.path().join(".worktrees/T-001");
    let nested = worktree.join("crates/test");
    fs::create_dir_all(&nested).unwrap();
    // When
    let restricted = Store::find_ancestor(&nested, true);
    // Then
    assert!(restricted.is_err());
    assert_eq!(
        Store::find_ancestor(&nested, false).unwrap(),
        tmp.path().canonicalize().unwrap()
    );
}

#[test]
fn protected_discovery_can_find_a_worktree_local_fixture_store() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    let worktree = tmp.path().join(".worktrees/T-001");
    let local = Store::init(&worktree).unwrap();
    let nested = worktree.join("tests");
    fs::create_dir_all(&nested).unwrap();
    // When
    let root = Store::find_ancestor(&nested, true).unwrap();
    // Then
    assert_eq!(root, local.root());
}
