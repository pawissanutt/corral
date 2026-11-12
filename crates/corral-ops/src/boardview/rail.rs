use corral_core::{AutoStage, GateResult, RunHead, RunOutcome, TaskHead, Verdict};
use corral_protocol::board::{RailGlyph as Glyph, RailStep};
use jiff::Timestamp;

pub(super) fn steps(head: &TaskHead) -> [RailStep; 4] {
    let Some(run) = head.current_run.as_deref() else {
        return std::array::from_fn(|_| step(Glyph::Unavailable, None, None));
    };
    let candidate = candidate(run);
    let run_glyph = match run.outcome {
        None => Glyph::Active,
        Some(RunOutcome::Done) => Glyph::Pass,
        Some(RunOutcome::Failed | RunOutcome::Interrupted) => Glyph::Fail,
        Some(_) => Glyph::Unavailable,
    };
    let gate = run.gate.as_ref().map_or_else(
        || step(Glyph::Pending, None, None),
        |gate| {
            let glyph = match gate.result {
                GateResult::Pass => Glyph::Pass,
                GateResult::Running if !gate.process_lost => Glyph::Active,
                GateResult::Fail => Glyph::Fail,
                GateResult::Stale => Glyph::Stale,
                _ => Glyph::Unavailable,
            };
            step(
                freshness(glyph, &gate.sha, candidate),
                Some(&gate.sha),
                Some(gate.at),
            )
        },
    );
    let review = run.review.as_ref().map_or_else(
        || step(Glyph::Pending, None, None),
        |review| {
            let glyph = if review.cancelled || review.process_lost {
                Glyph::Unavailable
            } else {
                match review.verdict {
                    None => Glyph::Active,
                    Some(Verdict::Approve) => Glyph::Pass,
                    Some(Verdict::Changes | Verdict::Escalate) => Glyph::Fail,
                }
            };
            step(
                freshness(glyph, &review.sha, candidate),
                Some(&review.sha),
                Some(review.at),
            )
        },
    );
    let stage = run.autopilot.as_ref();
    let land = if let Some(receipt) = head.landed.as_ref().filter(|receipt| receipt.run == run.id) {
        step(
            Glyph::Pass,
            Some(&receipt.sha),
            stage
                .filter(|stage| stage.stage == AutoStage::Done)
                .map(|stage| stage.at),
        )
    } else if let Some(stage) = stage.filter(|stage| {
        matches!(
            stage.stage,
            AutoStage::Landing | AutoStage::Done | AutoStage::Blocked
        )
    }) {
        let glyph = match stage.stage {
            AutoStage::Done => Glyph::Pass,
            AutoStage::Landing => Glyph::Active,
            _ => Glyph::Pending,
        };
        step(glyph, stage.sha.as_deref(), Some(stage.at))
    } else {
        step(Glyph::Pending, None, None)
    };
    [
        step(run_glyph, candidate, run.ended.or(Some(run.started))),
        gate,
        review,
        land,
    ]
}

/// Latest *recorded* candidate, never a Git probe and never a gate's own SHA used to
/// declare that gate fresh. A newer checkpoint supersedes the automation-stage candidate.
/// Unrecorded worktree movement cannot be detected until a writer publishes its candidate.
fn candidate(run: &RunHead) -> Option<&str> {
    let checkpoint = run.facts.latest_checkpoint.as_ref();
    let stage = run
        .autopilot
        .as_ref()
        .filter(|stage| stage.stage != AutoStage::Done && stage.sha.is_some());
    match (checkpoint, stage) {
        (Some(checkpoint), Some(stage)) if stage.at > checkpoint.captured => stage.sha.as_deref(),
        (Some(checkpoint), _) => Some(checkpoint.head.as_str()),
        (_, Some(stage)) => stage.sha.as_deref(),
        _ => None,
    }
}

fn freshness(glyph: Glyph, sha: &str, candidate: Option<&str>) -> Glyph {
    match candidate {
        Some(candidate) if sha != candidate => Glyph::Stale,
        // A receipt without a known comparator cannot assert candidate freshness.
        None if matches!(glyph, Glyph::Pass) => Glyph::Unavailable,
        _ => glyph,
    }
}

fn step(glyph: Glyph, sha: Option<&str>, at: Option<Timestamp>) -> RailStep {
    RailStep {
        glyph,
        sha7: sha
            .filter(|sha| !sha.is_empty())
            .map(|sha| super::row::clip(sha, 7)),
        at: at.map(|at| at.to_string()),
    }
}
