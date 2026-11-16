//! What a served plan record is (a design or the tracker it published) and the record it is
//! linked to, as corrald states them; and the approved design's state, which names its tracker.
//! A design and its tracker may share a title, so these say which is which; titles are never
//! matched to guess a link.

use corral_protocol::detail::PlanDetail;
use corral_protocol::rows::{PlanRole, PlanRow as WirePlan, PlanState as WireState};

use super::live::{project_of, state_chip, title};
use super::{Action, ApprovedView, Button, Chip, Header, LiveAction};
use crate::model::Id;
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;

/// The linked record's id: corrald names a source design as `T-7@P3`, a tracker as `T-11`.
pub(super) fn linked_id(linked: &str) -> Id {
    Id::from(linked.split_once('@').map_or(linked, |(id, _)| id))
}

/// The linked record as the app writes a plan and its revision: `T-7 P3`.
pub(super) fn linked_label(linked: &str) -> String {
    linked.replacen('@', " ", 1)
}

/// What a plan record is, and the record it is linked to: "design · published T-11",
/// "tracker of T-7 P3". None when corrald does not say (an older daemon) or for other tasks.
pub(super) fn role_line(role: Option<PlanRole>, linked: Option<&str>) -> Option<String> {
    Some(match (role?, linked) {
        (PlanRole::Design, Some(tracker)) => {
            format!("design · published {}", linked_label(tracker))
        }
        (PlanRole::Design, None) => "design".to_owned(),
        (PlanRole::Tracker, Some(design)) => format!("tracker of {}", linked_label(design)),
        (PlanRole::Tracker, None) => "tracker".to_owned(),
        (PlanRole::Other, _) => return None,
    })
}

/// The plan's role, from its detail when served, else from its row.
pub(super) fn role(row: Option<&WirePlan>, detail: &PlanDetail) -> Option<PlanRole> {
    detail.role.or_else(|| row.and_then(|row| row.role))
}

/// The record a plan is linked to, from its detail when served, else from its row.
pub(super) fn linked(row: Option<&WirePlan>, detail: &PlanDetail) -> Option<String> {
    detail
        .linked
        .clone()
        .or_else(|| row.and_then(|row| row.linked.clone()))
}

/// An approved design: its identity (state, role, id and revision, title) in the shared head,
/// then whether it is published as its tracker (with the way there) or not yet published. Ids
/// stay in the head and on the buttons, which never break them across lines.
pub(super) fn approved(
    id: &Id,
    row: Option<&WirePlan>,
    detail: &PlanDetail,
    stale: Option<String>,
    env: Env,
) -> ApprovedView {
    let approvals = detail
        .reviews
        .iter()
        .filter(|review| review.verdict.starts_with("approve"))
        .count();
    let phone = env.form == Form::Phone;
    let tracker = linked(row, detail).map(|tracker| (linked_id(&tracker), linked_label(&tracker)));
    let mut actions: Vec<Button> = tracker
        .iter()
        .map(|(tracker_id, label)| {
            Button::new(
                format!("Open tracker {label}"),
                Action::Live(LiveAction::Plan(tracker_id.clone())),
            )
        })
        .collect();
    if detail.document.is_some() {
        actions.push(Button::new(
            match &detail.revision {
                Some(rev) => format!("Open document {rev}"),
                None => "Open document".to_owned(),
            },
            Action::Live(LiveAction::Document(id.clone())),
        ));
    }
    let mut chips = vec![state_chip(WireState::Approved)];
    if let Some(PlanRole::Design) = role(row, detail) {
        chips.push(Chip {
            label: "design".into(),
            hue: Hue::Neutral,
        });
    }
    let approvals_said = match approvals {
        0 => String::new(),
        1 => "1 approval. ".to_owned(),
        n => format!("{n} approvals. "),
    };
    ApprovedView {
        header: Some(Header {
            plan: Some(id.clone()),
            chips,
            id: detail
                .revision
                .as_ref()
                .map_or_else(|| id.to_string(), |rev| format!("{id} {rev}")),
            project: project_of(row),
            title: title(id, row),
            meta: vec![],
            illustrative: None,
            actions: vec![],
            stale,
        }),
        heading: match (&tracker, phone) {
            (Some(_), true) => "Approved · published".into(),
            (Some(_), false) => "Approved and published as a tracker".into(),
            (None, true) => "Approved · not started".into(),
            (None, false) => "Approved, no tracker published".into(),
        },
        body: match &tracker {
            Some(_) => format!(
                "{approvals_said}Its members are delivered by the published tracker; open it to follow them."
            ),
            None => format!("{approvals_said}Publishing members is a separate owner action."),
        },
        actions,
        stale: None,
    }
}
