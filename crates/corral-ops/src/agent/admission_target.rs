//! What a launcher binds a generation to: the named container and its role.
use corral_core::agent::{AgentRole, ContainerRef};

/// What a launcher binds: never chosen by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindTarget {
    Planner {
        task: String,
        run: String,
    },
    PlanReviewer {
        task: String,
        plan: String,
        review: String,
    },
    Worker {
        task: String,
        run: String,
    },
    Reviewer {
        task: String,
        run: String,
        review: String,
        callback: String,
    },
    Pm {
        project: String,
    },
    Assistant {
        session: String,
    },
}

impl BindTarget {
    pub fn container(&self) -> ContainerRef {
        match self {
            BindTarget::Planner { task, run } => ContainerRef::Planner {
                task: task.clone(),
                run: run.clone(),
            },
            BindTarget::PlanReviewer { task, plan, review } => ContainerRef::PlanReviewer {
                task: task.clone(),
                plan: plan.clone(),
                review: review.clone(),
            },
            BindTarget::Worker { task, run } => ContainerRef::Worker {
                task: task.clone(),
                run: run.clone(),
            },
            BindTarget::Reviewer {
                task, run, review, ..
            } => ContainerRef::Reviewer {
                task: task.clone(),
                run: run.clone(),
                review: review.clone(),
            },
            BindTarget::Pm { .. } => ContainerRef::Pm,
            BindTarget::Assistant { session } => ContainerRef::Assistant {
                session: session.clone(),
            },
        }
    }

    pub fn role(&self) -> AgentRole {
        match self {
            BindTarget::Worker { .. } => AgentRole::Worker,
            BindTarget::Reviewer { .. } => AgentRole::Reviewer,
            BindTarget::Planner { .. } => AgentRole::Planner,
            BindTarget::PlanReviewer { .. } => AgentRole::PlanReviewer,
            BindTarget::Pm { .. } => AgentRole::Pm,
            BindTarget::Assistant { .. } => AgentRole::Assistant,
        }
    }
}
