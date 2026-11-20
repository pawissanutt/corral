//! The shown candidate's tabs and the presses that move between them.

use corral_types::CheckKind;

use crate::model::effect::Effect;
use crate::model::id::Id;
use crate::model::live::Live;

use super::live::Shown;
use super::{Action, State, Tab, TabView};

fn has_summary(shown: &Shown) -> bool {
    shown
        .run()
        .and_then(|run| run.summary.as_deref())
        .is_some_and(|text| !text.trim().is_empty())
}

fn has_artifacts(shown: &Shown) -> bool {
    shown
        .detail
        .is_some_and(|detail| !detail.artifacts.is_empty())
}

fn has_log(shown: &Shown) -> bool {
    shown
        .check(CheckKind::Gate)
        .is_some_and(|gate| gate.log.is_some())
}

pub(super) fn selected_tab(state: &State, shown: &Shown) -> Tab {
    match state.tab {
        Tab::Report if !has_summary(shown) => Tab::Brief,
        Tab::Shot if !has_artifacts(shown) => Tab::Brief,
        Tab::Log if !has_log(shown) => Tab::Brief,
        tab => tab,
    }
}

/// The tabs this candidate has content for. When optional content disappears, Brief is
/// selected without destroying the remembered choice.
pub(super) fn candidate_tabs(state: &State, shown: &Shown) -> Vec<TabView> {
    let selected = selected_tab(state, shown);
    let mut specs = vec![(Tab::Brief, "Brief"), (Tab::Diff, "Diff")];
    if has_log(shown) {
        specs.push((Tab::Log, "Gate log"));
    }
    if has_summary(shown) {
        specs.push((Tab::Report, "Summary"));
    }
    if has_artifacts(shown) {
        specs.push((Tab::Shot, "Artifacts"));
    }
    specs
        .into_iter()
        .map(|(tab, label)| TabView {
            tab,
            label,
            selected: selected == tab,
            action: Action::Tab(tab),
        })
        .collect()
}

pub(super) fn navigation(action: &Action, shown: &Shown) -> Option<Vec<Effect>> {
    match action {
        Action::AskCandidate => Some(vec![Effect::AskAbout {
            context: Some(Live::Review(shown.id().clone()).context(None)),
            text: None,
        }]),
        Action::OpenSession => Some(
            shown
                .session()
                .map(|session| vec![Effect::OpenLive(Live::Session(Id::new(&session)))])
                .unwrap_or_default(),
        ),
        Action::OpenReviewer => Some(
            shown
                .task
                .row
                .reviewer
                .as_ref()
                .map(|run| {
                    let session = format!("{}_{}", shown.id(), run.id);
                    vec![Effect::OpenLive(Live::Session(Id::new(&session)))]
                })
                .unwrap_or_default(),
        ),
        _ => None,
    }
}
