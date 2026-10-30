//! Native sync discovers unfinished containers through scalar owner indexes, not task history.
use super::TaskHead;
use crate::agent::ContainerRef;
use crate::backend::{RowQuery, TaskPart};
use crate::work::Counter;
use anyhow::{Context, Result};
use std::collections::BTreeMap;

fn id(doc: &serde_json::Value) -> Result<String> {
    doc.get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .context("native container identity missing")
}

impl TaskHead {
    fn native_parent_id(
        &self,
        part: TaskPart,
        key: &str,
        cache: &mut BTreeMap<(TaskPart, String), String>,
    ) -> Result<String> {
        let coordinate = (part, key.to_owned());
        if let Some(id) = cache.get(&coordinate) {
            return Ok(id.clone());
        }
        let current = match part {
            TaskPart::Run if self.seqs.run.is_some_and(|n| super::codec::seq(n) == key) => {
                self.current_run.as_deref().map(|run| run.id.clone())
            }
            TaskPart::Plan if self.seqs.plan.is_some_and(|n| super::codec::seq(n) == key) => {
                self.last_plan.as_deref().map(|plan| plan.id.clone())
            }
            _ => None,
        };
        let value = match current {
            Some(id) => id,
            None => {
                let mut query = RowQuery::part(part);
                query.key = Some(key.to_owned());
                let row = self
                    .query(&query, 1, Counter::NestedRowDecodes)?
                    .pop()
                    .context("native container parent missing")?;
                id(&row.doc)?
            }
        };
        cache.insert(coordinate, value.clone());
        Ok(value)
    }

    /// Unsettled or pending native owners, including retired children on terminal tasks.
    /// Retained resolved records are absent; append order is the full-task container order.
    pub fn unfinished_native_containers(&self) -> Result<Vec<ContainerRef>> {
        let mut out = BTreeMap::new();
        let mut parents = BTreeMap::new();
        for part in [
            TaskPart::Run,
            TaskPart::ReviewRun,
            TaskPart::PlanRun,
            TaskPart::PlanReview,
        ] {
            for row in self.query(
                &RowQuery::part(part).with_index("native:work"),
                u32::MAX,
                Counter::NestedRowDecodes,
            )? {
                let name = id(&row.doc)?;
                let (order, container) = match part {
                    TaskPart::Run => (
                        (0, row.key.clone(), String::new()),
                        ContainerRef::Worker {
                            task: self.id.clone(),
                            run: name,
                        },
                    ),
                    TaskPart::PlanRun => (
                        (1, row.key.clone(), String::new()),
                        ContainerRef::Planner {
                            task: self.id.clone(),
                            run: name,
                        },
                    ),
                    TaskPart::ReviewRun | TaskPart::PlanReview => {
                        let (parent, child) = row
                            .key
                            .split_once('/')
                            .context("native child coordinate malformed")?;
                        let coding = part == TaskPart::ReviewRun;
                        let parent_id = self.native_parent_id(
                            if coding {
                                TaskPart::Run
                            } else {
                                TaskPart::Plan
                            },
                            parent,
                            &mut parents,
                        )?;
                        let container = if coding {
                            ContainerRef::Reviewer {
                                task: self.id.clone(),
                                run: parent_id,
                                review: name,
                            }
                        } else {
                            ContainerRef::PlanReviewer {
                                task: self.id.clone(),
                                plan: parent_id,
                                review: name,
                            }
                        };
                        (
                            (
                                if coding { 0 } else { 2 },
                                parent.to_owned(),
                                child.to_owned(),
                            ),
                            container,
                        )
                    }
                    _ => anyhow::bail!("unexpected native container family"),
                };
                out.insert(order, container);
            }
        }
        Ok(out.into_values().collect())
    }
}
