//! Adding an unstarted task to a tracker Plan's required members. Membership is part of the
//! Plan's operating contract, so grants and member runs bound to the old contract are refused
//! afterwards; the caller decides which member states may be stranded.
use anyhow::{Result, bail, ensure};

use super::Store;
use crate::{Status, Task};

impl Store {
    /// Make `member` a required member of `tracker`, keeping a decomposed Plan's design children
    /// equal to the tracker's. `validate` checks the fresh tracker and member under the board lock
    /// and must not write. Writes go member, design, tracker, and each is skipped when already
    /// made, so rerunning after a crash completes the addition once.
    ///
    /// # Errors
    /// Rejects non-trackers, existing members, trackers or parents as members, members of
    /// another project or owner, an incomplete decomposition publication, a failed validation
    /// and persistence failures.
    pub fn add_plan_member(
        &self,
        tracker: &str,
        member: &str,
        validate: impl FnOnce(&Store, &Task, &Task) -> Result<()>,
    ) -> Result<Task> {
        let _board = self.lock("dispatch")?;
        let tracker = Self::normalize_id(tracker)?;
        let member = Self::normalize_id(member)?;
        ensure!(tracker != member, "a Plan cannot be its own member");
        let fresh = self.reopen_unobserved()?;
        let plan = fresh.get(&tracker)?;
        ensure!(
            plan.tracker && plan.approved_scope.is_some(),
            "{} is not a tracker Plan with approved scope",
            plan.id
        );
        ensure!(
            !plan.children.contains(&member),
            "{member} is already a member of {}",
            plan.id
        );
        let task = fresh.get(&member)?;
        ensure!(
            !task.tracker && task.children.is_empty(),
            "{member} is a tracker or has children"
        );
        ensure!(
            task.project == plan.project,
            "{member} belongs to another project than {}",
            plan.id
        );
        // A parent naming this Plan is an interrupted earlier addition.
        if let Some(parent) = task.parent.as_ref().filter(|parent| **parent != plan.id) {
            bail!("{member} already belongs to {parent}");
        }
        // A Plan nested in a parent tracker lists only itself there; a design lists the members.
        let design = match plan.parent.as_deref() {
            Some(id) => Some(fresh.get(id)?).filter(|parent| !parent.tracker),
            None => None,
        };
        if let Some(design) = &design {
            let published: Vec<_> = design
                .children
                .iter()
                .filter(|id| **id != member)
                .cloned()
                .collect();
            ensure!(
                design.status == Status::Done && published == plan.children,
                "cannot add a member to {} before decomposition publication completes",
                plan.id
            );
        }
        validate(&fresh, &plan, &task)?;

        if task.parent.is_none() {
            let _lock = fresh.lock(&member)?;
            let mut task = task;
            task.parent = Some(plan.id.clone());
            fresh.save_inner(&task)?;
        }
        if let Some(mut design) = design.filter(|design| !design.children.contains(&member)) {
            let _lock = fresh.lock(&design.id)?;
            design.children.push(member.clone());
            fresh.save_inner(&design)?;
        }
        let _lock = fresh.lock(&plan.id)?;
        let mut plan = plan;
        plan.children.push(member.clone());
        let saved = fresh.save_inner(&plan)?;
        fresh.log_event(
            Some(&saved.id),
            "plan_member_added",
            serde_json::json!({
                "tracker": saved.id, "member": member, "children": saved.children,
                "design": saved.parent,
            }),
        )?;
        Ok(saved)
    }
}
