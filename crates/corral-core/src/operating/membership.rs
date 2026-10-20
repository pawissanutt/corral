//! Tracker selection freezes direct task contracts in the ordinary finite scope.
use super::ScopedTask;
use crate::{Config, Status, Store, Task};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

impl Store {
    /// Resolve a task's direct execution Plan, never a legacy tracking parent.
    ///
    /// # Errors
    /// Rejects missing references, conflicting owners, nonreciprocal membership,
    /// and cross-project members. Reciprocal legacy trackers without approved
    /// scope resolve to `None`.
    pub fn execution_plan(&self, task: &Task) -> Result<Option<Task>> {
        if task.tracker && task.approved_scope.is_some() {
            self.check_operating_plan(task)?;
            return Ok(Some(task.clone()));
        }
        let listing = self.list_shared()?;
        let owners = listing
            .iter()
            .filter(|plan| plan.tracker && plan.children.contains(&task.id))
            .cloned()
            .collect::<Vec<_>>();
        let parent = task.parent.as_deref().map(|id| self.get(id)).transpose()?;
        let Some(plan) = parent.filter(|plan| plan.tracker) else {
            ensure!(owners.is_empty(), "task has a nonreciprocal Plan owner");
            return Ok(None);
        };
        ensure!(
            owners.len() == 1 && owners[0].id == plan.id && plan.children.contains(&task.id),
            "task must have exactly one reciprocal Plan owner"
        );
        ensure!(task.project == plan.project, "Plan child project differs");
        if plan.approved_scope.is_none() {
            return Ok(None);
        }
        self.check_operating_plan(&plan)?;
        Ok(Some(plan))
    }

    pub(super) fn expand_operating_plans(
        &self,
        ids: &mut BTreeSet<String>,
        exclusions: &BTreeSet<String>,
    ) -> Result<()> {
        // Only the original selection expands: child trackers are not recursive grants.
        let selected = ids.iter().cloned().collect::<Vec<_>>();
        for id in selected {
            if exclusions.contains(&id) {
                continue;
            }
            let plan = self.get(&id)?;
            if !plan.tracker {
                // Freeze the direct owning Plan without granting its siblings.
                // Newly inserted anchors are not revisited by this selection loop.
                if let Some(parent) = self.execution_plan(&plan)? {
                    ids.insert(parent.id);
                }
                continue;
            }
            if plan.approved_scope.is_none() {
                continue;
            }
            self.check_operating_plan(&plan)?;
            ids.extend(plan.children);
        }
        Ok(())
    }

    pub(super) fn check_operating_plan(&self, plan: &Task) -> Result<()> {
        ensure!(plan.tracker, "Plan is no longer a tracker");
        ensure!(plan.approved_scope.is_some(), "Plan has no approved scope");
        ensure!(!plan.children.is_empty(), "Plan has no children");
        let parent = plan.parent.as_deref().map(|id| self.get(id)).transpose()?;
        if let Some(parent) = &parent {
            ensure!(
                parent.id != plan.id && parent.project == plan.project,
                "Plan parent identity or project differs"
            );
            if parent.tracker {
                ensure!(
                    parent.children.contains(&plan.id),
                    "Plan parent is not reciprocal"
                );
            }
        }
        let children = plan.children.iter().collect::<BTreeSet<_>>();
        ensure!(
            children.len() == plan.children.len() && !children.contains(&plan.id),
            "Plan has duplicate or self children"
        );
        for id in children {
            let child = self
                .get(id)
                .with_context(|| format!("Plan child {id} missing"))?;
            ensure!(
                child.parent.as_ref() == Some(&plan.id) && child.project == plan.project,
                "Plan child {id} parent or project differs"
            );
        }
        for task in self.list_shared()?.iter() {
            if task.parent.as_ref() == Some(&plan.id) {
                ensure!(
                    plan.children.contains(&task.id),
                    "Plan child {} missing from membership",
                    task.id
                );
            }
            if task.tracker && task.id != plan.id {
                if task.children.contains(&plan.id) {
                    ensure!(
                        parent.as_ref().is_some_and(|parent| parent.id == task.id),
                        "Plan has a nonreciprocal owner"
                    );
                }
                ensure!(
                    !task.children.iter().any(|id| plan.children.contains(id)),
                    "Plan child has multiple owners"
                );
            }
        }
        Ok(())
    }

    pub(super) fn operating_plan_refusals(
        &self,
        config: &Config,
        members: &BTreeMap<String, ScopedTask>,
    ) -> BTreeMap<String, Vec<String>> {
        let mut refusals = BTreeMap::<String, Vec<String>>::new();
        // Parent links also retain the refusal when a frozen tracker disappears.
        let mut plans = BTreeSet::new();
        for id in members.keys() {
            if let Ok(task) = self.get(id) {
                if task.tracker {
                    plans.insert(id.clone());
                }
                if let Some(parent) = task.parent {
                    plans.insert(parent);
                }
            }
        }
        for id in plans {
            let plan = self.get(&id);
            if plan
                .as_ref()
                .is_ok_and(|task| !task.tracker || task.approved_scope.is_none())
            {
                // Ordinary design and legacy tracking parents are not execution Plans.
                // A changed frozen contract is still checked for each member below.
                if !members.contains_key(&id)
                    || plan.as_ref().is_ok_and(|task| {
                        self.operating_member(config, task)
                            .is_ok_and(|current| members.get(&id) == Some(&current))
                    })
                {
                    continue;
                }
            }
            let result = (|| {
                let plan = plan
                    .as_ref()
                    .map_err(|error| anyhow::anyhow!("{error:#}"))?;
                self.check_operating_plan(plan)?;
                ensure!(
                    !config.automation.scope.exclusions.contains(&id),
                    "Plan is explicitly excluded"
                );
                ensure!(!plan.dispatch_hold, "Plan dispatch hold");
                ensure!(
                    !matches!(plan.status, Status::Dropped | Status::NeedsMe),
                    "Plan lifecycle is {}",
                    plan.status.as_str()
                );
                if let Some(expected) = members.get(&id) {
                    ensure!(
                        &self.operating_member(config, plan)? == expected,
                        "Plan scope, target or membership changed"
                    );
                    for child in &plan.children {
                        if config.automation.scope.exclusions.contains(child) {
                            continue;
                        }
                        if let Some(expected) = members.get(child) {
                            ensure!(
                                &self.operating_member(config, &self.get(child)?)? == expected,
                                "Plan child {child} contract changed"
                            );
                        }
                    }
                }
                Ok::<_, anyhow::Error>(())
            })();
            if let Err(error) = result {
                let reason = format!("Plan {id}: {error:#}");
                for member in members.keys() {
                    let related = member == &id
                        || self
                            .get(member)
                            .is_ok_and(|task| task.parent.as_ref() == Some(&id))
                        || plan
                            .as_ref()
                            .is_ok_and(|plan| plan.children.contains(member));
                    if related {
                        refusals
                            .entry(member.clone())
                            .or_default()
                            .push(reason.clone());
                    }
                }
            }
        }
        refusals
    }
}
