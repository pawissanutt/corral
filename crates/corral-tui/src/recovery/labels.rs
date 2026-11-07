//! Text labels for owner recovery. Exceptions are named in words, never conveyed by color alone, and the
//! original observation stays beside its owner disposition.

use corral_core::{
    CheckDisposition, RecoveryCheckRecord, RecoveryEffect, RecoveryIdentity, RecoveryObservation,
    RecoveryRecord,
};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::theme::{self, FG, YELLOW};

pub(crate) const OWNER_RECOVERY: &str = "owner recovery";
pub(crate) const REVIEW_WAIVED: &str = "review waived";
pub(crate) const CHECK_OVERRIDDEN: &str = "check overridden";

pub(crate) fn not_policy() -> &'static str {
    "owner recovery, not ordinary policy success"
}

/// Kebab-case wire name of a serde enum value (`reviewer-family-policy`, `plan-review`, …). A macro, since
/// this crate names no `serde` trait bound; tagged values report their tag.
macro_rules! wire {
    ($value:expr) => {
        match serde_json::to_value($value) {
            Ok(serde_json::Value::String(s)) => s,
            Ok(serde_json::Value::Object(map)) => map
                .get("kind")
                .or_else(|| map.get("action"))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string(),
            _ => "?".to_string(),
        }
    };
}
pub(crate) use wire;

fn excepted(c: &RecoveryCheckRecord) -> bool {
    matches!(
        c.disposition,
        CheckDisposition::Overridden | CheckDisposition::Waived
    )
}

/// Badges for a task's durable recovery history, the same derivation cards read from heads.
pub(crate) fn badges(records: &[RecoveryRecord]) -> Vec<&'static str> {
    summary_badges(&corral_core::RecoverySummary::of(records))
}

/// History badges from the bounded summary. Exception labels mean an operation actually applied
/// them; uncertain, failed and pending operations are named separately. Empty without records.
pub(crate) fn summary_badges(s: &corral_core::RecoverySummary) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (n, label) in [
        (s.applied, OWNER_RECOVERY),
        (s.review_waived, REVIEW_WAIVED),
        (s.check_overridden, CHECK_OVERRIDDEN),
        (s.unknown, "recovery uncertain"),
        (s.failed, "recovery failed"),
        (s.unsettled, "recovery pending"),
    ] {
        if n > 0 {
            out.push(label);
        }
    }
    out
}

pub(crate) fn observation(o: &RecoveryObservation) -> String {
    match o {
        RecoveryObservation::Predicate { satisfied: true } => "satisfied".into(),
        RecoveryObservation::Predicate { satisfied: false } => "not satisfied".into(),
        RecoveryObservation::Gate { result } => format!("gate {}", result.as_str()),
        RecoveryObservation::Review { verdict: Some(v) } => format!("review {}", v.as_str()),
        RecoveryObservation::Review { verdict: None } => "review pending".into(),
        RecoveryObservation::Missing => "missing".into(),
        RecoveryObservation::Unverifiable { reason } => format!("unverifiable: {reason}"),
    }
}

pub(crate) fn disposition(d: CheckDisposition) -> &'static str {
    match d {
        CheckDisposition::Satisfied => "satisfied",
        CheckDisposition::Overridden => CHECK_OVERRIDDEN,
        CheckDisposition::Waived => "waived",
        CheckDisposition::NotApplicable => "not applicable",
    }
}

/// `check: original observation → disposition`, the original first.
pub(crate) fn check_line(c: &RecoveryCheckRecord) -> String {
    format!(
        "{}: original {} → {}",
        wire!(&c.check),
        observation(&c.observed),
        disposition(c.disposition)
    )
}

pub(crate) fn identity(i: &RecoveryIdentity) -> String {
    match i {
        RecoveryIdentity::Plan {
            plan,
            artifact_sha256,
            ..
        } => {
            format!("plan {plan} @{}", short(artifact_sha256))
        }
        RecoveryIdentity::Code { candidate, .. } => {
            format!("run {} @{}", candidate.run, short(&candidate.head))
        }
        RecoveryIdentity::Admission { stage, attempt, .. } => {
            format!("{} attempt {attempt}", wire!(stage))
        }
    }
}

pub(crate) fn effect(e: &RecoveryEffect) -> String {
    match e {
        RecoveryEffect::ReviewAttempt {
            stage,
            attempt,
            reviewer,
        } => format!(
            "one {} attempt {attempt} by {} ({} · {}); reviewer overrides and policy unchanged",
            wire!(stage),
            reviewer.profile,
            reviewer.model,
            reviewer.vendor
        ),
        RecoveryEffect::PlanAcceptance {
            plan,
            dispatch_holds,
        } => format!(
            "accept plan {plan}; dispatch stays held on {}",
            if dispatch_holds.is_empty() {
                "—".into()
            } else {
                dispatch_holds.join(", ")
            }
        ),
        RecoveryEffect::CodeAcceptance { candidate, merge } => format!(
            "accept run {} @{} with merge {}",
            candidate.run,
            short(&candidate.head),
            wire!(merge)
        ),
        RecoveryEffect::Admission { stage, attempt } => format!(
            "admit {} attempt {attempt} past resource pressure only",
            wire!(stage)
        ),
    }
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Acceptance-history section: every operation with its reason, attestation and original checks.
pub(crate) fn history_lines(records: &[RecoveryRecord]) -> Vec<Line<'static>> {
    let mut out = vec![Line::styled("Owner recovery", theme::bold())];
    if records.is_empty() {
        out.push(Line::styled("  none", theme::dim()));
        return out;
    }
    for r in records {
        let state = r.outcomes.last().map_or("authorized".into(), |o| {
            format!("{:?}", o.state).to_lowercase()
        });
        out.push(Line::from(vec![
            Span::styled(format!("  [{OWNER_RECOVERY}] "), Style::new().fg(YELLOW)),
            Span::styled(
                format!(
                    "{} {} · {state} · {}",
                    wire!(&r.preview.request.action),
                    identity(&r.preview.request.target.identity),
                    short(&r.id.replace("REC-", ""))
                ),
                Style::new().fg(FG),
            ),
        ]));
        out.push(Line::styled(format!("    {}", not_policy()), theme::dim()));
        out.push(Line::styled(
            format!("    reason: {}", r.authorization.reason),
            Style::new().fg(FG),
        ));
        let corral_core::RecoveryAttestation::LocalOwnerAttestation {
            interaction, at, ..
        } = &r.authorization.attestation;
        out.push(Line::styled(
            format!(
                "    attested: local owner via {} at {}",
                wire!(interaction),
                at.strftime("%Y-%m-%d %H:%M")
            ),
            theme::dim(),
        ));
        out.push(Line::styled(
            format!("    effect: {}", effect(&r.preview.proposed_effect)),
            theme::dim(),
        ));
        for c in &r.preview.checks {
            let style = if excepted(c) {
                Style::new().fg(YELLOW)
            } else {
                theme::dim()
            };
            out.push(Line::styled(format!("    {}", check_line(c)), style));
        }
        for link in &r.links {
            out.push(Line::styled(
                format!("    receipt: {}", wire!(&link.receipt)),
                theme::dim(),
            ));
        }
    }
    out
}
