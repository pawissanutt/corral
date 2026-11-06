use super::*;

pub(crate) fn authority(kind: PmActionKind) -> Option<OperatingActionKind> {
    match kind {
        PmActionKind::GateLog | PmActionKind::Gate => Some(OperatingActionKind::Gate),
        PmActionKind::StaleVerdict | PmActionKind::LoadFailure | PmActionKind::Diagnosis => {
            Some(OperatingActionKind::Repair)
        }
        PmActionKind::Review => Some(OperatingActionKind::Review),
        PmActionKind::Land => Some(OperatingActionKind::Accept),
        PmActionKind::Dispatch | PmActionKind::IdeaPromotion => Some(OperatingActionKind::Dispatch),
        PmActionKind::LessonsDistill
        | PmActionKind::Question
        | PmActionKind::EvidenceOnly
        | PmActionKind::ReviewInFlight
        | PmActionKind::Owner
        | PmActionKind::Wait
        | PmActionKind::AgentQuestion
        | PmActionKind::OwnerQuestion
        | PmActionKind::Inbox => None,
    }
}

pub(super) fn option_authority(id: &str, kind: PmActionKind) -> Option<OperatingActionKind> {
    match id {
        "independent_review" => Some(OperatingActionKind::Review),
        "review_plan" => Some(OperatingActionKind::PlanReview),
        "approve_plan" => Some(OperatingActionKind::AcceptPlan),
        "ask_owner" | "inspect_plan" | "request_plan_changes" | "file_flake" | "wait" => None,
        _ => super::authority(kind),
    }
}

pub(super) fn option_target<'a>(
    action: PmActionKind,
    task: Option<&'a str>,
    option: &'a str,
) -> Option<&'a str> {
    match action {
        PmActionKind::IdeaPromotion => option.strip_prefix("promote_"),
        PmActionKind::LessonsDistill
        | PmActionKind::GateLog
        | PmActionKind::StaleVerdict
        | PmActionKind::Question
        | PmActionKind::EvidenceOnly
        | PmActionKind::LoadFailure
        | PmActionKind::Diagnosis
        | PmActionKind::ReviewInFlight
        | PmActionKind::Gate
        | PmActionKind::Review
        | PmActionKind::Land
        | PmActionKind::Dispatch
        | PmActionKind::Owner
        | PmActionKind::Wait
        | PmActionKind::AgentQuestion
        | PmActionKind::OwnerQuestion
        | PmActionKind::Inbox => task,
    }
}
