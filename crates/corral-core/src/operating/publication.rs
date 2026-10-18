//! A reviewed plan label admits only the exact, recoverably published task contract.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use super::{ScopedPlan, ScopedTask, digest};
use crate::{Config, PlanDocument, Status, Store, Task, Verdict, operating_contract_revision};

#[derive(Deserialize)]
struct Publication {
    backlink: String,
    document: PlanDocument,
    source_contract: String,
    projected_contract: String,
    artifact_sha256: String,
    scopes: BTreeMap<String, ChildScope>,
}

#[derive(Deserialize)]
struct ChildScope {
    task: String,
    scope_digest: String,
    contract_revision: String,
}

impl Store {
    /// Includes projected parent contracts and exact children, never recursive descendants.
    pub(crate) fn operating_members(
        &self,
        config: &Config,
    ) -> (BTreeMap<String, ScopedTask>, Vec<String>) {
        let mut members = config.automation.scope.tasks.clone();
        let mut reasons = Vec::new();
        let mut grants = config.automation.scope.plans.clone();
        // No new task authority is added by a plan with no implementation scopes.
        for id in config.automation.scope.tasks.keys() {
            if grants.iter().any(|grant| &grant.task == id) {
                continue;
            }
            if let Ok(task) = self.get(id)
                && let Some(plan) = task.plans.last()
                && plan.document.dependencies.is_empty()
                && plan.approval.is_some()
            {
                grants.push(ScopedPlan {
                    task: id.clone(),
                    revision: plan.id.clone(),
                    artifact_sha256: plan.artifact.sha256.clone(),
                    scopes: BTreeMap::new(),
                });
            }
        }
        for grant in &grants {
            let path = self
                .state_dir()
                .join(format!("plan-decompose-{}.json", grant.task));
            if !path.exists() {
                continue;
            }
            let result = (|| {
                let journal: Publication = serde_json::from_slice(
                    &std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
                )
                .context("parsing scoped publication")?;
                let parent = self.get(&grant.task)?;
                let original = config
                    .automation
                    .scope
                    .tasks
                    .get(&parent.id)
                    .context("published parent is not a confirmed member")?;
                let plan = parent
                    .plans
                    .last()
                    .filter(|plan| plan.id == grant.revision)
                    .context("published plan revision changed")?;
                ensure!(
                    parent.plan.as_deref() == Some(journal.backlink.as_str())
                        && journal.backlink == format!("{}@{}", parent.id, plan.id)
                        && journal.document == plan.document
                        && journal.artifact_sha256 == plan.artifact.sha256
                        && grant.artifact_sha256 == plan.artifact.sha256
                        && plan.approval.is_some()
                        && plan
                            .reviews
                            .last()
                            .is_some_and(|review| review.verdict == Some(Verdict::Approve))
                        && crate::plan_contract_revision(&parent)? == plan.spec_revision
                        && (original.contract_revision == journal.source_contract
                            || original.contract_revision == journal.projected_contract)
                        && journal.projected_contract == operating_contract_revision(&parent)?,
                    "published parent identity changed"
                );
                let projected = self.operating_member(config, &parent)?;
                crate::plan_policy::validate_approval(self, config, &parent, plan)?;
                ensure!(
                    projected.project == original.project
                        && projected.repository == original.repository
                        && projected.repository_identity == original.repository_identity,
                    "published project identity changed"
                );
                let expected = plan
                    .document
                    .dependencies
                    .iter()
                    .map(|scope| &scope.task)
                    .collect::<BTreeSet<_>>();
                ensure!(
                    expected.len() == journal.scopes.len()
                        && journal.scopes.keys().all(|label| expected.contains(label)),
                    "published scope labels changed"
                );
                let mut published = BTreeMap::from([(parent.id.clone(), projected)]);
                if plan.document.dependencies.len() > 1 {
                    ensure!(
                        parent.status == Status::Done,
                        "parent publication barrier is incomplete"
                    );
                    let ids = journal
                        .scopes
                        .values()
                        .map(|scope| &scope.task)
                        .collect::<BTreeSet<_>>();
                    ensure!(
                        ids.len() == parent.children.len()
                            && parent.children.iter().all(|id| ids.contains(id)),
                        "published child identities changed"
                    );
                }
                for scope in &plan.document.dependencies {
                    if !grant.scopes.contains_key(&scope.task) {
                        reasons.push(format!(
                            "plan scope {}@{}:{} requires an explicit amendment",
                            parent.id, plan.id, scope.task
                        ));
                        continue;
                    }
                    let binding = journal
                        .scopes
                        .get(&scope.task)
                        .context("published scope missing")?;
                    ensure!(
                        grant.scopes.get(&scope.task) == Some(&binding.scope_digest)
                            && digest(scope)? == binding.scope_digest,
                        "unconfirmed published scope"
                    );
                    let child_member = (|| {
                        let child = self.get(&binding.task)?;
                        ensure!(
                            child.plan.as_deref() == Some(journal.backlink.as_str())
                                && child.project == parent.project
                                && operating_contract_revision(&child)?
                                    == binding.contract_revision,
                            "published child contract changed"
                        );
                        ensure!(
                            child.id == parent.id || parent.children.contains(&child.id),
                            "unknown published child"
                        );
                        let member = self.operating_member(config, &child)?;
                        ensure!(
                            member.repository_identity == original.repository_identity,
                            "published child repository changed"
                        );
                        Ok::<_, anyhow::Error>((child.id, member))
                    })();
                    match child_member {
                        Ok((id, member)) if id != parent.id => {
                            published.insert(id, member);
                        }
                        Ok(_) => {}
                        Err(error) => {
                            reasons.push(format!("plan child {}: {error:#}", binding.task))
                        }
                    }
                }
                Ok::<_, anyhow::Error>(published)
            })();
            match result {
                Ok(published) => {
                    // Explicit membership still binds the originally confirmed contract.
                    for (id, member) in published {
                        if id == grant.task || !members.contains_key(&id) {
                            members.insert(id, member);
                        }
                    }
                }
                Err(error) => reasons.push(format!("plan scope {}: {error:#}", grant.task)),
            }
        }
        (members, reasons)
    }

    /// Publication evidence without widening or rewriting the original approval receipt.
    pub fn published_operating_member(&self, task: &Task) -> Result<Option<ScopedTask>> {
        let config = self.operating_config()?;
        let effective = self.resolve_operating_policy(&config)?;
        ensure!(
            effective.authorization.is_some(),
            "no confirmed publication grant"
        );
        let (members, _) = self.operating_members(&config);
        match members.get(&task.id) {
            Some(member) => {
                ensure!(
                    &self.operating_member(&config, task)? == member,
                    "published member changed"
                );
                ensure!(
                    !config.automation.scope.exclusions.contains(&task.id),
                    "published member excluded"
                );
                Ok(Some(member.clone()))
            }
            None => Ok(None),
        }
    }
}
