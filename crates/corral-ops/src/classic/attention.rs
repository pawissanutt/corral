//! The attention part: the board, its operating text, and the two identities that decide
//! whether either must be recomputed or republished.
//!
//! Two attention boards for one store sequence differ in `Condition::age_secs`, which
//! `attention::condition` (crates/corral-ops/src/attention.rs) and the pass's publication
//! (attention/handling.rs) derive as `now - since` at assessment time. The board renders ages
//! from `since` at draw time and the PM workset never reads `age_secs`, so neither identity
//! includes it.
use anyhow::Result;
use corral_core::attention::Condition;
use jiff::Timestamp;
use serde::Serialize;

use super::digest;
use crate::Ctx;
use crate::attention::AttentionBoard;

pub(super) type Board = Result<AttentionBoard, String>;

/// Why the operating text's workset runs on this refresh; None reuses the cached text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Recompute {
    /// An explicit refresh.
    Forced,
    /// The periodic (or first, config or feed-overflow) recomputation of every part.
    Periodic,
    /// A non-lease store change since the last refresh.
    Store,
    /// The workset's attention inputs changed.
    Attention,
}

impl Recompute {
    pub(super) fn span(self) -> &'static str {
        match self {
            Self::Forced => "classic.workset.recompute.forced",
            Self::Periodic => "classic.workset.recompute.periodic",
            Self::Store => "classic.workset.recompute.store",
            Self::Attention => "classic.workset.recompute.attention",
        }
    }
}

/// The workset is a function of the store and of its attention inputs: it recomputes only
/// when one of them changed (or on a forced or periodic recomputation of every part).
pub(super) fn recompute(
    forced: bool,
    full: bool,
    store: bool,
    cached_inputs: Option<u64>,
    inputs: u64,
) -> Option<Recompute> {
    if forced {
        Some(Recompute::Forced)
    } else if full {
        Some(Recompute::Periodic)
    } else if store {
        Some(Recompute::Store)
    } else if cached_inputs != Some(inputs) {
        Some(Recompute::Attention)
    } else {
        None
    }
}

/// A condition without its assessment-time age.
fn ageless(condition: &Condition) -> Condition {
    Condition {
        age_secs: None,
        ..condition.clone()
    }
}

/// Identity of the board as the frame shows it, without `age_secs`: equal content publishes
/// no new revision.
pub(super) fn content(board: &Board) -> Result<u64> {
    let Ok(board) = board else {
        return digest(board);
    };
    let mut board = board.clone();
    for row in &mut board.tasks {
        for condition in row
            .blocked
            .iter_mut()
            .chain(row.assessment.primary.iter_mut())
            .chain(row.assessment.conditions.iter_mut())
        {
            condition.age_secs = None;
        }
    }
    digest(&Ok::<_, ()>(board))
}

/// Exactly what `pm_workset_with_attention` reads from the board (pm_workset.rs
/// `project_snapshot`, pm_next/observe.rs `project_blocked`): each row's task, its blocked
/// condition (cloned into PM cards: reason, responsible, resume, next action, since), and
/// the reasons of its primary condition and progress. An unavailable board yields its error.
pub(super) fn workset_inputs(board: &Board) -> Result<u64> {
    #[derive(Serialize)]
    struct Row<'a> {
        task: &'a str,
        blocked: Option<Condition>,
        primary: Option<&'a str>,
        progress: Option<&'a str>,
    }
    let Ok(board) = board else {
        return digest(board);
    };
    let rows: Vec<Row<'_>> = board
        .tasks
        .iter()
        .map(|row| Row {
            task: &row.task,
            blocked: row.blocked.as_ref().map(ageless),
            primary: row
                .assessment
                .primary
                .as_ref()
                .map(|primary| primary.reason.as_str()),
            progress: row
                .assessment
                .progress
                .as_ref()
                .map(|progress| progress.reason.as_str()),
        })
        .collect();
    digest(&Ok::<_, ()>(rows))
}

/// The attention board at `now`. A partial listing must never become a falsely reassuring
/// attention count: a file diagnostic makes the whole read unavailable.
pub(super) fn at(ctx: &Ctx, now: Timestamp) -> Board {
    let (sender, messages) = std::sync::mpsc::channel();
    let result = {
        let _capture = corral_core::capture_messages(sender);
        crate::attention::board_hot(ctx, now)
    };
    let mut unavailable = None;
    for message in messages.try_iter() {
        if message.file.is_some() {
            unavailable.get_or_insert(message.text);
        } else {
            corral_core::report_message(message.task.as_deref(), message.source, message.text);
        }
    }
    if let Some(error) = unavailable {
        return Err(error);
    }
    result.map_err(|error| format!("{error:#}"))
}

/// The operating header read back with the frame's attention, as `board_readback` does. On
/// the frame's pinned ctx the workset evaluates on that same view (no view of its own); its
/// live-store preparation ran before the read.
pub(super) fn operating(ctx: &Ctx, attention: &Board) -> String {
    let workset = attention
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
        .and_then(|board| {
            let _span = corral_core::timing::span("board.workset");
            crate::pm_workset_with_attention(ctx, board)
        });
    crate::pm_read::readback_result(workset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::attention::{Assessment, NextAction, Responsible, SinceSource, WaitClass};

    fn board(age: i64, reason: &str) -> Board {
        let condition = Condition {
            key: None,
            kind: "review_idle".into(),
            class: WaitClass::PmAction,
            reason_code: "review_idle".into(),
            reason: reason.into(),
            responsible: Responsible::Pm,
            since: Some(Timestamp::UNIX_EPOCH),
            since_source: SinceSource::Canonical,
            age_secs: Some(age),
            resume: Vec::new(),
            next_action: NextAction {
                command: None,
                description: "inspect".into(),
                requires_owner: false,
                recipient: None,
            },
        };
        Ok(AttentionBoard {
            version: 1,
            tasks: vec![crate::attention::TaskAssessment {
                task: "T-001".into(),
                title: "Task".into(),
                project: None,
                status: corral_core::Status::Review,
                blocked: Some(condition.clone()),
                assessment: Assessment {
                    conditions: vec![condition.clone()],
                    progress: None,
                    primary: Some(condition),
                },
            }],
        })
    }

    #[test]
    fn workset_recomputes_only_when_its_attention_inputs_change() {
        // Given the workset's inputs from a board computed earlier for the same store.
        let cached = workset_inputs(&board(10, "review idle")).unwrap();
        // When a later computation differs only in the conditions' assessment-time age.
        let aged = board(70, "review idle");
        // Then neither the workset nor the published content is recomputed.
        assert_eq!(
            recompute(
                false,
                false,
                false,
                Some(cached),
                workset_inputs(&aged).unwrap()
            ),
            None
        );
        assert_eq!(
            content(&aged).unwrap(),
            content(&board(10, "review idle")).unwrap()
        );
        // And a board whose blocked reason (read by PM cards) changed recomputes it.
        let changed = workset_inputs(&board(70, "review stalled")).unwrap();
        assert_eq!(
            recompute(false, false, false, Some(cached), changed),
            Some(Recompute::Attention)
        );
    }
}
