//! Native PM reads share workset policy without widening their approved project set.
#[cfg(test)]
#[path = "scope_tests.rs"]
mod tests;

use std::borrow::Cow;
use std::collections::BTreeSet;

use anyhow::Result;
use corral_core::{EffectivePolicy, Task};

use super::{PmAction, PmActionKind, PmDecision};
use crate::{Ctx, PmObservation};

#[path = "scope_inbox.rs"]
mod inbox;

pub(crate) fn next_for_projects(ctx: &Ctx, projects: &BTreeSet<String>) -> Result<PmAction> {
    super::project(ctx, None, Some(projects)).map(|(workset, _)| workset.next)
}

pub(super) struct ProjectScope {
    tasks: Option<BTreeSet<String>>,
    projects: Option<BTreeSet<String>>,
    policy: Option<EffectivePolicy>,
    promotion: Option<PmObservation>,
}

impl ProjectScope {
    pub(super) fn new(
        tasks: &[Task],
        projects: Option<&BTreeSet<String>>,
        policy: &EffectivePolicy,
    ) -> Self {
        Self {
            tasks: projects.map(|projects| {
                tasks
                    .iter()
                    .filter(|task| {
                        task.project
                            .as_ref()
                            .is_some_and(|project| projects.contains(project))
                    })
                    .map(|task| task.id.clone())
                    .collect()
            }),
            projects: projects.cloned(),
            policy: projects.map(|_| policy.clone()),
            promotion: None,
        }
    }

    pub(super) fn inbox(&self, ctx: &Ctx, bin: &str) -> Result<Vec<PmAction>> {
        crate::pm_next::inbox::cards(ctx, bin, |item| {
            inbox::includes(ctx, self.projects.as_ref(), item)
        })
    }

    pub(super) fn enrich(
        &self,
        ctx: &Ctx,
        action: PmAction,
        allow_lessons: bool,
    ) -> Result<PmAction> {
        crate::pm_next::enrich(ctx, action, allow_lessons, self.projects.as_ref())
    }

    pub(super) fn includes(&self, id: &str) -> bool {
        self.tasks.as_ref().is_none_or(|tasks| tasks.contains(id))
    }

    pub(super) fn observations(&mut self, observations: &mut Vec<PmObservation>) {
        if self.tasks.is_none() {
            return;
        }
        observations.retain_mut(|observation| {
            if matches!(observation, PmObservation::IdleIdeas { .. }) {
                self.promotion = Some(observation.clone());
            }
            if let PmObservation::IdleIdeas {
                ideas, shortlist, ..
            } = observation
            {
                ideas.retain(|id| self.includes(id));
                shortlist.retain(|candidate| self.includes(&candidate.id));
                !ideas.is_empty()
            } else {
                observation.task().is_some_and(|id| self.includes(id))
            }
        });
    }

    /// Scope can trim a promotion's sibling options, but its executable ID must still
    /// match the canonical workspace card that pm_act revalidates. Only the hash uses it.
    pub(super) fn canonical_action<'a>(
        &self,
        action: &'a PmAction,
        bin: &str,
    ) -> Cow<'a, PmAction> {
        if action.kind == PmActionKind::IdeaPromotion
            && let Some((observation, policy)) = self.promotion.as_ref().zip(self.policy.as_ref())
        {
            let mut canonical = super::pm_card(observation, bin);
            super::project_permissions(&mut canonical, policy);
            return Cow::Owned(canonical);
        }
        Cow::Borrowed(action)
    }

    pub(super) fn decisions(&self, decisions: &mut Vec<PmDecision>) {
        if self.tasks.is_none() {
            return;
        }
        // A previously presented card may carry a mixed-project shortlist. Do not leak its
        // options/evidence or rewrite its durable identity; the scoped observation builds
        // a fresh card instead. Taskless durable workspace decisions are likewise excluded.
        decisions.retain(|decision| {
            self.includes(&decision.task)
                && decision
                    .action
                    .shortlist
                    .iter()
                    .all(|candidate| self.includes(&candidate.id))
                && decision.action.options.iter().all(|option| {
                    super::option_target(
                        decision.action.kind,
                        decision.action.task.as_deref(),
                        &option.id,
                    )
                    .is_none_or(|id| self.includes(id))
                })
        });
    }
}
