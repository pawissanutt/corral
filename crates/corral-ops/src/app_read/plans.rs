//! Canonical plan revisions and published member graphs. Lists read head indexes only;
//! revision detail reads only that design task's plan/review rows. Missing source fields
//! remain absent. Patches page the immutable snapshot bytes, never planner host paths.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use corral_core::backend::{HeadQuery, PageReq, RowQuery, TaskPart};
use corral_core::{PlanHead, PlanRunState, Reviewer, Status, TaskHead, Verdict};
use corral_protocol::detail::{
    DetailBody, PatchPage, PlanDetail, PlanDocument, PlanEdge, PlanMember, PlanPosition,
    PlanProposal, PlanReview,
};
use corral_protocol::rows::{PlanRole, PlanRow, PlanState};

use super::{revision, short};
use crate::Ctx;

pub fn plans_rows(ctx: &Ctx) -> Result<Vec<PlanRow>> {
    let mut heads = ctx
        .store
        .heads_checked(&HeadQuery::Open { project: None })?;
    // Terminal history is bounded by the existing recent-status index, not a store scan.
    for status in [Status::Done, Status::Dropped] {
        heads.extend(ctx.store.heads_page(
            &HeadQuery::RecentByStatus {
                status: status.as_str().into(),
            },
            200,
        )?);
    }
    let mut rows = Vec::new();
    for head in heads.into_iter().filter(is_plan) {
        let members = member_heads(ctx, &head)?;
        let selected = selected_plan(ctx, &head)?;
        let mut row = PlanRow {
            id: head.id.clone(),
            title: short(&head.title, 120),
            role: Some(role(&head)),
            linked: linked(ctx, &head)?,
            state: state(ctx, &head),
            revision: selected.as_ref().map(|(_, plan)| plan.id.clone()),
            delivery: delivery(&head, selected.as_ref().map(|(_, p)| p)),
            members_total: count(members.len()),
            members_landed: count(members.iter().filter(|m| m.landed.is_some()).count()),
            blockers: members
                .iter()
                .filter(|m| m.status != Status::Done)
                .take(4)
                .map(|m| m.id.clone())
                .collect(),
            accepted_at: selected
                .as_ref()
                .and_then(|(_, p)| p.approval.as_ref())
                .map(|approval| approval.at.to_string()),
            projects: members
                .iter()
                .filter_map(|member| member.project.as_deref())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            rev: 0,
        };
        row.rev = revision(&row)?;
        rows.push(row);
    }
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(rows)
}

pub(super) fn plan(ctx: &Ctx, id: &str) -> Result<DetailBody> {
    let head = ctx.store.head(id)?;
    ensure!(is_plan(&head), "{} has no canonical plan", head.id);
    let selected = selected_plan(ctx, &head)?;
    let members = member_heads(ctx, &head)?;
    let layers = layers(&members);
    let edges = edges(&members);
    let mut blockers = Vec::new();
    let mut reviews = Vec::new();
    let mut revisions = Vec::new();
    if let Some((design, selected)) = &selected {
        let view = ctx.store.view()?;
        let mut page = PageReq::first(64);
        loop {
            let records = view.rows(design.num, &RowQuery::part(TaskPart::Plan), page)?;
            for row in records.items {
                let record: PlanHead =
                    serde_json::from_value(row.doc).context("decoding plan revision")?;
                revisions.push(record.id.clone());
                for review in design.plan_reviews(&record.id)? {
                    reviews.push(PlanReview {
                        revision: record.id.clone(),
                        verdict: review.verdict.map_or_else(
                            || {
                                if review.cancelled {
                                    "cancelled"
                                } else {
                                    "pending"
                                }
                                .into()
                            },
                            |verdict| verdict.as_str().into(),
                        ),
                        profile: Some(review.reviewer),
                        at: Some(review.at.to_string()),
                        note: review.notes.map(|text| short(&text, 8192)),
                        source_ref: Some(record.artifact.path.clone()),
                    });
                }
            }
            let Some(after) = records.next else {
                break;
            };
            page = PageReq {
                after: Some(after),
                limit: 64,
            };
        }
        if selected.approval.is_none() && !head.tracker {
            blockers = acceptance_blockers(ctx, design)?;
        }
    }
    if head.tracker {
        // Aggregate final acceptance is a different canonical operation from accepting
        // a document revision; do not offer plan.verdict on a tracker.
        blockers.push("Published plans require aggregate verification and plan acceptance".into());
    }
    if selected.is_none() {
        blockers.push("No submitted plan revision".into());
    }
    if matches!(head.status, Status::Done | Status::Dropped) {
        blockers.push("Plan is terminal".into());
    }
    let acceptable =
        selected.as_ref().is_some_and(|(_, p)| p.approval.is_none()) && blockers.is_empty();
    let document = selected
        .as_ref()
        .map(|(_, p)| document(&p.document))
        .or_else(|| {
            head.approved_scope.as_ref().map(|scope| PlanDocument {
                problem: None,
                goal: Some(short(&scope.goal, 4096)),
                changes: Vec::new(),
                retained: Vec::new(),
                non_goals: Vec::new(),
                proposed: Vec::new(),
            })
        });
    let scenarios = selected
        .as_ref()
        .map(|(_, p)| count(p.document.acceptance.len()))
        .or_else(|| {
            head.approved_scope
                .as_ref()
                .map(|s| count(s.acceptance.len()))
        });
    // A criterion link is an explicit mapping; do not invent mappings from task order.
    let mapped = selected
        .as_ref()
        .map(|(_, p)| {
            count(
                p.document
                    .acceptance
                    .iter()
                    .filter(|scenario| scenario.criterion.is_some())
                    .count(),
            )
        })
        .or_else(|| {
            head.approved_scope.as_ref().map(|s| {
                count(
                    s.acceptance
                        .iter()
                        .filter(|scenario| scenario.criterion.is_some())
                        .count(),
                )
            })
        });
    let mut owner_decisions = Vec::new();
    if let Some((design, plan)) = &selected {
        let obligations = design
            .current_run
            .as_ref()
            .filter(|run| run.facts.pending_decision.is_some())
            .map(|run| design.open_obligations(&run.id))
            .transpose()?;
        let sources = plan
            .document
            .owner_decisions
            .iter()
            .chain(
                design
                    .completion
                    .iter()
                    .flat_map(|manifest| &manifest.owner_decisions),
            )
            .chain(obligations.iter().flat_map(|obligations| {
                obligations
                    .decisions
                    .iter()
                    .filter(|decision| decision.pending())
                    .map(|decision| &decision.request.question)
            }));
        let mut seen = BTreeSet::new();
        for decision in sources {
            if seen.insert(decision.as_str()) {
                owner_decisions.push(decision.clone());
            }
        }
    }
    Ok(DetailBody::Plan(PlanDetail {
        id: head.id.clone(),
        role: Some(role(&head)),
        linked: linked(ctx, &head)?,
        revision: selected.as_ref().map(|(_, p)| p.id.clone()),
        state: state(ctx, &head),
        document,
        revisions,
        owner_decisions,
        reviews,
        members: members
            .iter()
            .map(|member| PlanMember {
                id: member.id.clone(),
                title: short(&member.title, 512),
                layer: layers.get(&member.id).copied(),
                state: member.status.as_str().into(),
                rail: crate::boardview::rail::steps(member),
            })
            .collect(),
        edges,
        scenarios,
        mapped,
        acceptable,
        accept_blockers: blockers,
    }))
}

/// Same owner acceptance boundary as plan_verdict, without starting a review or writing.
fn acceptance_blockers(ctx: &Ctx, head: &TaskHead) -> Result<Vec<String>> {
    let task = corral_core::plan_policy::approval_shell(head)?;
    let selected = task.plans.last().context("submitted plan missing")?;
    let check = || -> Result<()> {
        crate::assistant::workflow::handoff::admission::require(
            &ctx.store,
            &task,
            corral_core::OperatingActionKind::AcceptPlan,
            false,
        )?;
        crate::plan::fresh(ctx, &task, selected)?;
        if let Some(review) = selected.reviews.last()
            && crate::recovery::plan_review_record(ctx, &task, &selected.id, &review.id)?.is_some()
        {
            anyhow::bail!("Recovery-bound plan needs trusted local owner confirmation");
        }
        let config = crate::plan::config(ctx)?;
        if crate::plan::expected_reviewer(&config, &task)? != "owner" {
            crate::recovery::plan_review_valid(ctx, &task, selected)?;
        }
        // Decision text is exposed separately: owner notes resolve it at submit.
        // Its presence alone does not make this revision unacceptable.
        crate::plan_decompose::check_attachment(&ctx.store, &task, &selected.id)?;
        Ok(())
    };
    Ok(check()
        .err()
        .map(|error| vec![short(&format!("{error:#}"), 2048)])
        .unwrap_or_default())
}

pub(super) fn plan_diff(
    ctx: &Ctx,
    id: &str,
    from: &str,
    to: &str,
    offset: usize,
) -> Result<(DetailBody, Option<usize>)> {
    let head = ctx.store.head(id)?;
    let (design, _) = selected_plan(ctx, &head)?.context("no submitted plan revision")?;
    let before = design
        .plan_by_id(from)?
        .context("unknown source plan revision")?;
    let after = design
        .plan_by_id(to)?
        .context("unknown destination plan revision")?;
    let old = snapshot(ctx, &before)?;
    let new = snapshot(ctx, &after)?;
    let old = std::str::from_utf8(&old).context("source plan is not UTF-8")?;
    let new = std::str::from_utf8(&new).context("destination plan is not UTF-8")?;
    // Whole-file unified replacement is deterministic and retains every snapshot byte;
    // paging avoids allocating a second document-sized patch or quadratic diff work.
    let header = if old == new {
        String::new()
    } else {
        format!(
            "--- {}@{}\n+++ {}@{}\n@@ -{},{} +{},{} @@\n",
            design.id,
            from,
            design.id,
            to,
            usize::from(!old.is_empty()),
            old.lines().count(),
            usize::from(!new.is_empty()),
            new.lines().count()
        )
    };
    let deleted = old.split_inclusive('\n').flat_map(|line| {
        [
            "-",
            line,
            if line.ends_with('\n') {
                ""
            } else {
                "\n\\ No newline at end of file\n"
            },
        ]
    });
    let added = new.split_inclusive('\n').flat_map(|line| {
        [
            "+",
            line,
            if line.ends_with('\n') {
                ""
            } else {
                "\n\\ No newline at end of file\n"
            },
        ]
    });
    let chunks = std::iter::once(header.as_str()).chain(deleted).chain(added);
    let (patch, next) = if old == new {
        super::review::text_page(std::iter::empty(), offset)?
    } else {
        super::review::text_page(chunks, offset)?
    };
    Ok((
        DetailBody::PlanDiff(PatchPage {
            path: design.id.clone(),
            patch,
            truncated: next.is_some(),
            candidate: None,
        }),
        next,
    ))
}

fn snapshot(ctx: &Ctx, plan: &PlanHead) -> Result<Vec<u8>> {
    let bytes = crate::artifact_storage::artifact_read_confined_limited(
        ctx,
        &plan.artifact,
        ctx.store.config.artifacts.max_bytes,
    )?;
    ensure!(
        bytes.len() as u64 <= ctx.store.config.artifacts.max_bytes
            && bytes.len() as u64 == plan.artifact.bytes
            && crate::verification::digest(&bytes) == plan.artifact.sha256,
        "plan snapshot is missing, purged, or changed"
    );
    Ok(bytes)
}

fn document(source: &corral_core::PlanDocument) -> PlanDocument {
    PlanDocument {
        problem: source
            .human_review
            .as_ref()
            .map(|v| short(&v.problem, 4096)),
        goal: Some(short(&source.goal, 4096)),
        changes: source
            .human_review
            .as_ref()
            .map(|v| v.changes.iter().map(|v| short(v, 8192)).collect())
            .unwrap_or_default(),
        retained: source
            .human_review
            .as_ref()
            .map(|v| v.retained.iter().map(|v| short(v, 8192)).collect())
            .unwrap_or_default(),
        non_goals: source.non_goals.iter().map(|v| short(v, 8192)).collect(),
        proposed: source
            .dependencies
            .iter()
            .map(|scope| PlanProposal {
                name: scope.task.clone(),
                needs: scope.depends_on.clone(),
            })
            .collect(),
    }
}

fn is_plan(head: &TaskHead) -> bool {
    head.tracker || head.design || head.last_plan.is_some() || head.last_plan_run.is_some()
}

fn role(head: &TaskHead) -> PlanRole {
    if head.tracker {
        PlanRole::Tracker
    } else if head.design {
        PlanRole::Design
    } else {
        PlanRole::Other
    }
}

fn linked(ctx: &Ctx, head: &TaskHead) -> Result<Option<String>> {
    if head.tracker {
        return Ok(head
            .approved_scope
            .as_ref()
            .and_then(|scope| scope.approved_design.as_deref())
            .and_then(|source| source.split_once('@'))
            .map(|(id, _)| id.to_owned()));
    }
    if !head.design {
        return Ok(None);
    }
    // Publication retains work members in design.children; its tracker is instead
    // reached through the indexed parent relation, including terminal trackers.
    Ok(ctx
        .store
        .heads_checked(&HeadQuery::Children {
            parent: head.id.clone(),
        })?
        .into_iter()
        .find(|child| child.tracker)
        .map(|tracker| tracker.task.id))
}

/// Design intent and planner history stay visible, but only a submitted document can
/// await acceptance. Current planner/reviewer work takes precedence over old verdicts.
pub(super) fn state(ctx: &Ctx, head: &TaskHead) -> PlanState {
    match head.status {
        Status::Dropped => return PlanState::Cancelled,
        Status::Done if head.tracker => return PlanState::Complete,
        _ if head.tracker => return PlanState::Executing,
        _ if !head.design && !head.children.is_empty() => return PlanState::Executing,
        _ => {}
    }
    let run = head.last_plan_run.as_deref();
    if let Some(run) = run {
        match run.state {
            PlanRunState::Failed | PlanRunState::Escalated => return PlanState::PlanFailed,
            PlanRunState::Cancelled => return PlanState::Cancelled,
            PlanRunState::Planning | PlanRunState::Changes => return PlanState::Drafting,
            _ => {}
        }
    }
    let Some(plan) = head.last_plan.as_deref() else {
        return PlanState::Drafting;
    };
    if plan.approval.is_some() {
        return PlanState::Approved;
    }
    if head.status == Status::Done {
        return PlanState::Complete;
    }
    let awaiting_owner = run.is_some_and(|run| {
        run.state == PlanRunState::AwaitingOwner
            && run.submitted.as_deref() == Some(plan.id.as_str())
            && plan.run.as_deref() == Some(run.id.as_str())
    });
    match plan.current_review.as_ref() {
        Some(review) if review.cancelled => PlanState::Reviewing,
        Some(review) => match review.verdict {
            Some(Verdict::Approve) if awaiting_owner || head.status == Status::NeedsMe => {
                PlanState::Awaiting
            }
            Some(Verdict::Changes) => PlanState::Drafting,
            Some(Verdict::Escalate) => PlanState::PlanFailed,
            None if review.reviewer == "owner" => PlanState::Awaiting,
            _ => PlanState::Reviewing,
        },
        None if plan.run.is_none()
            && ctx.store.config.task_review_policy(head).reviewer == Reviewer::None =>
        {
            PlanState::Awaiting
        }
        None => PlanState::Reviewing,
    }
}

fn delivery(head: &TaskHead, selected: Option<&PlanHead>) -> Option<String> {
    let target = head
        .approved_scope
        .as_ref()
        .map(|s| &s.delivery_target)
        .or_else(|| selected.and_then(|p| p.document.delivery_target.as_ref()));
    target.map(|target| match target {
        corral_core::PlanDeliveryTarget::Main => "main".into(),
        corral_core::PlanDeliveryTarget::Integration { branch } => branch.clone(),
    })
}

fn selected_plan(ctx: &Ctx, head: &TaskHead) -> Result<Option<(TaskHead, PlanHead)>> {
    if let Some(plan) = &head.last_plan {
        return Ok(Some((head.clone(), (**plan).clone())));
    }
    if let Some(link) = head
        .approved_scope
        .as_ref()
        .and_then(|s| s.approved_design.as_deref())
        && let Some((id, revision)) = link.split_once('@')
    {
        let design = ctx.store.head(id)?;
        if let Some(plan) = design.plan_by_id(revision)? {
            return Ok(Some((design, plan)));
        }
    }
    Ok(None)
}

fn member_heads(ctx: &Ctx, head: &TaskHead) -> Result<Vec<TaskHead>> {
    let mut members = ctx
        .store
        .heads_checked(&HeadQuery::Ids(head.children.clone()))?;
    // A design can publish a tracker; expose its real members, not the tracker as work.
    if let Some(tracker) = members.iter().find(|member| member.tracker) {
        members = ctx
            .store
            .heads_checked(&HeadQuery::Ids(tracker.children.clone()))?;
    }
    Ok(members)
}

fn edges(members: &[TaskHead]) -> Vec<PlanEdge> {
    let ids: BTreeSet<_> = members.iter().map(|m| m.id.as_str()).collect();
    members
        .iter()
        .flat_map(|member| {
            member
                .depends_on
                .iter()
                .filter(|id| ids.contains(id.as_str()))
                .map(|dependency| PlanEdge {
                    from: dependency.clone(),
                    to: member.id.clone(),
                })
        })
        .collect()
}

fn layers(members: &[TaskHead]) -> BTreeMap<String, u32> {
    let ids: BTreeSet<_> = members.iter().map(|m| m.id.as_str()).collect();
    let mut remaining: BTreeMap<&str, (usize, u32)> = ids.iter().map(|id| (*id, (0, 1))).collect();
    let mut followers = BTreeMap::<&str, Vec<&str>>::new();
    for member in members {
        for dependency in member
            .depends_on
            .iter()
            .filter(|id| ids.contains(id.as_str()))
        {
            if let Some((count, _)) = remaining.get_mut(member.id.as_str()) {
                *count += 1;
            }
            followers
                .entry(dependency.as_str())
                .or_default()
                .push(member.id.as_str());
        }
    }
    let mut ready: std::collections::VecDeque<_> = remaining
        .iter()
        .filter(|(_, (count, _))| *count == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut layers = BTreeMap::new();
    while let Some(id) = ready.pop_front() {
        let layer = remaining[id].1;
        layers.insert(id.to_owned(), layer);
        if let Some(children) = followers.get(id) {
            for child in children {
                if let Some((count, next)) = remaining.get_mut(child) {
                    *count -= 1;
                    *next = (*next).max(layer + 1);
                    if *count == 0 {
                        ready.push_back(*child);
                    }
                }
            }
        }
    }
    // Cycles do not acquire invented positions.
    layers
}

pub(super) fn plan_context(
    ctx: &Ctx,
    task: &TaskHead,
) -> Result<Option<(PlanPosition, Option<String>)>> {
    let parent = if let Some(parent) = task.parent.as_deref() {
        ctx.store.head_if_exists(parent)?
    } else if let Some((id, _)) = task.plan.as_deref().and_then(|s| s.split_once('@')) {
        ctx.store.head_if_exists(id)?
    } else {
        None
    };
    let Some(parent) = parent else {
        return Ok(None);
    };
    let members = member_heads(ctx, &parent)?;
    let layers = layers(&members);
    let Some(layer) = layers.get(&task.id).copied() else {
        return Ok(None);
    };
    let goal = parent
        .approved_scope
        .as_ref()
        .map(|s| s.goal.clone())
        .or_else(|| parent.last_plan.as_ref().map(|p| p.document.goal.clone()));
    Ok(Some((
        PlanPosition {
            plan: parent.id.clone(),
            layer,
            layers: layers.values().copied().max().unwrap_or(0),
            landed: count(
                members
                    .iter()
                    .filter(|member| member.landed.is_some())
                    .count(),
            ),
            total: count(members.len()),
        },
        goal.map(|goal| short(&goal, 4096)),
    )))
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
#[path = "plans_tests.rs"]
mod tests;
