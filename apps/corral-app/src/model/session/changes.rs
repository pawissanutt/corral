//! The Changes tab: what the session changed, read from its host (`SessionChanges`, the
//! session's own comparison first). This module holds the tab's summary, its badge (the host's
//! file count, `git?` after a failed read) and its unavailable, reading and failed states; the
//! comparisons, the file list and the patches are this module's to add, with their state in
//! `ChangesUi` and their actions in `ChangesAction`.

use corral_protocol::detail::{ChangesComparison, DetailRef};

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::ActionGroup;
use super::keys::KeyBinding;
use super::read_state::ReadState;
use super::tabs::{Badge, PartBody, count, load, part};
use super::{Action, Cx, State};

const CAPABILITY: &str = "changes";

/// The Changes tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangesUi {}

impl ChangesUi {
    pub const EMPTY: Self = Self {};
}

/// What the Changes tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesAction {}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangesTab {
    pub part: PartBody,
}

/// The session's own comparison, which the tab and its count read first.
pub fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionChanges {
        id: cx.id.to_string(),
        comparison: ChangesComparison::Session,
        scope: None,
    }
}

/// `12 files changed`, `no changes yet`; not read while the host has not counted them.
pub fn summary(cx: &Cx<'_>) -> String {
    match cx.state.and_then(|state| state.changes) {
        Some(0) => "no changes yet".into(),
        Some(1) => "1 file changed".into(),
        Some(n) => format!("{n} files changed"),
        None => "changed files not counted yet".into(),
    }
}

pub fn changes_tab(cx: &Cx<'_>) -> ChangesTab {
    ChangesTab {
        part: part(
            cx,
            Ok(target(cx)),
            Some(CAPABILITY),
            summary(cx),
            Hue::Neutral,
        ),
    }
}

/// The host's file count; `git?` once a read of the changes failed.
pub fn badge(cx: &Cx<'_>) -> Option<Badge> {
    if matches!(
        ReadState::of(cx.feed.detail(&target(cx)), cx.feed),
        ReadState::Failed(_)
    ) {
        return Some(Badge {
            text: "git?".into(),
            hue: Hue::Held,
        });
    }
    count(cx.state?.changes?)
}

/// What the tab reads while it shows.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    load(cx, Ok(target(cx)), Some(CAPABILITY))
        .into_iter()
        .collect()
}

pub(super) fn act(_: &mut State, action: ChangesAction, _: &Feed) -> Vec<Effect> {
    match action {}
}

/// The tab's rows in the session's action catalog.
pub fn actions(_: &Cx<'_>) -> Option<ActionGroup> {
    None
}

/// The tab's own keys.
pub fn keys() -> Vec<KeyBinding> {
    Vec::new()
}

pub fn press(_: &str, _: &Cx<'_>) -> Option<Action> {
    None
}
