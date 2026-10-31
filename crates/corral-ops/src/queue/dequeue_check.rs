use super::*;

pub(super) fn check_dequeue(task: &Task) -> Result<()> {
    if !matches!(task.status, Status::Idea | Status::Backlog | Status::Ready) {
        bail!(
            "{} is {}; dequeue only applies to Idea, Backlog or Ready work",
            task.id,
            task.status.label()
        );
    }
    Ok(())
}
