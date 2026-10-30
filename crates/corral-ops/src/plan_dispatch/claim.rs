//! The current design-run submission, retained independently of its process slot.
use std::io::Write;

use anyhow::{Context, Result, ensure};
use corral_core::{HerdrRef, OperatingActionIdentity, OperatingActionKind, TaskRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Claim {
    pub run: String,
    pub attempt: String,
    pub round: u32,
    pub previous_round: u32,
    pub plan: Option<String>,
    pub review: Option<String>,
    pub notes: Option<String>,
    pub force_resources: bool,
    pub command: Option<String>,
    pub herdr: Option<HerdrRef>,
}

impl Claim {
    pub fn identity(&self, task: &str) -> OperatingActionIdentity {
        let mut action =
            crate::operating::identity(task, &self.run, OperatingActionKind::Plan, &self.attempt);
        action.round = Some(self.round);
        action.plan = self.plan.clone();
        action
    }

    pub fn check<T: TaskRef + ?Sized>(&self, task: &T) -> Result<()> {
        let run = task.last_plan_run_head()?.context("plan run missing")?;
        ensure!(
            run.id == self.run && run.rounds == self.previous_round,
            "prepared planner run or revision round changed"
        );
        if let Some(plan) = &self.plan {
            let current = task
                .last_plan_head()?
                .filter(|current| &current.id == plan)
                .context("prepared planner revision belongs to a different plan")?;
            ensure!(
                current
                    .current_review
                    .as_ref()
                    .is_some_and(|review| Some(&review.id) == self.review.as_ref()
                        && review.verdict == Some(corral_core::Verdict::Changes)
                        && review.notes == self.notes),
                "prepared planner revision review or notes changed"
            );
        }
        ensure!(
            run.pending_reply == self.notes,
            "prepared planner revision notes changed"
        );
        Ok(())
    }

    pub fn save<T: TaskRef + ?Sized>(&self, task: &T) -> Result<()> {
        let run = task
            .last_plan_run_head()?
            .filter(|run| run.id == self.run)
            .context("planner claim run replaced")?;
        std::fs::create_dir_all(&run.output_dir).context("creating planner claim directory")?;
        let path = run.output_dir.join("launch.json");
        let mut file =
            tempfile::NamedTempFile::new_in(&run.output_dir).context("staging planner claim")?;
        file.write_all(&serde_json::to_vec(self)?)
            .context("writing planner claim")?;
        file.persist(&path).context("publishing planner claim")?;
        Ok(())
    }
}

pub(super) fn load<T: TaskRef + ?Sized>(task: &T) -> Result<Option<Claim>> {
    let Some(run) = task.last_plan_run_head()? else {
        return Ok(None);
    };
    let path = run.output_dir.join("launch.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading planner claim"),
    };
    let claim: Claim = serde_json::from_slice(&bytes).context("parsing planner claim")?;
    ensure!(claim.run == run.id, "planner claim identity changed");
    Ok(Some(claim))
}
