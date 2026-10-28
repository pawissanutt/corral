//! Tracker selection freezes direct task contracts in the ordinary finite scope.
use super::ScopedTask;
use crate::backend::HeadQuery;
use crate::{Config, Status, Store, Task, TaskHead};
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
        self.execution_plan_fields(task)?
            .map(|plan| self.get(&plan.id))
            .transpose()
    }

    /// Head-only form of [`Store::execution_plan`], on the caller's read snapshot.
    ///
    /// # Errors
    /// Returns the same membership and lookup errors as the full form.
    pub fn execution_plan_head(&self, task: &TaskHead) -> Result<Option<TaskHead>> {
        if task.tracker && task.approved_scope.is_some() {
            self.check_operating_plan(&task.to_shell_task()?)?;
            return Ok(Some(task.clone()));
        }
        self.execution_plan_fields(&task.to_shell_task()?)
    }

    pub(super) fn execution_plan_fields<T: crate::TaskRef + ?Sized>(
        &self,
        task: &T,
    ) -> Result<Option<TaskHead>> {
        let owners = self
            .heads(&HeadQuery::EdgeTo {
                kind: "child".into(),
                other: task.id().clone(),
            })?
            .into_iter()
            .filter(|plan| plan.tracker)
            .collect::<Vec<_>>();
        let parent = task
            .parent()
            .as_deref()
            .map(|id| self.head(id))
            .transpose()?;
        let Some(plan) = parent.filter(|plan| plan.tracker) else {
            ensure!(owners.is_empty(), "task has a nonreciprocal Plan owner");
            return Ok(None);
        };
        ensure!(
            owners.len() == 1 && owners[0].id == plan.id && plan.children.contains(task.id()),
            "task must have exactly one reciprocal Plan owner"
        );
        ensure!(
            *task.project() == plan.project,
            "Plan child project differs"
        );
        if plan.approved_scope.is_none() {
            return Ok(None);
        }
        self.check_operating_plan(&plan.to_shell_task()?)?;
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
        let key = format!(
            "operating_plan\0{}",
            super::digest(&(
                &plan.id,
                &plan.project,
                &plan.parent,
                &plan.children,
                plan.tracker,
                plan.approved_scope.is_some(),
            ))?
        );
        let checked = self.pass_derived(&key, || {
            Ok(self
                .check_operating_plan_uncached(plan)
                .map_err(|error| format!("{error:#}")))
        })?;
        checked.as_ref().clone().map_err(anyhow::Error::msg)
    }

    fn check_operating_plan_uncached(&self, plan: &Task) -> Result<()> {
        ensure!(plan.tracker, "Plan is no longer a tracker");
        ensure!(plan.approved_scope.is_some(), "Plan has no approved scope");
        ensure!(!plan.children.is_empty(), "Plan has no children");
        let parent = plan.parent.as_deref().map(|id| self.head(id)).transpose()?;
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
                .head(id)
                .with_context(|| format!("Plan child {id} missing"))?;
            ensure!(
                child.parent.as_ref() == Some(&plan.id) && child.project == plan.project,
                "Plan child {id} parent or project differs"
            );
        }
        // Retain the old board order for the first failing reciprocal check.
        let mut related = BTreeMap::new();
        for task in self.heads(&HeadQuery::Children {
            parent: plan.id.clone(),
        })? {
            related.insert(task.num, task);
        }
        for id in std::iter::once(&plan.id).chain(&plan.children) {
            for task in self.heads(&HeadQuery::EdgeTo {
                kind: "child".into(),
                other: id.clone(),
            })? {
                related.insert(task.num, task);
            }
        }
        for task in related.values() {
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
        let mut by_parent = BTreeMap::<String, BTreeSet<String>>::new();
        for id in members.keys() {
            if let Ok(task) = self.head(id) {
                if task.tracker {
                    plans.insert(id.clone());
                }
                if let Some(parent) = &task.parent {
                    plans.insert(parent.clone());
                    by_parent
                        .entry(parent.clone())
                        .or_default()
                        .insert(id.clone());
                }
            }
        }
        for id in plans {
            let plan = self.head(&id);
            if plan
                .as_ref()
                .is_ok_and(|task| !task.tracker || task.approved_scope.is_none())
            {
                // Ordinary design and legacy tracking parents are not execution Plans.
                // A changed frozen contract is still checked for each member below.
                if !members.contains_key(&id)
                    || plan.as_ref().is_ok_and(|task| {
                        self.operating_member_head(config, task)
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
                self.check_operating_plan(&plan.to_shell_task()?)?;
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
                        &self.operating_member_head(config, plan)? == expected,
                        "Plan scope, target or membership changed"
                    );
                    for child in &plan.children {
                        if config.automation.scope.exclusions.contains(child) {
                            continue;
                        }
                        if let Some(expected) = members.get(child) {
                            ensure!(
                                &self.operating_member_head(config, &self.head(child)?)?
                                    == expected,
                                "Plan child {child} contract changed"
                            );
                        }
                    }
                }
                Ok::<_, anyhow::Error>(())
            })();
            if let Err(error) = result {
                let reason = format!("Plan {id}: {error:#}");
                let mut related = by_parent.get(&id).cloned().unwrap_or_default();
                related.insert(id.clone());
                if let Ok(plan) = &plan {
                    related.extend(plan.children.iter().cloned());
                }
                for member in related {
                    if members.contains_key(&member) {
                        refusals.entry(member).or_default().push(reason.clone());
                    }
                }
            }
        }
        refusals
    }
}
