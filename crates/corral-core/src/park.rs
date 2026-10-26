//! Deliberate, revision-bound parking. Checks describe canonical evidence, never commands.
use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Responsible, ResumeCheck, Status, Store, Task};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Park {
    pub version: u32,
    pub revision: u64,
    pub by: Responsible,
    pub reason: String,
    pub at: Timestamp,
    pub resume_when: Vec<ResumeCheck>,
    pub resume_action: String,
    pub review_by: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<Timestamp>,
}

impl Park {
    pub const fn supports(status: Status) -> bool {
        matches!(status, Status::Ready | Status::Review | Status::NeedsMe)
    }

    pub fn refusal(&self, task: &str) -> String {
        format!(
            "task parked by {} at revision {}; use `corral attention unpark {task} --expected-revision {}`",
            self.by.label(),
            self.revision,
            self.revision
        )
    }

    pub fn require_unparked(task: &Task) -> Result<()> {
        if let Some(park) = &task.park {
            anyhow::bail!("{}", park.refusal(&task.id));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported park version");
        ensure!(
            matches!(self.by, Responsible::Pm | Responsible::Owner),
            "park requires PM or owner responsibility"
        );
        ensure!(
            !self.reason.trim().is_empty() && self.reason.len() <= 2048,
            "park reason must contain 1..2048 bytes"
        );
        ensure!(
            self.resume_action.len() <= 1024,
            "park resume_action exceeds 1024 bytes"
        );
        ensure!(
            self.resume_when.len() <= 8,
            "park permits at most 8 resume checks"
        );
        let seconds = self.review_by.duration_since(self.at).as_secs();
        ensure!(
            (3600..=604800).contains(&seconds),
            "park review_by must be 1h..7d"
        );
        for check in &self.resume_when {
            validate_check(check)?;
        }
        Ok(())
    }
}

/// Parse the supported CLI vocabulary once, before any task mutation.
pub fn parse_park_check(value: &str) -> Result<ResumeCheck> {
    let (kind, value) = value
        .split_once(':')
        .context("resume-when requires kind:target")?;
    let (target, expected) = match kind {
        "status" => {
            let (id, status) = value
                .split_once('=')
                .context("status requires task=status")?;
            let status = Status::parse(status).context("unknown resume status")?;
            (Store::normalize_id(id)?, Some(status.as_str().to_owned()))
        }
        "landed" => (Store::normalize_id(value)?, None),
        "answered" => {
            let (key, revision) = value
                .rsplit_once('@')
                .context("answered requires key@revision")?;
            ensure!(!key.is_empty(), "answered requires a question key");
            let revision = revision
                .parse::<u32>()
                .context("invalid question revision")?;
            ensure!(revision > 0, "question revision must be positive");
            (key.to_owned(), Some(revision.to_string()))
        }
        "decision" | "hold-lifted" => (value.to_owned(), None),
        _ => anyhow::bail!("unknown or Manual resume condition: {kind}"),
    };
    let check = ResumeCheck {
        kind: kind.into(),
        target: Some(target),
        expected,
        description: format!("{kind}:{value}"),
        machine_checkable: true,
        satisfied: None,
    };
    validate_check(&check)?;
    Ok(check)
}

fn validate_check(check: &ResumeCheck) -> Result<()> {
    ensure!(
        check.machine_checkable
            && check
                .target
                .as_ref()
                .is_some_and(|target| !target.is_empty() && target.len() <= 512)
            && check.description.len() <= 1024,
        "park resume condition must be bounded and machine-checkable"
    );
    match check.kind.as_str() {
        "status" => {
            ensure!(
                check.expected.as_deref().and_then(Status::parse).is_some(),
                "unknown resume status"
            );
            Store::normalize_id(check.target.as_deref().context("missing task")?)?;
        }
        "landed" => {
            Store::normalize_id(check.target.as_deref().context("missing task")?)?;
            ensure!(check.expected.is_none(), "landed takes only a task");
        }
        "answered" => ensure!(
            check
                .expected
                .as_ref()
                .is_some_and(|value| value.parse::<u32>().is_ok_and(|n| n > 0)),
            "answered requires a positive revision"
        ),
        "decision" | "hold-lifted" => {
            ensure!(
                check.expected.is_none(),
                "resume condition does not accept an expected value"
            );
        }
        _ => anyhow::bail!("unknown or Manual resume condition: {}", check.kind),
    }
    Ok(())
}
