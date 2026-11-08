//! Full-task oracle ported from db316644; keep its board scans independent of head resolution.
use super::*;
use std::ops::Deref;
#[path = "oracle_membership_tests.rs"]
mod membership;
#[path = "oracle_projects_tests.rs"]
mod projects;
#[path = "oracle_publication_tests.rs"]
mod publication;
pub(super) struct Oracle<'a>(pub &'a Store);
impl Deref for Oracle<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        self.0
    }
}
impl Oracle<'_> {
    pub fn operating_member(&self, config: &Config, task: &Task) -> Result<ScopedTask> {
        let project = task.project.as_ref().context("task has no project")?;
        let (repository, repository_identity) = self.project_identity(config, project)?;
        let contract = operating_contract_revision(task)?;
        let plan = if task.tracker {
            None
        } else {
            self.execution_plan(task)?
        };
        let contract_revision = match plan {
            Some(plan) => digest(&(contract, operating_contract_revision(&plan)?))?,
            None => contract,
        };
        Ok(ScopedTask {
            project: project.clone(),
            repository_identity,
            repository,
            contract_revision,
        })
    }
    pub(crate) fn resolve_operating_policy(&self, config: &Config) -> Result<EffectivePolicy> {
        // Membership checks look up every task per member; read the board once, now.
        let policy = &config.automation;
        let state = self.operating_state()?;
        let expired = state
            .approval
            .as_ref()
            .and_then(|approval| crate::operating::projects::expiry(&policy.scope, approval))
            .filter(|end| *end <= Timestamp::now());
        let valid = state.approval.as_ref().is_some_and(|approval| {
            policy.mode == OperatingMode::Autopilot
                && expired.is_none()
                && (!policy.scope.tasks.is_empty() || !policy.scope.projects.is_empty())
                && policy.authorization.as_ref() == Some(&approval.authorization)
                && approval.revision == policy.revision
                && approval.workspace == self.root()
                && !approval.confirmed_by.trim().is_empty()
                && policy
                    .grant_digest()
                    .is_ok_and(|digest| digest == approval.digest)
        });
        let mut effective = EffectivePolicy {
            mode: if valid {
                OperatingMode::Autopilot
            } else {
                OperatingMode::Assist
            },
            paused: policy.paused,
            revision: self.policy_revision(config)?,
            authorization: valid.then(|| policy.authorization.clone()).flatten(),
            tasks: BTreeMap::new(),
            reasons: Vec::new(),
        };
        if let Some(end) = expired {
            effective
                .reasons
                .push(format!("the grant expired at {end}; confirm a new one"));
        } else if !valid {
            effective.reasons.push(
                "no matching confirmed workspace grant; legacy automation is not consent".into(),
            );
        }
        let (members, publication_reasons) = self.operating_members(config);
        let plan_refusals = self.operating_plan_refusals(config, &members);
        effective.reasons.extend(publication_reasons);
        for (id, member) in &members {
            let task = self.get(id);
            let mut common = Vec::new();
            if !valid {
                common.extend(effective.reasons.clone());
            }
            if policy.paused {
                common.push("operating policy is paused".into());
            }
            if policy.scope.exclusions.contains(id) {
                common.push("task is explicitly excluded".into());
            }
            if let Some(reasons) = plan_refusals.get(id) {
                common.extend(reasons.clone());
            }
            let task = match task {
                Ok(task) => task,
                Err(error) => {
                    common.push(format!("task unavailable: {error:#}"));
                    let mut refused = task_policy(config, None);
                    deny_all(&mut refused, &common);
                    effective.tasks.insert(id.clone(), refused);
                    continue;
                }
            };
            if task.dispatch_hold {
                common.push("task dispatch hold".into());
            }
            match self.operating_member(config, &task) {
                Ok(current) if &current == member => {}
                Ok(_) => common.push("task contract or project identity changed".into()),
                Err(error) => common.push(format!("scope identity unavailable: {error:#}")),
            }
            for grant in policy
                .scope
                .plans
                .iter()
                .filter(|grant| grant.task == task.id)
            {
                let matching = task.plans.last().filter(|plan| {
                    plan.id == grant.revision
                        && plan.artifact.sha256 == grant.artifact_sha256
                        && plan
                            .reviews
                            .last()
                            .is_some_and(|review| review.verdict == Some(crate::Verdict::Approve))
                        && plan_contract_revision(&task)
                            .is_ok_and(|revision| plan.spec_revision == revision)
                        && grant.scopes.iter().all(|(label, expected)| {
                            plan.document
                                .dependencies
                                .iter()
                                .find(|scope| &scope.task == label)
                                .is_some_and(|scope| {
                                    digest(scope).is_ok_and(|actual| &actual == expected)
                                })
                        })
                });
                if matching.is_none() {
                    common.push("reviewed plan scope changed".into());
                }
            }
            if matches!(task.status, Status::Dropped | Status::Done) {
                common.push(format!("task lifecycle is {}", task.status.as_str()));
            }
            let has_owner_decisions = match task.plans.last() {
                Some(plan) => crate::plan_policy::owner_decisions(&task, plan),
                None => task
                    .completion
                    .as_ref()
                    .is_some_and(|manifest| !manifest.owner_decisions.is_empty()),
            };
            let unresolved = has_owner_decisions
                && crate::plan_policy::validate_owner_resolution(self, config, &task).is_err();
            // Owner changes notes resolving every decision admit only the revision they request.
            let revision = unresolved && crate::plan_policy::revision_resolved(&task);
            if task
                .current_run()
                .is_some_and(|run| run.obligations.pending_decision().is_some())
                || (unresolved && !revision)
            {
                common.push("unresolved owner decision".into());
            }
            if task
                .plan_runs
                .last()
                .is_some_and(|run| run.state == crate::PlanRunState::Cancelled)
            {
                common.push("planning run is cancelled".into());
            }
            let mut resolved = task_policy(config, Some(&task));
            deny_all(&mut resolved, &common);
            for action in ACTIONS {
                let mut reasons = Vec::new();
                let aggregate = action == OperatingActionKind::AcceptAggregate;
                if task.tracker && !aggregate {
                    reasons.push("tracker Plan is not executable".into());
                } else if aggregate && !task.plan_acceptance_required() {
                    reasons.push(
                        "only tracker Plans with acceptance criteria have aggregate acceptance"
                            .into(),
                    );
                }
                let delegated = match action {
                    OperatingActionKind::Accept => policy.authority.accept,
                    OperatingActionKind::AcceptPlan => policy.authority.plan_accept,
                    OperatingActionKind::AcceptAggregate => policy.authority.aggregate_accept,
                    OperatingActionKind::Dispatch
                    | OperatingActionKind::Plan
                    | OperatingActionKind::Gate
                    | OperatingActionKind::Review
                    | OperatingActionKind::PlanReview
                    | OperatingActionKind::Repair => policy.authority.dispatch,
                };
                if !delegated {
                    reasons.push("action was not delegated".into());
                }
                if revision && action != OperatingActionKind::Plan {
                    reasons.push("unresolved owner decision".into());
                }
                if config.review.autopilot == Some(false) {
                    reasons.push("legacy review automation is explicitly disabled".into());
                }
                if matches!(
                    action,
                    OperatingActionKind::Dispatch
                        | OperatingActionKind::Plan
                        | OperatingActionKind::Repair
                ) {
                    if config.dispatch.paused {
                        reasons.push("dispatch is paused".into());
                    }
                    if !config.ready_project_allowed(task.project.as_deref()) {
                        reasons.push("project is not admitted".into());
                    }
                    let dispatch = config.dispatch_policy(task.project.as_deref());
                    if task.status == Status::Backlog && !dispatch.auto_backlog {
                        reasons.push("automatic backlog admission is disabled".into());
                    }
                    if task.status == Status::Idea
                        && dispatch.promote_ideas != crate::PromoteIdeas::Auto
                    {
                        reasons.push("automatic Idea promotion is disabled".into());
                    }
                }
                if matches!(
                    action,
                    OperatingActionKind::Accept | OperatingActionKind::AcceptAggregate
                ) {
                    if config.explicit_owner_acceptance(&task, false)
                        || (aggregate && config.explicit_owner_acceptance(&task, true))
                    {
                        reasons.push("explicit owner-only acceptance".into());
                    }
                    if resolved.review.gate.is_empty()
                        || matches!(resolved.review.reviewer, Reviewer::None | Reviewer::Pm)
                    {
                        reasons.push(
                            "automatic acceptance requires a gate and an independent reviewer"
                                .into(),
                        );
                    }
                }
                if action == OperatingActionKind::AcceptPlan
                    && config.explicit_owner_acceptance(&task, true)
                {
                    reasons.push("explicit owner-only plan acceptance".into());
                }
                if action == OperatingActionKind::Repair {
                    resolved.diagnosis_refusals.extend(reasons.clone());
                }
                if task.status == Status::NeedsMe {
                    reasons.push(format!("task lifecycle is {}", task.status.as_str()));
                }
                if action == OperatingActionKind::Repair
                    && task.rounds >= resolved.review.max_rounds
                {
                    reasons.push("automatic repair rounds exhausted".into());
                }
                if !reasons.is_empty() {
                    resolved.refusals.entry(action).or_default().extend(reasons);
                }
            }
            effective.tasks.insert(id.clone(), resolved);
        }
        Ok(effective)
    }
}
const ACTIONS: [OperatingActionKind; 9] = [
    OperatingActionKind::Dispatch,
    OperatingActionKind::Plan,
    OperatingActionKind::Gate,
    OperatingActionKind::Review,
    OperatingActionKind::PlanReview,
    OperatingActionKind::Repair,
    OperatingActionKind::Accept,
    OperatingActionKind::AcceptPlan,
    OperatingActionKind::AcceptAggregate,
];

fn task_policy(config: &Config, task: Option<&Task>) -> EffectiveTaskPolicy {
    let project = task.and_then(|task| task.project.as_deref());
    let sources = config.review_policy_sources(project, task.and_then(|task| task.review));
    EffectiveTaskPolicy {
        review: task.map_or_else(
            || config.review_policy(None, None),
            |task| config.task_review_policy(task),
        ),
        acceptance_source: sources.accept,
        plan_acceptance_source: sources.plan_accept,
        refusals: BTreeMap::new(),
        diagnosis_refusals: Vec::new(),
    }
}

fn deny_all(policy: &mut EffectiveTaskPolicy, reasons: &[String]) {
    policy.diagnosis_refusals.extend_from_slice(reasons);
    if !reasons.is_empty() {
        for action in ACTIONS {
            policy.refusals.insert(action, reasons.to_vec());
        }
    }
}
