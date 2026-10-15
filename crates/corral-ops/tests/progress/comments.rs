use corral_core::{CommentAuthor, ResumeStatus};
use corral_ops::{comment, progress_readback, save_checkpoint};

use super::fixture::Fixture;

#[test]
fn owner_comment_keeps_checkpoint_and_resume_contract_compatible() {
    // Given a persisted portable checkpoint bound to the current task spec and tree.
    let f = Fixture::new();
    f.seed_checkpoint();
    let before = progress_readback(&f.ctx, &f.id).unwrap();
    let input = f.checkpoint();

    // When an owner note is added between checkpoint reads and writes.
    comment(
        &f.ctx,
        &f.id,
        "Keep this implementation local.",
        CommentAuthor::Owner,
        false,
    )
    .unwrap();

    // Then existing progress and checkpoints retain their contract and remain writable.
    let after = progress_readback(&f.ctx, &f.id).unwrap();
    assert_eq!(after, before);
    let task = f.task();
    let packet = corral_ops::resume_packet(&f.ctx, &task, task.current_run().unwrap()).unwrap();
    assert_eq!(packet.status, ResumeStatus::Compatible);
    assert!(packet.reasons.is_empty());
    assert_eq!(packet.spec_revision, before.spec_revision);
    assert!(save_checkpoint(&f.ctx, &f.id, &input).is_ok());
}
