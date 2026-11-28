//! Durable counters for current observation failures, separate from lifecycle history.

use std::{borrow::Cow, collections::HashMap, io::ErrorKind};

use anyhow::{Context, Result};
use corral_herdr::{ENGINE_DOWN, EngineProblem};
use jiff::Timestamp;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    Actor, Run, Store, Task,
    events::{EventKind, NewEvent},
    runner::pi::PiRefusal,
    store::encode,
};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS run_observation_episodes (
    task TEXT NOT NULL,
    run TEXT NOT NULL,
    opened_seq INTEGER NOT NULL REFERENCES events(seq),
    document TEXT NOT NULL,
    PRIMARY KEY(task,run),
    FOREIGN KEY(task,run) REFERENCES runs(task,id)
) WITHOUT ROWID;";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Cause {
    HerdrUnavailable,
    HerdrMissing,
    HerdrIncompatible,
    PiResponseTooLarge,
    PiStarting,
    PiRefused,
    TimedOut,
    PermissionDenied,
    TransportUnavailable,
    InvalidResponse,
    IoFailed,
    Unknown,
}

impl Cause {
    fn reason(self) -> &'static str {
        match self {
            Self::HerdrUnavailable => "herdr is unavailable",
            Self::HerdrMissing => "herdr executable is missing",
            Self::HerdrIncompatible => "herdr version is incompatible",
            Self::PiResponseTooLarge => "Pi state response is too large",
            Self::PiStarting => "Pi host is starting",
            Self::PiRefused => "Pi host refused the state request",
            Self::TimedOut => "session read timed out",
            Self::PermissionDenied => "session read permission denied",
            Self::TransportUnavailable => "session transport is unavailable",
            Self::InvalidResponse => "invalid session state response",
            Self::IoFailed => "session I/O failed",
            Self::Unknown => "unclassified session read failure",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Failure {
    cause: Cause,
    signature: Option<String>,
}

impl Failure {
    pub(super) fn from_error(error: &anyhow::Error) -> Self {
        use Cause::*;
        let fixed = |cause| Self {
            cause,
            signature: None,
        };
        if let Some(problem) = error.downcast_ref::<EngineProblem>() {
            return fixed(match problem {
                EngineProblem::Missing => HerdrMissing,
                EngineProblem::TooOld(_) => HerdrIncompatible,
                EngineProblem::Down => HerdrUnavailable,
            });
        }
        if let Some(refusal) = error.downcast_ref::<PiRefusal>() {
            return match refusal.code.as_str() {
                "response_too_large" => fixed(PiResponseTooLarge),
                "starting" => fixed(PiStarting),
                // Reasons may contain per-request details; use the stable protocol code.
                code => Self::fingerprint(PiRefused, code),
            };
        }
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            return match error.kind() {
                ErrorKind::TimedOut | ErrorKind::WouldBlock => fixed(TimedOut),
                ErrorKind::PermissionDenied => fixed(PermissionDenied),
                ErrorKind::NotFound
                | ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof => fixed(TransportUnavailable),
                ErrorKind::InvalidData => fixed(InvalidResponse),
                _ => Self::fingerprint(IoFailed, &error.to_string()),
            };
        }
        if error.is::<serde_json::Error>() {
            return fixed(InvalidResponse);
        }
        // herdr exposes a fixed availability message for its private transport error.
        if corral_herdr::friendly_error(error) == ENGINE_DOWN {
            return fixed(HerdrUnavailable);
        }
        Self::fingerprint(Unknown, &error.root_cause().to_string())
    }

    /// Never persist an external error message, path, or arbitrary protocol code.
    fn fingerprint(cause: Cause, diagnostic: &str) -> Self {
        Self {
            cause,
            signature: Some(format!("{:x}", Sha256::digest(diagnostic))),
        }
    }

    pub(super) fn reason(&self) -> Cow<'static, str> {
        let reason = self.cause.reason();
        match self
            .signature
            .as_deref()
            .and_then(|signature| signature.get(..12))
        {
            Some(signature) => format!("{reason} (diagnostic {signature})").into(),
            None => reason.into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Episode {
    #[serde(flatten)]
    failure: Failure,
    observations: u64,
    first_seen: Timestamp,
    last_seen: Timestamp,
}

impl Store {
    fn current_observation_episode(&self, run: &Run) -> Result<Option<(i64, Episode)>> {
        let row: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT opened_seq,document FROM run_observation_episodes WHERE task=?1 AND run=?2",
                params![run.task, run.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("read run observation episode")?;
        row.map(|(seq, document)| {
            let episode =
                serde_json::from_str(&document).context("decode run observation episode")?;
            Ok((seq, episode))
        })
        .transpose()
    }

    pub(super) fn record_observation_failure(
        &self,
        run: &Run,
        failure: &Failure,
        now: Timestamp,
    ) -> Result<()> {
        if let Some((seq, mut episode)) = self.current_observation_episode(run)? {
            if &episode.failure == failure {
                episode.observations = episode.observations.saturating_add(1);
                episode.last_seen = episode.last_seen.max(now);
                self.connection
                    .execute(
                        "UPDATE run_observation_episodes SET document=?3 WHERE task=?1 AND run=?2",
                        params![run.task, run.id, encode(&episode)?],
                    )
                    .context("count repeated observation failure")?;
                return Ok(());
            }
            self.close_observation_episode(run, seq, &episode, "cause_changed", now)?;
        }
        let episode = Episode {
            failure: failure.clone(),
            observations: 1,
            first_seen: now,
            last_seen: now,
        };
        let seq = self.append_observation_event(run, &episode, "opened", None, now)?;
        self.connection.execute(
            "INSERT INTO run_observation_episodes(task,run,opened_seq,document) VALUES(?1,?2,?3,?4)",
            params![run.task, run.id, seq, encode(&episode)?],
        ).context("start run observation episode")?;
        Ok(())
    }

    pub(super) fn recover_observation_episode(&self, run: &Run, now: Timestamp) -> Result<()> {
        if let Some((seq, episode)) = self.current_observation_episode(run)? {
            self.close_observation_episode(run, seq, &episode, "recovered", now)?;
        }
        Ok(())
    }

    pub(crate) fn end_observation_episode(&self, run: &Run, now: Timestamp) -> Result<()> {
        if let Some((seq, episode)) = self.current_observation_episode(run)? {
            self.close_observation_episode(run, seq, &episode, "run_ended", now)?;
        }
        Ok(())
    }

    fn close_observation_episode(
        &self,
        run: &Run,
        seq: i64,
        episode: &Episode,
        state: &str,
        now: Timestamp,
    ) -> Result<()> {
        self.append_observation_event(run, episode, state, Some(seq), now)?;
        self.connection
            .execute(
                "DELETE FROM run_observation_episodes WHERE task=?1 AND run=?2",
                params![run.task, run.id],
            )
            .context("close run observation episode")?;
        Ok(())
    }

    fn append_observation_event(
        &self,
        run: &Run,
        episode: &Episode,
        state: &str,
        opened_seq: Option<i64>,
        now: Timestamp,
    ) -> Result<i64> {
        let reason = match state {
            "cause_changed" => format!(
                "Session observation failure changed after {} failed observations: {}",
                episode.observations,
                episode.failure.reason()
            ),
            "recovered" => format!(
                "Session observation recovered after {} failed observations: {}",
                episode.observations,
                episode.failure.reason()
            ),
            "run_ended" => format!(
                "Run ended before session observation recovered ({} failed observations): {}",
                episode.observations,
                episode.failure.reason()
            ),
            _ => format!("Session state unknown: {}", episode.failure.reason()),
        };
        let event = self.append_event_at(
            &NewEvent {
                task: Some(run.task.clone()),
                run: Some(run.id.clone()),
                kind: EventKind::Error,
                actor: Actor::Auto,
                detail: json!({
                    "operation": "session reconciliation",
                    "state": state,
                    "cause": episode.failure.cause,
                    "signature": episode.failure.signature,
                    "reason": reason,
                    "opened_seq": opened_seq,
                    "first_seen": episode.first_seen,
                    "last_seen": episode.last_seen,
                    "observations": episode.observations,
                }),
            },
            now,
        )?;
        i64::try_from(event.seq).context("observation event sequence exceeds SQLite integer range")
    }

    /// Add current observation summaries to a board snapshot, without changing the stored
    /// task's advisory or historical events. Look up only the requested tasks' episodes.
    /// Repeated failures change SQLite's data version; board readers refresh independently
    /// of the event cursor, which advances only at episode boundaries.
    pub fn add_observation_attention(&self, tasks: &mut [Task]) -> Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        let ids: Vec<_> = tasks.iter().map(|task| task.id.as_str()).collect();
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT e.task,e.run,e.document FROM run_observation_episodes e
             WHERE e.task IN (SELECT value FROM json_each(?1)) ORDER BY e.task,e.run",
            )
            .context("prepare board observation summaries")?;
        let mut rows = statement
            .query([encode(&ids)?])
            .context("read board observation summaries")?;
        let mut attention = HashMap::<String, Vec<String>>::new();
        while let Some(row) = rows.next().context("read board observation summary row")? {
            let task: String = row.get(0)?;
            let run: String = row.get(1)?;
            let doc: String = row.get(2)?;
            let episode: Episode =
                serde_json::from_str(&doc).context("decode board observation summary")?;
            attention.entry(task).or_default().push(format!(
                "{run} session state unknown: {} ({} failed; first {}; last {})",
                episode.failure.reason(),
                episode.observations,
                episode.first_seen,
                episode.last_seen,
            ));
        }
        for task in tasks {
            if let Some(messages) = attention.remove(&task.id) {
                let observations = messages.join("; ");
                task.attention = Some(match task.attention.take() {
                    Some(existing) => format!("{existing} {observations}"),
                    None => observations,
                });
            }
        }
        Ok(())
    }
}
