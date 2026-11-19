//! The Board tab of a build: the corral tasks it filed and a proposed plan waiting for the
//! owner, read from its host (`BuildBoard`). This module holds the tab's summary, its badge
//! (tasks not landed, `!` when one is blocked or failed, `◆` while a plan waits and nothing is
//! open) and its unavailable, reading and failed states; the task rows and the plan choice are
//! this module's to add, with their state in `BoardUi` and their actions in `BoardAction`.

use corral_protocol::detail::DetailRef;

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::ActionGroup;
use super::keys::KeyBinding;
use super::tabs::{Badge, PartBody, count, load, part};
use super::{Action, Cx, State};

const CAPABILITY: &str = "board";

/// The Board tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoardUi {}

impl BoardUi {
    pub const EMPTY: Self = Self {};
}

/// What the Board tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardAction {}

#[derive(Debug, Clone, PartialEq)]
pub struct BoardTab {
    pub part: PartBody,
}

pub fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::BuildBoard {
        id: cx.id.to_string(),
    }
}

/// `3 open · 1 blocked · plan waiting`, as the host's board brief counts them.
pub fn summary(cx: &Cx<'_>) -> (String, Hue) {
    let Some(board) = cx.state.and_then(|state| state.board) else {
        return ("board not counted yet".into(), Hue::Neutral);
    };
    let mut parts = vec![format!("{} open", board.open)];
    if board.blocked > 0 {
        parts.push(format!("{} blocked", board.blocked));
    }
    if board.failed > 0 {
        parts.push(format!("{} failed", board.failed));
    }
    if board.proposed {
        parts.push("plan waiting".into());
    }
    let hue = if board.blocked + board.failed > 0 || board.proposed {
        Hue::Held
    } else {
        Hue::Neutral
    };
    (parts.join(" · "), hue)
}

pub fn board_tab(cx: &Cx<'_>) -> BoardTab {
    let (summary, hue) = summary(cx);
    BoardTab {
        part: part(cx, Ok(target(cx)), Some(CAPABILITY), summary, hue),
    }
}

pub fn badge(cx: &Cx<'_>) -> Option<Badge> {
    let board = cx.state?.board?;
    if board.blocked + board.failed > 0 {
        Some(Badge {
            text: format!("{} !", board.open),
            hue: Hue::Held,
        })
    } else if board.proposed && board.open == 0 {
        Some(Badge {
            text: "◆".into(),
            hue: Hue::Held,
        })
    } else {
        count(board.open)
    }
}

/// What the tab reads while it shows.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    load(cx, Ok(target(cx)), Some(CAPABILITY))
        .into_iter()
        .collect()
}

pub(super) fn act(_: &mut State, action: BoardAction, _: &Feed) -> Vec<Effect> {
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
