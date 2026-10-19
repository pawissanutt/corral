use super::*;

#[test]
fn inactive_transcript_is_evicted_after_the_next_board_refresh() {
    // Given a transcript cached by an active task that has since finished.
    let (tmp, original, _serial) = session(OMP);
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("omo"));
    let mut task = store
        .create("Usage eviction", None, Status::Running, "")
        .unwrap();
    let mut snapshot = run();
    snapshot.ended = None;
    let dir = tmp
        .path()
        .join("omo/sessions")
        .join(crate::omo::encode_cwd(&snapshot.worktree));
    fs::create_dir_all(&dir).unwrap();
    let transcript = dir.join("session.jsonl");
    fs::rename(original, &transcript).unwrap();
    task.runs.push(snapshot);
    store.save(&task).unwrap();
    refresh_active_usage(&store);
    assert!(transcript_cache::contains(&transcript));
    store
        .update(&task.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    // When the next refresh no longer references that transcript.
    refresh_active_usage(&store);
    // Then the old parsed entries are released rather than retained forever.
    assert!(!transcript_cache::contains(&transcript));
}
