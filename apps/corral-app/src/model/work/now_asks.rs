//! Owner moves retain Inbox's classifier and canonical record routing.
use corral_protocol::rows::{InboxKind, InboxRow};

use crate::model::daemon::Feed;
use crate::model::inbox;
use crate::model::projects::{ProjectFilter, project_chip};
use crate::model::{Id, Live};

use super::{Button, ButtonKind, Press, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NowAskKind {
    Question,
    Plan,
    Verdict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowAskRow {
    pub key: Id,
    pub kind: NowAskKind,
    pub reference: String,
    pub project: Option<String>,
    pub project_chip: Option<String>,
    pub title: String,
    /// Also the phone's explanatory line; never inferred from board status.
    pub consequence: String,
    pub age: String,
    pub selected: bool,
    pub select: Press,
    /// Exactly one primary action, routed through Inbox's target resolver.
    pub button: Button,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowAskPanel {
    pub row: NowAskRow,
    pub asked_by: Option<String>,
    pub asked_at: Option<String>,
    pub link: Option<Button>,
    pub close: Press,
}

pub(super) fn asks(state: &State, feed: &Feed, filter: &ProjectFilter) -> Vec<NowAskRow> {
    inbox::ordered_filtered(feed, filter)
        .into_iter()
        .map(|row| ask(state, feed, filter, row))
        .collect()
}

fn ask(state: &State, feed: &Feed, filter: &ProjectFilter, row: &InboxRow) -> NowAskRow {
    let target = inbox::target(row);
    let target_missing = target.is_none();
    let kind = match &target {
        Some(Live::Plan(_)) => NowAskKind::Plan,
        Some(Live::Review(_)) => NowAskKind::Verdict,
        _ if matches!(row.kind, InboxKind::OwnerDecision | InboxKind::Review) => {
            NowAskKind::Verdict
        }
        _ => NowAskKind::Question,
    };
    let label = match kind {
        NowAskKind::Question => "Answer",
        NowAskKind::Plan => "Review plan",
        NowAskKind::Verdict => "Give verdict",
    };
    let title = row.title.split_whitespace().collect::<Vec<_>>().join(" ");
    let consequence = row
        .attention
        .as_ref()
        .map_or_else(String::new, |attention| {
            let reason = attention.reason.trim();
            let next = attention.next_step.trim();
            let reason = (!reason.is_empty() && !same_text(reason, &title)).then_some(reason);
            let next = (!next.is_empty()
                && !same_text(next, &title)
                && !reason.is_some_and(|reason| same_text(reason, next)))
            .then_some(next);
            match (reason, next) {
                (Some(reason), Some(next)) => format!("{reason} · {next}"),
                (Some(part), None) | (None, Some(part)) => part.to_owned(),
                (None, None) => String::new(),
            }
        });
    let reference = match kind {
        NowAskKind::Plan => row.plan.as_ref().or(row.task.as_ref()),
        _ => row.task.as_ref().or(row.plan.as_ref()),
    }
    .unwrap_or(&row.id)
    .clone();
    let key = Id::from(row.id.as_str());
    NowAskRow {
        reference,
        kind,
        project: row.project.clone(),
        project_chip: project_chip(feed, filter, row.project.as_deref()),
        title,
        consequence,
        age: row
            .asked_at
            .as_deref()
            .map(|at| super::ago(at, feed))
            .unwrap_or_default(),
        selected: state.selected_ask.as_ref() == Some(&key),
        select: Press::SelectAsk(key.clone()),
        button: Button {
            label: label.into(),
            enabled: target.is_some(),
            action: target.map(Press::Open),
            kind: ButtonKind::Primary,
            reason: target_missing.then(|| "The daemon has not supplied a record to open.".into()),
        },
        key,
    }
}

fn same_text(left: &str, right: &str) -> bool {
    left.chars()
        .flat_map(char::to_lowercase)
        .eq(right.chars().flat_map(char::to_lowercase))
}

pub(super) fn panel(feed: &Feed, row: NowAskRow) -> NowAskPanel {
    let source = feed.inbox.get(&row.key);
    let link = source.and_then(|source| {
        source
            .plan
            .as_deref()
            .filter(|_| row.kind == NowAskKind::Plan)
            .map(|id| {
                Button::new(
                    format!("Open {id}"),
                    Press::Open(Live::Plan(id.into())),
                    ButtonKind::Quiet,
                )
            })
            .or_else(|| {
                source.task.as_deref().map(|id| {
                    Button::new(
                        format!("Open {id}"),
                        Press::Open(Live::Task(id.into())),
                        ButtonKind::Quiet,
                    )
                })
            })
            .or_else(|| {
                source.plan.as_deref().map(|id| {
                    Button::new(
                        format!("Open {id}"),
                        Press::Open(Live::Plan(id.into())),
                        ButtonKind::Quiet,
                    )
                })
            })
    });
    NowAskPanel {
        asked_by: source
            .map(|source| source.from.clone())
            .filter(|from| !from.is_empty()),
        asked_at: source
            .and_then(|source| source.asked_at.as_deref())
            .map(|at| super::when(at, feed))
            .filter(|at| !at.is_empty()),
        row,
        link,
        close: Press::ClearSelection,
    }
}
