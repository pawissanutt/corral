//! Closed owner-feed vocabulary, shared by selection, totals and row typing.
use corral_protocol::app::ChangeKind;

pub(super) const KINDS: &[(&str, ChangeKind)] = &[
    ("created", ChangeKind::Created),
    ("moved", ChangeKind::Moved),
    ("discarded", ChangeKind::Work),
    ("done", ChangeKind::Work),
    ("gate_started", ChangeKind::Gate),
    ("gate", ChangeKind::Gate),
    ("verdict", ChangeKind::Review),
    ("approved", ChangeKind::Land),
    ("landing_stale", ChangeKind::Land),
    ("needs_me", ChangeKind::Attention),
    ("stuck", ChangeKind::Attention),
    ("blocked", ChangeKind::Attention),
    ("escalated", ChangeKind::Attention),
    ("held", ChangeKind::Attention),
    ("released", ChangeKind::Attention),
    ("commented", ChangeKind::Comment),
    ("annotation_opened", ChangeKind::Annotation),
    ("annotation_replied", ChangeKind::Annotation),
    ("annotation_resolved", ChangeKind::Annotation),
    ("annotation_reopened", ChangeKind::Annotation),
    ("automation_blocked", ChangeKind::Automation),
    ("automation_resumed", ChangeKind::Automation),
    ("autopilot_error", ChangeKind::Automation),
    ("operating_policy_confirmed", ChangeKind::Policy),
    ("queued", ChangeKind::Queue),
    ("dispatch_failed", ChangeKind::Queue),
    ("question_opened", ChangeKind::Question),
    ("question_answered", ChangeKind::Question),
    ("question_escalated", ChangeKind::Question),
    ("question_expired", ChangeKind::Question),
    ("question_cancelled", ChangeKind::Question),
    ("question_withdrawn", ChangeKind::Question),
    ("question_unattended", ChangeKind::Question),
    ("question_deadline_set", ChangeKind::Question),
    ("question_followup", ChangeKind::Question),
    ("plan_submitted", ChangeKind::Plan),
    ("plan_decomposed", ChangeKind::Plan),
    ("plan_dispatched", ChangeKind::Plan),
    ("plan_failed", ChangeKind::Plan),
    ("plan_cancelled", ChangeKind::Plan),
    ("plan_verdict", ChangeKind::Plan),
    ("plan_awaiting_owner", ChangeKind::Plan),
    ("plan_escalated", ChangeKind::Plan),
    ("plan_verification_started", ChangeKind::Gate),
    ("plan_gate", ChangeKind::Gate),
    ("plan_evidence_recorded", ChangeKind::Plan),
];

pub(super) fn query() -> corral_core::backend::EventQuery {
    corral_core::backend::EventQuery {
        kinds: KINDS.iter().map(|(kind, _)| (*kind).to_owned()).collect(),
        valid_only: true,
        ..Default::default()
    }
}
