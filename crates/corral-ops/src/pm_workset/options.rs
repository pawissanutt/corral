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

/// Whether the effective policy lets Corral's executor run this option for its task.
pub(super) fn delegated(
    action: &PmAction,
    option: &crate::pm_next::PmOption,
    policy: &EffectivePolicy,
) -> bool {
    option_authority(&option.id, action.kind)
        .zip(option_target(
            action.kind,
            action.task.as_deref(),
            &option.id,
        ))
        .is_some_and(|(kind, task)| {
            if action.kind == PmActionKind::Diagnosis {
                policy.allows_diagnosis(task)
            } else {
                policy.allows(task, kind)
            }
        })
}

pub(super) fn project_permissions(action: &mut PmAction, policy: &EffectivePolicy) {
    // Executor delegation and owner-only decisions are separate contracts.
    for index in 0..action.options.len() {
        let allowed = delegated(action, &action.options[index], policy);
        let option = &mut action.options[index];
        // The owner's grant already decided a delegated option; it needs no further owner input.
        if allowed {
            option.requires_owner = false;
        }
        option.pm_runs = !option.requires_owner && option.id != "wait" && !allowed;
        if allowed {
            option.label = match option.id.as_str() {
                "dispatch_owner" => "Start delegated work".into(),
                "accept_owner" => "Accept verified delegated work".into(),
                "approve_plan" => "Accept the delegated reviewed plan".into(),
                "review_plan" => "Start delegated independent plan review".into(),
                _ => option.label.clone(),
            };
        }
    }
    action.requires_owner = action
        .options
        .iter()
        .find(|option| option.id == action.recommended)
        .is_none_or(|option| option.requires_owner);
    if action.kind == PmActionKind::Owner
        && let Some((id, command)) = ["approve_plan", "review_plan"].iter().find_map(|id| {
            action
                .options
                .iter()
                .find(|option| &option.id == id && delegated(action, option, policy))
                .map(|option| (option.id.clone(), option.command.clone()))
        })
    {
        action.recommended = id;
        action.command = command;
        action.requires_owner = false;
        action.verify = "Inspect the exact plan's fresh review and acceptance using plan show --json; no owner-authored verdict or fabricated decision response is supplied.".into();
    }
}
