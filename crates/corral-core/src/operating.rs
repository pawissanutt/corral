//! Workspace-only, finite operating grants and durable one-action provenance.
//!
//! Permissions narrow existing executors; they never replace candidate verification,
//! dependency admission, resource checks, independent review, or recovery.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::plan_contract_revision;
use crate::{
    ActionAuthorization, AuthorizedSubmission, Config, OperatingActionKind, OperatingOrigin,
    PolicySource, ReviewPolicy, Reviewer, Status, Store, Task,
};

mod membership;
mod projects;
mod publication;

pub use projects::{DEFAULT_GRANT_DAYS, ScopedProject};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingMode {
    #[default]
    Assist,
    Autopilot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperatingAuthority {
    pub dispatch: bool,
    pub accept: bool,
    pub plan_accept: bool,
    /// Final tracker Plan acceptance, never implied by `accept` or `plan_accept`.
    /// Omitted when off so grants confirmed before this field keep their digest.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub aggregate_accept: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedTask {
    pub project: String,
    pub repository: PathBuf,
    pub repository_identity: String,
    pub contract_revision: String,
}

/// Exact reviewed scope labels, not recursive permission for a plan's descendants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedPlan {
    pub task: String,
    pub revision: String,
    pub artifact_sha256: String,
    pub scopes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperatingScope {
    pub tasks: BTreeMap<String, ScopedTask>,
    pub exclusions: BTreeSet<String>,
    pub plans: Vec<ScopedPlan>,
    /// Projects whose unfinished tasks, including ones filed later, are covered.
    /// Omitted when empty so grants confirmed before this field keep their digest.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, ScopedProject>,
    /// The whole grant ends this many days after owner confirmation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_after_days: Option<u32>,
    /// Most tasks filed after confirmation that project grants cover.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_new_tasks: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperatingPolicy {
    pub mode: OperatingMode,
    pub paused: bool,
    /// Changes on scope/authority confirmation, not ordinary execution progress.
    pub revision: u64,
    pub authorization: Option<String>,
    pub authority: OperatingAuthority,
    pub scope: OperatingScope,
}

impl OperatingPolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub(crate) fn grant_digest(&self) -> Result<String> {
        // Pause/resume retains the approval, but is always checked at submission.
        digest(&(
            &self.mode,
            self.revision,
            &self.authorization,
            &self.authority,
            &self.scope,
        ))
    }
}

/// A selection is resolved again at confirmation; project selection is not persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperatingSelection {
    pub projects: BTreeSet<String>,
    pub tasks: BTreeSet<String>,
    pub exclusions: BTreeSet<String>,
    /// Parent task -> reviewed scope labels on its latest plan revision.
    pub plan_scopes: BTreeMap<String, BTreeSet<String>>,
    pub authority: OperatingAuthority,
    /// Selected projects also cover tasks filed after confirmation, within the limits below.
    /// Omitted when off so existing preview identities do not change.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub include_new: bool,
    /// Defaults to [`DEFAULT_GRANT_DAYS`] when `include_new` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_after_days: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_new_tasks: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperatingPreview {
    pub id: String,
    /// Exact policy hash retained in the confirmed approval receipt.
    pub grant_digest: String,
    pub workspace: PathBuf,
    pub source_revision: String,
    pub selection: OperatingSelection,
    pub policy: OperatingPolicy,
    /// Includes explicit owner exceptions and all existing verification restrictions.
    pub constraints: BTreeMap<String, OperatingTaskConstraints>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperatingTaskConstraints {
    pub review: ReviewPolicy,
    pub owner_acceptance: bool,
    pub owner_plan_acceptance: bool,
    pub dispatch_hold: bool,
    pub lifecycle: Status,
    pub pending_decisions: Vec<crate::DecisionRequest>,
    pub cancelled_plan: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatingApproval {
    pub authorization: String,
    pub revision: u64,
    pub workspace: PathBuf,
    pub digest: String,
    pub confirmed_by: String,
    pub confirmed_at: Timestamp,
}

/// A transport receipt is not a decision resolution. The key binds the full request identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatingPresentation {
    pub request_digest: String,
    pub delivery: String,
    pub confirmed: bool,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatingState {
    pub version: u32,
    pub approval: Option<OperatingApproval>,
    #[serde(default)]
    pub presentations: BTreeMap<String, OperatingPresentation>,
}

impl Default for OperatingState {
    fn default() -> Self {
        Self {
            version: 1,
            approval: None,
            presentations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveTaskPolicy {
    pub review: ReviewPolicy,
    pub acceptance_source: PolicySource,
    pub plan_acceptance_source: PolicySource,
    /// An absent action is allowed; entries contain concrete narrowing reasons.
    pub refusals: BTreeMap<OperatingActionKind, Vec<String>>,
}

impl EffectiveTaskPolicy {
    pub fn allows(&self, action: OperatingActionKind) -> bool {
        !self.refusals.contains_key(&action)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectivePolicy {
    pub mode: OperatingMode,
    pub paused: bool,
    pub revision: String,
    pub authorization: Option<String>,
    pub tasks: BTreeMap<String, EffectiveTaskPolicy>,
    pub reasons: Vec<String>,
}

impl EffectivePolicy {
    pub fn allows(&self, task: &str, action: OperatingActionKind) -> bool {
        self.tasks
            .get(task)
            .is_some_and(|policy| policy.allows(action))
    }
}

pub(crate) fn digest(value: &impl Serialize) -> Result<String> {
    Ok(crate::config_events::content_hash(
        &serde_json::to_string(value).context("encoding operating identity")?,
    ))
}

/// Execution progress, comments, receipts and branch movement do not change a task contract.
pub fn operating_contract_revision(task: &Task) -> Result<String> {
    let original = digest(&(
        &task.id,
        &task.title,
        &task.project,
        task.body.trim_end_matches('\n'),
        &task.completion,
        &task.depends_on,
        &task.write_scope,
        &task.kind,
        &task.size,
        task.design,
        &task.plan,
        task.plan_required,
    ))?;
    // Preserve existing receipt identities when the new Plan fields are absent.
    let scoped = if task.tracker || task.approved_scope.is_some() || task.parent.is_some() {
        digest(&(
            original,
            &task.parent,
            &task.approved_scope,
            task.tracker,
            task.tracker
                .then_some((&task.children, &task.source_digest)),
        ))?
    } else {
        original
    };
    if task.approved_scope_revision != 0 || !task.approved_scope_history.is_empty() {
        digest(&(
            scoped,
            task.approved_scope_revision,
            &task.approved_scope_history,
        ))
    } else {
        Ok(scoped)
    }
}

fn task_constraints(config: &Config, task: &Task) -> OperatingTaskConstraints {
    OperatingTaskConstraints {
        review: config.task_review_policy(task),
        owner_acceptance: config.explicit_owner_acceptance(task, false),
        owner_plan_acceptance: config.explicit_owner_acceptance(task, true),
        dispatch_hold: task.dispatch_hold,
        lifecycle: task.status,
        pending_decisions: task
            .current_run()
            .map(|run| {
                run.obligations
                    .decisions
                    .iter()
                    .filter(|decision| decision.pending())
                    .map(|decision| decision.request.clone())
                    .collect()
            })
            .unwrap_or_default(),
        cancelled_plan: task
            .plan_runs
            .last()
            .is_some_and(|run| run.state == crate::PlanRunState::Cancelled),
    }
}

fn file_identity(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).with_context(|| format!("identifying {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        digest(&(metadata.dev(), metadata.ino(), metadata.created().ok()))
    }
    #[cfg(not(unix))]
    digest(&(
        path,
        metadata
            .created()
            .context("repository creation identity unavailable")?,
    ))
}

impl Store {
    /// Strict and read-only: never substitutes the ordinary reload cache after a bad edit.
    pub(crate) fn operating_config(&self) -> Result<Config> {
        self.check_runtime()?;
        let layers = self.config_layers()?;
        crate::config_events::parse_layers(&layers, self.root(), true).map_err(anyhow::Error::msg)
    }

    /// Fingerprint a task and its execution Plan for grants and one-action consent.
    ///
    /// # Errors
    /// Rejects unavailable repositories and invalid execution Plan membership.
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

    pub(crate) fn policy_revision(&self, config: &Config) -> Result<String> {
        // Only policy inputs participate; unrelated personal/UI edits are not consent changes.
        // A dispatch pause is an admission hold, not withdrawal of queued owner intent.
        // Executors still recheck the live pause at their final submission boundary.
        let config = config.policy_view();
        let mut dispatch = config.dispatch.clone();
        dispatch.paused = false;
        digest(&(
            self.root(),
            &config.automation,
            &config.review,
            &dispatch,
            &config.projects,
            &config.limits,
            &config.resources,
        ))
    }

    /// Resolve a finite selection without writing any configuration, approval or presentation.
    pub fn preview_operating_policy(
        &self,
        selection: &OperatingSelection,
    ) -> Result<OperatingPreview> {
        let _board = self.lock("dispatch")?;
        let config = self.operating_config()?;
        self.operating_preview(&config, selection)
    }

    pub(crate) fn operating_preview(
        &self,
        config: &Config,
        selection: &OperatingSelection,
    ) -> Result<OperatingPreview> {
        if !self.is_snapshot() {
            return self
                .read_only_snapshot()?
                .operating_preview(config, selection);
        }
        let mut ids = BTreeSet::new();
        for project in &selection.projects {
            ensure!(
                config.projects.contains_key(project),
                "unknown project {project}"
            );
            // Covered live instead, so a later contract edit does not drop the task.
            if selection.include_new {
                continue;
            }
            for task in self.list()? {
                if task.project.as_ref() == Some(project)
                    && matches!(task.status, Status::Ready | Status::Backlog)
                {
                    ids.insert(task.id);
                }
            }
        }
        for id in selection.tasks.iter().chain(selection.plan_scopes.keys()) {
            ids.insert(Self::normalize_id(id)?);
        }
        let exclusions = selection
            .exclusions
            .iter()
            .map(|id| Self::normalize_id(id))
            .collect::<Result<BTreeSet<_>>>()?;
        let mut scope = OperatingScope {
            exclusions,
            ..OperatingScope::default()
        };
        self.expand_operating_plans(&mut ids, &scope.exclusions)?;
        let mut constraints = BTreeMap::new();
        for id in ids {
            if scope.exclusions.contains(&id) {
                continue;
            }
            let task = self.get(&id)?;
            scope
                .tasks
                .insert(id.clone(), self.operating_member(config, &task)?);
            constraints.insert(id, task_constraints(config, &task));
        }
        if selection.include_new {
            self.preview_projects(config, selection, &mut scope, &mut constraints)?;
        }
        ensure!(
            !scope.tasks.is_empty() || !scope.projects.is_empty(),
            "Autopilot needs a nonempty finite task scope"
        );
        for (id, labels) in &selection.plan_scopes {
            let task = self.get(id)?;
            ensure!(
                scope.tasks.contains_key(&task.id),
                "excluded plan parent {}",
                task.id
            );
            let plan = task
                .plans
                .last()
                .context("selected task has no reviewed plan")?;
            ensure!(
                plan.spec_revision == plan_contract_revision(&task)?,
                "reviewed plan contract is stale"
            );
            ensure!(
                plan.reviews
                    .last()
                    .is_some_and(|review| review.verdict == Some(crate::Verdict::Approve)),
                "plan needs an independent approving review"
            );
            let mut scopes = BTreeMap::new();
            ensure!(
                !labels.is_empty(),
                "plan selection needs exact scope labels"
            );
            for label in labels {
                let matching = plan
                    .document
                    .dependencies
                    .iter()
                    .filter(|scope| &scope.task == label)
                    .collect::<Vec<_>>();
                ensure!(
                    matching.len() == 1,
                    "unknown or ambiguous reviewed scope {label}"
                );
                scopes.insert(label.clone(), digest(&matching[0])?);
            }
            scope.plans.push(ScopedPlan {
                task: task.id,
                revision: plan.id.clone(),
                artifact_sha256: plan.artifact.sha256.clone(),
                scopes,
            });
        }
        let revision = config
            .automation
            .revision
            .checked_add(1)
            .context("operating revision exhausted")?;
        let source_revision = self.policy_revision(config)?;
        let id = digest(&(
            self.root(),
            &source_revision,
            selection,
            &scope,
            &constraints,
        ))?;
        let policy = OperatingPolicy {
            mode: OperatingMode::Autopilot,
            paused: false,
            revision,
            authorization: Some(id.clone()),
            authority: selection.authority.clone(),
            scope,
        };
        Ok(OperatingPreview {
            id,
            grant_digest: policy.grant_digest()?,
            workspace: self.root().to_owned(),
            source_revision,
            selection: selection.clone(),
            policy,
            constraints,
        })
    }

    /// A matching receipt is necessary but never sufficient for any executor's safety checks.
    pub fn effective_operating_policy(&self) -> Result<EffectivePolicy> {
        let config = self.operating_config()?;
        self.resolve_operating_policy(&config)
    }

    pub(crate) fn resolve_operating_policy(&self, config: &Config) -> Result<EffectivePolicy> {
        // Membership checks look up every task per member; read the board once, now.
        if !self.is_snapshot() {
            return self.read_only_snapshot()?.resolve_operating_policy(config);
        }
        let policy = &config.automation;
        let state = self.operating_state()?;
        let expired = state
            .approval
            .as_ref()
            .and_then(|approval| projects::expiry(&policy.scope, approval))
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
            if matches!(
                task.status,
                Status::NeedsMe | Status::Dropped | Status::Done
            ) {
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
    }
}

fn deny_all(policy: &mut EffectiveTaskPolicy, reasons: &[String]) {
    if !reasons.is_empty() {
        for action in ACTIONS {
            policy.refusals.insert(action, reasons.to_vec());
        }
    }
}

impl Store {
    pub(crate) fn submission_path(&self, id: &str) -> Result<PathBuf> {
        ensure!(
            id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid action authorization identity"
        );
        Ok(self
            .state_dir()
            .join("action-authorizations")
            .join(format!("{id}.json")))
    }

    pub fn authorized_submission(&self, id: &str) -> Result<AuthorizedSubmission> {
        let path = self.submission_path(id)?;
        let record: AuthorizedSubmission = serde_json::from_str(
            &fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
        )
        .with_context(|| format!("parsing {}", path.display()))?;
        ensure!(
            record.version == 1 && record.authorization.id == id,
            "mismatched submission record"
        );
        Ok(record)
    }

    pub(crate) fn save_authorized_submission(&self, record: &AuthorizedSubmission) -> Result<()> {
        let path = self.submission_path(&record.authorization.id)?;
        fs::create_dir_all(path.parent().context("submission has no parent")?)
            .context("creating authorization journal")?;
        crate::store::atomic_write(
            &path,
            &serde_json::to_string(record).context("encoding authorization")?,
        )
    }

    pub(crate) fn validate_operating_action(
        &self,
        config: &Config,
        task: &Task,
        authorization: &ActionAuthorization,
    ) -> Result<()> {
        ensure!(
            authorization.workspace == self.root() && authorization.action.task == task.id,
            "action workspace/task identity changed"
        );
        ensure!(
            authorization.member == self.operating_member(config, task)?,
            "action task contract or project identity changed"
        );
        ensure!(
            authorization.policy_revision == self.policy_revision(config)?,
            "action policy revision changed"
        );
        match &authorization.origin {
            OperatingOrigin::Automatic {
                authorization: grant,
                revision,
            } => {
                let effective = self.resolve_operating_policy(config)?;
                ensure!(
                    effective.authorization.as_ref() == Some(grant)
                        && *revision == config.automation.revision,
                    "automatic grant was revoked"
                );
                ensure!(
                    effective.allows(&task.id, authorization.action.kind),
                    "automatic action is not authorized: {:?}",
                    effective.tasks.get(&task.id).map(|task| &task.refusals)
                );
            }
            OperatingOrigin::ExplicitOwner {
                approval,
                confirmed_by,
                ..
            } => {
                ensure!(
                    !approval.trim().is_empty() && !confirmed_by.trim().is_empty(),
                    "explicit owner approval is missing"
                );
            }
        }
        if let Some(candidate) = &authorization.action.candidate {
            ensure!(
                candidate.task == task.id && candidate.run == authorization.action.run,
                "candidate does not match action task/run"
            );
        }
        Ok(())
    }
}
