use super::*;
use crate::Status;

const ID: &str = "review-T-001-R1-review1";
const OWNER: &str = "scratch-reclaim";

fn reclaimed(store: &Store) -> usize {
    store
        .events()
        .unwrap()
        .iter()
        .filter(|event| event.kind == "storage_reclaimed" && event.detail["scratch"] == ID)
        .count()
}

#[test]
fn a_reclaim_that_crashed_after_deleting_completes_after_restart_with_one_event() {
    // Given a closed attempt whose reclaim began, deleted the directory and crashed before
    // recording it.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Scratch", None, Status::Review, "").unwrap();
    let dir = store
        .attempt_scratch(&AttemptScratch {
            kind: ScratchKind::ImplementationReview,
            id: ID,
            task: &task.id,
            subject: Some("R1"),
            attempt: "R1-review1",
            rel: "reviews/T-001-R1-review1",
        })
        .unwrap();
    std::fs::write(dir.join("notes.md"), "notes").unwrap();
    assert!(store.close_open_scratch(ID).unwrap());
    store
        .backend()
        .apply(Operation::Scratch(ScratchOp::BeginReclaim {
            id: ID.into(),
            from: ScratchState::Closed,
            owner: OWNER.into(),
            at: Timestamp::now(),
        }))
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    drop(store);

    // When a restarted process resumes it.
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store.reclaim_scratch_entry(ID, OWNER).unwrap(),
        Reclaimed::AlreadyAbsent
    );

    // Then the reclaim is recorded once and cannot be recorded again.
    assert_eq!(reclaimed(&store), 1);
    let rows = store.scratch_rows_in(&[ScratchState::Reclaimed]).unwrap();
    assert!(rows.iter().any(|row| row.id == ID));
    assert!(store.reclaim_scratch_entry(ID, OWNER).is_err());
    assert_eq!(reclaimed(&store), 1);
}
