//! The tracker is part of the existing decomposition receipt, not a second store.
use super::*;
use corral_core::ApprovedScope;

pub(super) fn stage(
    parent: &Task,
    document: &PlanDocument,
    backlink: &str,
    artifact_sha256: &str,
    children: &mut [Task],
) -> Result<Task> {
    let number = children
        .iter()
        .try_fold(0_u64, |number, child| {
            Ok::<_, anyhow::Error>(number.max(child.id.trim_start_matches("T-").parse::<u64>()?))
        })?
        .checked_add(1)
        .context("task ID exhausted")?;
    let mut tracker = parent.clone();
    tracker.id = format!("T-{number:03}");
    tracker.status = Status::Backlog;
    tracker.tracker = true;
    tracker.parent = Some(parent.id.clone());
    tracker.plan = None;
    tracker.design = false;
    tracker.plan_required = Some(false);
    tracker.approved_scope = Some(ApprovedScope {
        goal: document.goal.clone(),
        acceptance: document.acceptance.clone(),
        delivery_target: document.delivery_target.clone().unwrap_or_default(),
        approved_design: Some(backlink.to_owned()),
    });
    tracker.source_digest = Some(artifact_sha256.to_owned());
    tracker.children = children.iter().map(|child| child.id.clone()).collect();
    tracker.plans.clear();
    tracker.plan_runs.clear();
    tracker.runs.clear();
    tracker.artifacts.clear();
    tracker.comments.clear();
    tracker.completion = None;
    tracker.dashboard = None;
    tracker.rounds = 0;
    tracker.write_scope.clear();
    tracker.depends_on.clear();
    tracker.queued = false;
    tracker.dispatch_plan = None;
    tracker.dispatch_hold = false;
    tracker.review_reservation = None;
    tracker.review_wait = None;
    tracker.created = jiff::Timestamp::now();
    tracker.updated = tracker.created;
    for child in children {
        child.parent = Some(tracker.id.clone());
    }
    Ok(tracker)
}

pub(super) fn upgrade(journal: &mut Journal, parent: &Task) -> Result<()> {
    if journal.tracker.is_some() {
        return Ok(());
    }
    let mut children = journal
        .children
        .iter()
        .map(|text| parse_task(text, Path::new("")))
        .collect::<Result<Vec<_>>>()?;
    for child in &children {
        journal
            .previous_contracts
            .entry(child.id.clone())
            .or_default()
            .insert(operating_contract_revision(child)?);
    }
    let tracker = stage(
        parent,
        &journal.document,
        &journal.backlink,
        &journal.artifact_sha256,
        &mut children,
    )?;
    journal.children = children.iter().map(render_task).collect();
    journal.tracker = Some(render_task(&tracker));
    Ok(())
}

pub(super) fn check(journal: &Journal, parent: &Task) -> Result<()> {
    let Some(text) = &journal.tracker else {
        // Validate historical receipts before upgrading an unfinished publication.
        return Ok(());
    };
    let tracker = parse_task(text, Path::new(""))?;
    let children = journal
        .children
        .iter()
        .map(|text| parse_task(text, Path::new("")))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        tracker.tracker
            && tracker.status == Status::Backlog
            && tracker.parent.as_ref() == Some(&parent.id)
            && tracker.source_digest.as_ref() == Some(&journal.artifact_sha256)
            && tracker.approved_scope
                == Some(ApprovedScope {
                    goal: journal.document.goal.clone(),
                    acceptance: journal.document.acceptance.clone(),
                    delivery_target: journal.document.delivery_target.clone().unwrap_or_default(),
                    approved_design: Some(journal.backlink.clone()),
                })
            && tracker.children
                == children
                    .iter()
                    .map(|child| child.id.clone())
                    .collect::<Vec<_>>()
            && children
                .iter()
                .all(|child| child.parent.as_ref() == Some(&tracker.id) && child.id != tracker.id),
        "decomposition journal tracker identity changed"
    );
    Ok(())
}
