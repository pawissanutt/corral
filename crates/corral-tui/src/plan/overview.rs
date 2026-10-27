//! Human review is a projection of the canonical plan, not another maintained brief.
use corral_core::PlanDocument;

use super::tab::{acceptance_text, markdown_list};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReviewField {
    Problem,
    Goal,
    Included,
    Retained,
    Exclusions,
    Choices,
    Acceptance,
    OwnerDecisions,
}

impl ReviewField {
    fn title(self) -> &'static str {
        match self {
            Self::Problem => "Why this work is needed",
            Self::Goal => "Goal",
            Self::Included => "What will change",
            Self::Retained => "Existing behavior retained",
            Self::Exclusions => "Where this work stops",
            Self::Choices => "Important choices",
            Self::Acceptance => "What counts as done",
            Self::OwnerDecisions => "Owner decisions",
        }
    }
}

pub(super) struct Change {
    pub(super) field: ReviewField,
    pub(super) removed: Vec<String>,
    pub(super) added: Vec<String>,
}

/// Compare only human-review values. File moves, task edges, commands and body edits are
/// implementation changes; reordered lists or criterion links do not imply changed behavior.
pub(super) fn changes(previous: &PlanDocument, current: &PlanDocument) -> Vec<Change> {
    let values = |d: &PlanDocument| {
        let overview = d.human_review.as_ref();
        vec![
            (
                ReviewField::Problem,
                overview
                    .map(|o| vec![o.problem.clone()])
                    .unwrap_or_default(),
            ),
            (ReviewField::Goal, vec![d.goal.clone()]),
            (
                ReviewField::Included,
                overview.map(|o| o.changes.clone()).unwrap_or_default(),
            ),
            (
                ReviewField::Retained,
                overview.map(|o| o.retained.clone()).unwrap_or_default(),
            ),
            (ReviewField::Exclusions, d.non_goals.clone()),
            (ReviewField::Choices, d.decisions.clone()),
            (
                ReviewField::Acceptance,
                d.acceptance
                    .iter()
                    .map(|a| format!("{} → {}", a.scenario, a.expected))
                    .collect(),
            ),
            (ReviewField::OwnerDecisions, d.owner_decisions.clone()),
        ]
    };
    values(previous)
        .into_iter()
        .zip(values(current))
        .filter_map(|((field, before), (_, after))| {
            if previous.human_review.is_none()
                && matches!(
                    field,
                    ReviewField::Problem | ReviewField::Included | ReviewField::Retained
                )
            {
                return None;
            }
            let removed = before
                .iter()
                .filter(|v| !after.contains(v))
                .cloned()
                .collect::<Vec<_>>();
            let added = after
                .iter()
                .filter(|v| !before.contains(v))
                .cloned()
                .collect::<Vec<_>>();
            (!removed.is_empty() || !added.is_empty()).then_some(Change {
                field,
                removed,
                added,
            })
        })
        .collect()
}

pub(super) fn revision_markdown(previous: &PlanDocument, current: &PlanDocument) -> String {
    let mut out = String::from("## Changes since the previous revision\n\n");
    if previous.human_review.is_none() {
        out.push_str("The previous plan has no structured overview. New overview content is shown below; it is not evidence of newly added scope.\n");
    }
    let delta = changes(previous, current);
    if delta.is_empty() {
        if previous.human_review.is_some() {
            out.push_str("Goal, included work, retained behavior, exclusions, choices and acceptance are unchanged. Implementation details may have changed.\n");
        } else {
            out.push_str(
                "Previously recorded goal, exclusions, choices and acceptance are unchanged.\n",
            );
        }
    }
    for change in delta {
        out.push_str(&format!("\n### {}\n", change.field.title()));
        for item in &change.removed {
            out.push_str(&format!("- Removed: {item}\n"));
        }
        for item in &change.added {
            out.push_str(&format!("- Added: {item}\n"));
        }
    }
    out
}

pub(super) fn markdown(d: &PlanDocument) -> String {
    let Some(overview) = &d.human_review else {
        return String::new();
    };
    let mut out = format!(
        "## Why this work is needed\n\n{}\n\n## Goal\n\n{}\n",
        overview.problem, d.goal
    );
    markdown_list(&mut out, "What will change", &overview.changes);
    markdown_list(&mut out, "Existing behavior retained", &overview.retained);
    markdown_list(&mut out, "Where this work stops", &d.non_goals);
    markdown_list(&mut out, "Important choices", &d.decisions);
    markdown_list(&mut out, "Owner decisions", &d.owner_decisions);
    markdown_list(
        &mut out,
        "What counts as done",
        &d.acceptance.iter().map(acceptance_text).collect::<Vec<_>>(),
    );
    out
}
