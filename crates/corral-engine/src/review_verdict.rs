//! Pi writes an immutable intent in its run directory before invoking Corral.
//! Only the engine's transactional verdict event is a receipt. The spool is
//! retained with the run, but reads consider only the current authorized review.
use std::{
    fs::OpenOptions,
    io::{self, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;

use crate::{
    Engine, Outcome, PendingReviewVerdict, Role, Run, SessionRef, Status, Store, Task,
    checkout::Checkout, refuse, session_recovery::LOST_REVIEWER,
};

const INTENT_MAX: u64 = 256 * 1024;
const REASON_MAX: u64 = 4096;

pub(crate) struct ReviewRequest {
    pub(crate) id: String,
    pub(crate) sha: String,
    submission: i64,
}

impl Store {
    /// Use the submitted candidate and its event sequence, not the moving branch
    /// or a round number alone (the owner can reset and reuse review rounds).
    pub(crate) fn pi_review_request(&self, task: &Task) -> Result<ReviewRequest> {
        let (submission, sha): (i64, String) = self
            .connection
            .query_row(
                "SELECT seq, json_extract(detail,'$.sha') FROM events
                 WHERE task=?1 AND kind='done' ORDER BY seq DESC LIMIT 1",
                [&task.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("read Pi review submission")?
            .context("Pi code review requires a recorded submission")?;
        ensure!(
            matches!(sha.len(), 40 | 64)
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid candidate SHA in Pi review submission"
        );
        Ok(ReviewRequest {
            id: format!("{submission}-{}-{sha}", task.round),
            sha,
            submission,
        })
    }

    pub(crate) fn has_bound_review_request(&self, run: &Run) -> Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE task=?1 AND run=?2 AND kind='review_started'
             AND json_type(detail,'$.request')='text')",
            params![run.task, run.id], |row| row.get(0),
        ).context("check Pi reviewer protocol")
    }

    fn authorized_request(&self, task: &Task, reviewer: &Run) -> Result<Option<ReviewRequest>> {
        let waiting = match task.status {
            Status::Review => reviewer.ended.is_none(),
            Status::Blocked => {
                reviewer.outcome == Some(Outcome::Failed)
                    && task
                        .blocked
                        .as_ref()
                        .is_some_and(|b| b.reason == LOST_REVIEWER)
            }
            _ => false,
        };
        if !waiting || self.has_verdict_since_done(&task.id, task.round)? {
            return Ok(None);
        }
        if !self.has_bound_review_request(reviewer)? {
            return Ok(None);
        }
        let request = self.pi_review_request(task)?;
        let asked = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE task=?1 AND run=?2 AND kind='review_started'
             AND seq > ?3 AND json_extract(detail,'$.request')=?4)",
            params![task.id, reviewer.id, request.submission, request.id], |row| row.get::<_, bool>(0),
        ).context("authorize Pi reviewer request")?;
        Ok(asked.then_some(request))
    }

    /// A bound Pi approval authorizes only the branch head admitted with its receipt.
    /// Manual and legacy verdicts keep their existing landing semantics.
    pub(crate) fn bound_approval_sha(&self, task: &Task) -> Result<Option<String>> {
        self.connection.query_row(
            "SELECT CASE WHEN json_extract(detail,'$.approve') THEN json_extract(detail,'$.candidate') END
             FROM events WHERE task=?1 AND kind='verdict' AND json_extract(detail,'$.round')=?2
               AND seq > COALESCE((SELECT MAX(seq) FROM events WHERE task=?1 AND kind='done'),0)
             ORDER BY seq DESC LIMIT 1",
            params![task.id, task.round], |row| row.get::<_, Option<String>>(0),
        ).optional().map(Option::flatten).context("read candidate authorized by Pi approval")
    }

    pub(crate) fn recorded_review_verdict(&self, run: &Run, request: &str) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE task=?1 AND run=?2 AND kind='verdict'
             AND json_extract(detail,'$.request')=?3)",
                params![run.task, run.id, request],
                |row| row.get(0),
            )
            .context("check Pi verdict receipt")
    }
}

impl Engine {
    pub fn record_review_verdict(&self, task: &str, run: &str, request: &str) -> Result<()> {
        self.bound_verdict(task, run, request)
    }

    pub fn pending_review_verdict(&self, task: &Task) -> Result<Option<PendingReviewVerdict>> {
        let Some(run) = self
            .store
            .latest_run(&task.id, Role::Reviewer)?
            .filter(is_pi_reviewer)
        else {
            return Ok(None);
        };
        let Some(request) = self.store.authorized_request(task, &run)? else {
            return Ok(None);
        };
        let Some(path) = verdict_path(&run, &request.id) else {
            return Ok(None);
        };
        let result = read_small(&path.with_extension("json"), INTENT_MAX)
            .and_then(|bytes| bytes.map(decode_intent).transpose());
        let (verdict, reason) = match result {
            Ok(None) => return Ok(None),
            Ok(Some(intent)) => {
                let reason = read_small(&path.with_extension("error"), REASON_MAX)
                    .ok()
                    .flatten()
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "The saved verdict has no engine receipt yet.".into());
                (intent.verdict.as_str(), reason)
            }
            Err(error) => (
                "unknown",
                format!("Could not read the saved verdict: {error:#}"),
            ),
        };
        Ok(Some(PendingReviewVerdict {
            message: "Verdict not recorded".into(),
            task: task.id.clone(),
            run: run.id,
            request: request.id,
            verdict: verdict.into(),
            reason,
        }))
    }

    pub(crate) fn read_authorized_verdict(
        &self,
        store: &Store,
        task: &Task,
        reviewer: &Run,
        request: &str,
    ) -> Result<VerdictIntent> {
        if store
            .latest_run(&task.id, Role::Reviewer)?
            .is_none_or(|run| run.id != reviewer.id)
        {
            return Err(stale_request());
        }
        let Some(expected) = store
            .authorized_request(task, reviewer)?
            .filter(|r| r.id == request)
        else {
            return Err(stale_request());
        };
        let worker = self.last_worker(store, task)?;
        let checkout = Checkout::from_run(&worker)?;
        let head = corral_git::rev_sha(&checkout.repo, &checkout.branch_ref())
            .context("read current candidate before recording Pi verdict")?;
        if head != expected.sha || worker.head_sha.as_deref() != Some(&expected.sha) {
            return Err(refuse(
                "the candidate branch no longer matches this review request; submit the current candidate for review",
                vec![],
            ));
        }
        let path = verdict_path(reviewer, &expected.id).ok_or_else(stale_request)?;
        let Some(bytes) = read_small(&path.with_extension("json"), INTENT_MAX)? else {
            return Err(refuse(
                "this reviewer run has no saved verdict for that request",
                vec![],
            ));
        };
        decode_intent(bytes)
    }
}

fn verdict_path(reviewer: &Run, request: &str) -> Option<PathBuf> {
    let SessionRef::Pi { dir, .. } = reviewer.session.as_ref()? else {
        return None;
    };
    // The conversation directory can survive replacement of the reviewer run.
    Some(dir.join(format!("verdict-{}-{request}", reviewer.id)))
}

pub(crate) fn is_pi_reviewer(run: &Run) -> bool {
    run.role == Role::Reviewer && run.harness == "pi"
}

pub(crate) fn changed_approved_candidate() -> anyhow::Error {
    refuse(
        "the branch no longer matches the recorded Pi approval; send the task back and submit the current candidate for review",
        vec![],
    )
}

pub(crate) fn stale_request() -> anyhow::Error {
    refuse(
        "this is no longer the authorized Pi review request for the submitted candidate",
        vec![],
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerdictIntent {
    pub(crate) verdict: Decision,
    pub(crate) notes: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Decision {
    Approve,
    Changes,
}

impl Decision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Changes => "changes",
        }
    }
}

fn decode_intent(bytes: Vec<u8>) -> Result<VerdictIntent> {
    serde_json::from_slice(&bytes).context("decode saved Pi review verdict")
}

fn read_small(path: &Path, max: u64) -> Result<Option<Vec<u8>>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("stat {}", path.display()))?;
    ensure!(
        metadata.is_file() && metadata.len() <= max,
        "expected a small regular file at {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= max,
        "file exceeds {max} bytes: {}",
        path.display()
    );
    Ok(Some(bytes))
}
