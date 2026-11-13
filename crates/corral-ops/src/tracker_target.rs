//! Target qualification uses existing acceptance events and actual Git ancestry.
use anyhow::{Context, Result, bail};
use corral_core::{PlanDeliveryTarget, Status, Store, Task};
use serde::{Deserialize, Serialize};

/// Acceptance and inclusion are separate facts about a prerequisite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPrerequisite {
    pub task: String,
    pub status: Status,
    pub external: bool,
    pub target: String,
    pub target_head: Option<String>,
    /// Proposed dispatch base, not an assertion that the target ref exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_initial_base: Option<String>,
    pub receipt_sha: Option<String>,
    pub design_publication: Option<String>,
    pub accepted: bool,
    pub satisfied: bool,
    pub reason: Option<String>,
}

pub(super) fn prerequisites(
    store: &Store,
    plan: &Task,
    task: &Task,
) -> Result<Vec<PlanPrerequisite>> {
    let _span = corral_core::timing::span("plan.prerequisites");
    let target = match plan
        .approved_scope
        .as_ref()
        .map(|scope| &scope.delivery_target)
    {
        Some(PlanDeliveryTarget::Integration { branch }) => branch.as_str(),
        Some(PlanDeliveryTarget::Main) | None => "main",
    };
    // The Plan's repository and proposed base are the same for every dependency: resolve
    // them once, at the first dependency that needs them, in the original order.
    let mut plan_base: Option<(std::path::PathBuf, Option<String>)> = None;
    let memo = crate::integration::git_memo_of(store);
    task.depends_on
        .iter()
        .map(|id| {
            // Shared with dependency_check: one read per pinned pass.
            let dependency_head = crate::queue::dependency_head(store, id)
                .with_context(|| format!("{}: missing dependency {id}", task.id))?;
            let dependency = crate::queue::dependency_shell(store, &dependency_head)?;
            let mut result = PlanPrerequisite {
                task: dependency.id.clone(),
                status: dependency.status,
                external: !plan.children.contains(id),
                target: target.into(),
                target_head: None,
                proposed_initial_base: None,
                receipt_sha: None,
                design_publication: None,
                accepted: dependency.status == Status::Done,
                satisfied: false,
                reason: None,
            };
            if !result.accepted {
                result.reason = Some(format!("waiting on {id} ({})", dependency.status.label()));
                return Ok(result);
            }
            // Legacy trackers have no execution target contract.
            if plan.approved_scope.is_none() {
                result.satisfied = true;
                return Ok(result);
            }
            // Decomposition's exact approved design is a non-code publication receipt.
            if task.plan.as_ref().is_some_and(|link| {
                plan.approved_scope
                    .as_ref()
                    .and_then(|scope| scope.approved_design.as_ref())
                    == Some(link)
                    && dependency.plans.last().is_some_and(|design| {
                        link == &format!("{}@{}", dependency.id, design.id)
                            && dependency.children.contains(&task.id)
                    })
            }) {
                // The approval validator uses the final two reviews, the referenced
                // planner and pending decisions. Supply those keyed rows explicitly;
                // a shell's empty history is not evidence of an accepted design.
                let mut dependency = dependency.as_ref().clone();
                approval_inputs(store, &dependency_head, &mut dependency)?;
                let design = dependency.plans.last().context("design receipt missing")?;
                corral_core::plan_policy::validate_approval(
                    store,
                    &store.config,
                    &dependency,
                    design,
                )?;
                result.satisfied = true;
                result.design_publication = task.plan.clone();
                return Ok(result);
            }
            if dependency.project != plan.project {
                result.reason = Some(format!("{id}: prerequisite belongs to another repository"));
                return Ok(result);
            }
            let Some(run) = dependency
                .current_run()
                .filter(|run| run.outcome == Some(corral_core::RunOutcome::Done))
            else {
                result.reason = Some(format!("{id}: accepted but no completed run receipt"));
                return Ok(result);
            };
            let receipt = approval(store, id, &run.id)?;
            let Some(receipt) = receipt else {
                result.reason = Some(format!("{id}: accepted but no current acceptance receipt"));
                return Ok(result);
            };
            if let Some(binding) = &run.plan_target
                && binding.plan != plan.id
            {
                result.reason = Some(format!("{id}: accepted into another Plan; explicit target reconciliation required"));
                return Ok(result);
            }
            if !matches!(
                receipt.detail["strategy"].as_str(),
                Some("fast_forward" | "rebase")
            ) {
                result.reason = Some(format!(
                    "{id}: accepted branch was not integrated into {target}"
                ));
                return Ok(result);
            }
            let sha = receipt.detail["sha"]
                .as_str()
                .context("acceptance receipt has no commit SHA")?;
            result.receipt_sha = Some(sha.into());
            if run.plan_target.is_some()
                && !crate::plan_target::has_receipt(store, plan, &dependency, run, sha)?
            {
                result.reason = Some(format!("{id}: no published owned-target integration receipt"));
                return Ok(result);
            }
            let (repo, proposed) = match &plan_base {
                Some(known) => known.clone(),
                None => {
                    let repo = store.project_path(
                        plan.project
                            .as_deref()
                            .context("execution Plan has no project")?,
                    )?;
                    // Readiness may qualify a new target against the exact base its writer
                    // will use. Owned initialized refs still require their actual Git head.
                    let proposed = crate::plan_target::proposed_initial_base(store, plan)?;
                    plan_base.insert((repo, proposed)).clone()
                }
            };
            result.proposed_initial_base = proposed;
            let head = match &result.proposed_initial_base {
                Some(base) => base.clone(),
                None => match crate::plan_target::branch_head(store, &repo, target) {
                    Ok(head) => {
                        result.target_head = Some(head.clone());
                        head
                    }
                    Err(_) => {
                        result.reason = Some(format!("Plan target {target} is missing; initialize or reconcile it before dispatch"));
                        return Ok(result);
                    }
                },
            };
            // A stored inclusion of two full SHAs cannot change; exclusions are rechecked.
            result.satisfied = memo
                .is_ancestor(&repo, sha, &head)
                .with_context(|| format!("qualifying prerequisite {id} in {target}"))?;
            if !result.satisfied {
                result.reason = Some(format!("{id}: accepted work is not included in {target}"));
            }
            Ok(result)
        })
        .collect()
}

pub(crate) fn target_blocker(store: &Store, task: &Task) -> Result<Option<String>> {
    let _span = corral_core::timing::span("queue.target_blocker");
    let head = store.head(&task.id)?;
    let Some(plan_head) = store.execution_plan_head(&head)? else {
        return Ok(None);
    };
    let plan = plan_head.to_shell_task()?;
    if let Err(error) = crate::plan_recovery::require_active(store, &plan) {
        return Ok(Some(format!("{error:#}")));
    }
    // Admission must retain the actual member's bound run/infra-wait context. The
    // target helper resolves its parent through heads, so this does not hydrate history.
    // On one pinned view, a task with its stored membership has the Plan just resolved;
    // a live store resolves it again.
    let pinned = store.is_snapshot()
        && task.id == head.id
        && task.tracker == head.tracker
        && task.approved_scope.is_some() == head.approved_scope.is_some()
        && task.parent == head.parent
        && task.project == head.project
        && task.children == head.children;
    crate::plan_target::check_dispatch_target(store, task, || {
        if pinned {
            Ok(Some(plan_head.clone()))
        } else {
            store.execution_plan_ref(task)
        }
    })?;
    let prerequisites = prerequisites(store, &plan, task)?;
    let reason = prerequisites
        .iter()
        .find(|dependency| !dependency.satisfied)
        .and_then(|dependency| dependency.reason.clone());
    if reason.is_some() {
        return Ok(reason);
    }
    if task.launch.is_some()
        && let Some(run) = task.current_run()
    {
        return base_reason(store, &plan, &prerequisites, &run.base, &run.base_sha);
    }
    Ok(None)
}

fn base_reason(
    store: &Store,
    plan: &Task,
    prerequisites: &[PlanPrerequisite],
    base: &str,
    sha: &str,
) -> Result<Option<String>> {
    crate::plan_recovery::require_active(store, plan)?;
    let target = match &plan
        .approved_scope
        .as_ref()
        .context("Plan has no scope")?
        .delivery_target
    {
        PlanDeliveryTarget::Main => "main",
        PlanDeliveryTarget::Integration { branch } => branch,
    };
    if base != target {
        return Ok(Some(format!(
            "execution Plan requires {target} as its dispatch base"
        )));
    }
    let repo = store.project_path(plan.project.as_deref().context("Plan has no project")?)?;
    for prerequisite in prerequisites {
        if !prerequisite.satisfied {
            return Ok(prerequisite.reason.clone());
        }
        if let Some(commit) = &prerequisite.receipt_sha
            && !corral_git::is_ancestor(&repo, commit, sha)?
        {
            return Ok(Some(format!(
                "prepared dispatch base does not include prerequisite {}; requeue from the current target",
                prerequisite.task,
            )));
        }
    }
    Ok(None)
}

pub(crate) fn check_dispatch_base(store: &Store, task: &Task, base: &str, sha: &str) -> Result<()> {
    if let Some(plan) = store.execution_plan_head(&store.head(&task.id)?)? {
        let plan = plan.to_shell_task()?;
        if let Some(reason) =
            base_reason(store, &plan, &prerequisites(store, &plan, task)?, base, sha)?
        {
            bail!("{reason}");
        }
    } else {
        // Legacy metadata-only dependencies retain their status contract. A dependency
        // with a coding run must prove that its accepted commit is in this exact base,
        // even when the consumer is not itself a Plan member.
        for id in &task.depends_on {
            let dependency = store.head(id)?.to_shell_task()?;
            let Some(run) = dependency.current_run() else {
                continue;
            };
            let receipt = approval(store, id, &run.id)?
                .with_context(|| format!("{id}: no current integration receipt"))?;
            anyhow::ensure!(
                dependency.status == Status::Done
                    && run.outcome == Some(corral_core::RunOutcome::Done)
                    && matches!(
                        receipt.detail["strategy"].as_str(),
                        Some("fast_forward" | "rebase" | "keep_branch")
                    ),
                "{id}: prerequisite was not accepted into a target"
            );
            anyhow::ensure!(
                dependency.project == task.project,
                "{id}: prerequisite belongs to another repository"
            );
            let repo =
                store.project_path(task.project.as_deref().context("task has no project")?)?;
            let commit = receipt.detail["sha"]
                .as_str()
                .context("acceptance receipt has no commit SHA")?;
            anyhow::ensure!(
                corral_git::is_ancestor(&repo, commit, sha)?,
                "prepared dispatch base {base} does not include prerequisite {id}"
            );
        }
    }
    Ok(())
}

/// The newest valid `approved` event of (`task`, `run`): read once per pinned pass, since
/// several Plan members name the same dependency; a live store reads it on every call.
pub(crate) fn approval(store: &Store, task: &str, run: &str) -> Result<Option<corral_core::Event>> {
    use corral_core::backend::{EventQuery, PageReq};
    let receipt = store.pass_derived(&format!("plan-approval\0{task}\0{run}"), || {
        store
            .view()?
            .events(
                &EventQuery {
                    task: Some(task.to_owned()),
                    kinds: vec!["approved".into()],
                    run: Some(run.to_owned()),
                    newest_first: true,
                    valid_only: true,
                    ..EventQuery::default()
                },
                PageReq::first(1),
            )?
            .items
            .into_iter()
            .next()
            .map(crate::approvals::event)
            .transpose()
            .map(Option::flatten)
    })?;
    Ok(receipt.as_ref().clone())
}

fn approval_inputs(store: &Store, head: &corral_core::TaskHead, task: &mut Task) -> Result<()> {
    use corral_core::backend::{PageReq, TaskPart};
    let plan = head.last_plan.as_ref().context("design receipt missing")?;
    let count = usize::try_from(plan.facts.children).context("plan review count overflow")?;
    let mut reviews = Vec::new();
    for ordinal in count.saturating_sub(1).max(1)..=count {
        if let Some(review) = head.plan_review_at(&plan.id, ordinal)? {
            reviews.push(serde_json::from_value(serde_json::to_value(review)?)?);
        }
    }
    task.plans
        .last_mut()
        .context("design receipt missing")?
        .reviews = reviews;
    if let Some(id) = &plan.run {
        let mut page = PageReq::first(1);
        let rows = store.task_rows(
            &task.id,
            TaskPart::PlanRun,
            Some(format!("id:{id}")),
            None,
            &mut page,
        )?;
        task.plan_runs = rows
            .into_iter()
            .map(serde_json::from_value)
            .collect::<std::result::Result<_, _>>()?;
    }
    if let Some(run) = task.runs.last_mut() {
        run.obligations = head.open_obligations(&run.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod derived_rows_tests {
    use super::*;
    use corral_core::work::{self, Counter, HotScope};
    use serde_json::json;

    #[test]
    fn derived_dependency_receipts_use_the_newest_event_of_the_exact_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let dependency = store
            .create("Dependency", Some("app"), Status::Done, "")
            .unwrap();
        for (run, sha) in [("R1", "old"), ("R2", "accepted"), ("R1", "newer-other-run")] {
            store
                .log_event(
                    Some(&dependency.id),
                    "approved",
                    json!({"run":run,"sha":sha}),
                )
                .unwrap();
        }
        let expected = store
            .events_of(Some(&dependency.id), &["approved"])
            .unwrap()
            .into_iter()
            .rev()
            .find(|event| event.detail["run"] == "R2")
            .unwrap();
        let measure = || {
            let _scope = HotScope::enter();
            let before = work::snapshot();
            let output = approval(&store, &dependency.id, "R2").unwrap().unwrap();
            let counts = work::snapshot().since(&before);
            counts.check_identity().unwrap();
            assert_eq!(counts.unattributed_hydrations(), 0);
            (output, counts)
        };
        let (before, base) = measure();
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        for i in 0..10 {
            let task = store
                .create("Other", Some("app"), Status::Done, "")
                .unwrap();
            store
                .log_event(
                    Some(&task.id),
                    "approved",
                    json!({"run":"R2","sha":"other"}),
                )
                .unwrap();
            store
                .log_event(
                    Some(&dependency.id),
                    "approved",
                    json!({"run":"R1","sha":i}),
                )
                .unwrap();
        }
        let (after, grown) = measure();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
        for counter in Counter::ALL {
            assert_eq!(base.get(counter), grown.get(counter), "{counter:?}");
        }
        assert_eq!(base.get(Counter::EventDecodes), 1);
    }

    #[test]
    fn derived_legacy_plan_prerequisites_read_dependency_heads_without_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let mut plan = store
            .create("Plan", Some("app"), Status::Running, "")
            .unwrap();
        let dependency = store
            .create("Done dependency", Some("app"), Status::Done, "")
            .unwrap();
        let mut task = store
            .create("Consumer", Some("app"), Status::Ready, "")
            .unwrap();
        task.depends_on.push(dependency.id.clone());
        plan.children.push(dependency.id.clone());
        let measure = || {
            let _scope = HotScope::enter();
            let before = work::snapshot();
            let output = prerequisites(&store, &plan, &task).unwrap();
            let counts = work::snapshot().since(&before);
            counts.check_identity().unwrap();
            assert_eq!(counts.unattributed_hydrations(), 0);
            (serde_json::to_value(output).unwrap(), counts)
        };
        let (before, base) = measure();
        assert_eq!(before[0]["task"], dependency.id);
        assert_eq!(before[0]["satisfied"], true);
        assert_eq!(before[0]["external"], false);
        for i in 0..10 {
            store
                .create("Unrelated", Some("other"), Status::Dropped, "")
                .unwrap();
            store.log_event(None, "unrelated", json!({"i":i})).unwrap();
        }
        let (after, grown) = measure();
        assert_eq!(before, after);
        for counter in Counter::ALL {
            assert_eq!(base.get(counter), grown.get(counter), "{counter:?}");
        }
    }
}
