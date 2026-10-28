//! Adding an unstarted task to a tracker Plan's required members, or moving one between tracker
//! Plans. Membership is part of the Plan's operating contract, so grants and member runs bound to
//! the old contract are refused afterwards; the caller decides which member states may be stranded.
use anyhow::{Context, Result, bail, ensure};

use super::member_gate::next_record;
use super::{ScopeWrite, Store};
use crate::{ScopeGate, Status, Task};

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

    /// Move `member` from tracker `source` to tracker `target`, keeping each decomposed Plan's
    /// design children equal to its tracker's. The target records `owner_gate` (gate, reason) as
    /// the owner's next member-gate record after the source's, else carries the source's record. Both
    /// trackers advance `membership_revision`, so proof bound to an earlier membership stays stale
    /// even when a later change restores the same member list. `validate` checks the fresh source,
    /// target and member under the board lock and must not write. Writes go member, target design,
    /// target, source design, source, and each is skipped when already made, so rerunning after a
    /// crash completes the move once. Returns the saved source and target.
    ///
    /// # Errors
    /// Rejects non-trackers, a member the source does not own, the source's last member, a target
    /// that already owns it, trackers or parents as members, another project, an incomplete
    /// decomposition publication, a failed validation and persistence failures.
    pub fn move_plan_member(
        &self,
        source: &str,
        member: &str,
        target: &str,
        owner_gate: Option<(&ScopeGate, &str)>,
        validate: impl FnOnce(&Store, &Task, &Task, &Task) -> Result<()>,
    ) -> Result<(Task, Task)> {
        let _board = self.lock("dispatch")?;
        let source = Self::normalize_id(source)?;
        let target = Self::normalize_id(target)?;
        let member = Self::normalize_id(member)?;
        ensure!(source != target, "{member} already belongs to {target}");
        let fresh = self.reopen_unobserved()?;
        let from = fresh.get(&source)?;
        let to = fresh.get(&target)?;
        for plan in [&from, &to] {
            ensure!(
                plan.tracker && plan.approved_scope.is_some(),
                "{} is not a tracker Plan with approved scope",
                plan.id
            );
        }
        ensure!(
            from.children.contains(&member),
            "{member} is not a member of {}",
            from.id
        );
        ensure!(
            from.children.len() > 1,
            "{member} is the last member of {}; a Plan needs at least one required member",
            from.id
        );
        let task = fresh.get(&member)?;
        ensure!(
            !task.tracker && task.children.is_empty(),
            "{member} is a tracker or has children"
        );
        ensure!(
            task.project == from.project && to.project == from.project,
            "{} belongs to another project than {}",
            to.id,
            from.id
        );
        // A parent naming the target is an interrupted earlier move; the source is written last.
        let resumed = task.parent.as_ref() == Some(&to.id);
        ensure!(
            resumed || (task.parent.as_ref() == Some(&from.id) && !to.children.contains(&member)),
            "{member} has an inconsistent Plan owner"
        );
        let to_design = published_design(&fresh, &to, &member)?;
        let from_design = published_design(&fresh, &from, &member)?;
        validate(&fresh, &from, &to, &task)?;

        if !resumed {
            let _lock = fresh.lock(&member)?;
            let mut task = task;
            task.parent = Some(to.id.clone());
            fresh.save_inner(&task)?;
        }
        if let Some(mut design) = to_design.filter(|design| !design.children.contains(&member)) {
            let _lock = fresh.lock(&design.id)?;
            design.children.push(member.clone());
            fresh.save_inner(&design)?;
        }
        let to = if to.children.contains(&member) {
            to
        } else {
            let _lock = fresh.lock(&to.id)?;
            let mut plan = to;
            plan.children.push(member.clone());
            plan.membership_revision = next_revision(plan.membership_revision)?;
            let previous = from.member_gates.get(&member);
            let record = match owner_gate {
                Some((gate, reason)) => Some(next_record(previous, gate, reason, "owner")?),
                None => previous.cloned(),
            };
            if let Some(record) = record {
                plan.member_gates.insert(member.clone(), record);
            }
            fresh.save_task(&plan, ScopeWrite::MemberGate)?
        };
        if let Some(mut design) = from_design.filter(|design| design.children.contains(&member)) {
            let _lock = fresh.lock(&design.id)?;
            design.children.retain(|id| *id != member);
            fresh.save_inner(&design)?;
        }
        let _lock = fresh.lock(&from.id)?;
        let mut from = from;
        from.children.retain(|id| *id != member);
        from.membership_revision = next_revision(from.membership_revision)?;
        from.member_gates.remove(&member);
        let from = fresh.save_task(&from, ScopeWrite::MemberGate)?;
        fresh.log_event(
            Some(&from.id),
            "plan_member_moved",
            serde_json::json!({
                "member": member, "from": from.id, "to": to.id,
                "from_children": from.children, "to_children": to.children,
                "from_membership_revision": from.membership_revision,
                "to_membership_revision": to.membership_revision,
                "to_member_gate_revision": to.member_gates.get(&member).map(|r| r.revision),
            }),
        )?;
        Ok((from, to))
    }
}

/// A Plan nested in an ordinary design lists only itself there; the design lists the members and
/// must already equal the tracker's, ignoring `member`.
fn published_design(store: &Store, plan: &Task, member: &str) -> Result<Option<Task>> {
    let Some(design) = plan.parent.as_deref().map(|id| store.get(id)).transpose()? else {
        return Ok(None);
    };
    if design.tracker {
        return Ok(None);
    }
    let others = |children: &[String]| -> Vec<String> {
        children
            .iter()
            .filter(|id| *id != member)
            .cloned()
            .collect()
    };
    ensure!(
        design.status == Status::Done && others(&design.children) == others(&plan.children),
        "cannot change the members of {} before decomposition publication completes",
        plan.id
    );
    Ok(Some(design))
}

fn next_revision(revision: u64) -> Result<u64> {
    revision
        .checked_add(1)
        .context("Plan membership revision exhausted")
}
