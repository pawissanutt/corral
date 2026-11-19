//! The Usage view (`Chat › Usage`, opened from the status bar's cost and meter): what the
//! session and its members spent, read from its host (`SessionUsage`). This module holds the
//! view's summary from the host's state (cost, tokens, context use, each absent fact said
//! absent) and its unavailable, reading and failed states; the member rows are this module's to
//! add, with their state in `UsageUi` and their actions in `UsageAction`.

use corral_protocol::detail::DetailRef;

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::{ActionGroup, row};
use super::keys::KeyBinding;
use super::status::{cost_text, tokens};
use super::tabs::{PartBody, load, part};
use super::{Action, Cx, State, SubView};

const CAPABILITY: &str = "usage";

/// The Usage view's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageUi {}

impl UsageUi {
    pub const EMPTY: Self = Self {};
}

/// What the Usage view does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageAction {}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageView {
    pub part: PartBody,
}

pub fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionUsage {
        id: cx.id.to_string(),
    }
}

/// `$1.24+ · 48.0k tokens · ctx 24%`; `No usage recorded yet.` while the host counted nothing.
pub fn summary(cx: &Cx<'_>) -> String {
    let Some(usage) = cx.state.map(|state| &state.usage) else {
        return "usage not reported yet".into();
    };
    if usage.tokens == 0 && usage.cost_usd.is_none() {
        return "No usage recorded yet.".into();
    }
    let context = usage.context_percent.map_or_else(
        || "ctx –".to_owned(),
        |percent| format!("ctx {percent:.0}%"),
    );
    format!(
        "{} · {} tokens · {context}",
        cost_text(usage.cost_usd, usage.cost_partial),
        tokens(usage.tokens)
    )
}

pub fn usage_view(cx: &Cx<'_>) -> UsageView {
    UsageView {
        part: part(
            cx,
            Ok(target(cx)),
            Some(CAPABILITY),
            summary(cx),
            Hue::Neutral,
        ),
    }
}

/// What the view reads while it shows.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    load(cx, Ok(target(cx)), Some(CAPABILITY))
        .into_iter()
        .collect()
}

pub(super) fn act(_: &mut State, action: UsageAction, _: &Feed) -> Vec<Effect> {
    match action {}
}

/// `Open usage` among the views, where the host serves it.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if matches!(cx.can(CAPABILITY), super::Can::Absent) {
        return None;
    }
    Some(ActionGroup {
        title: "Views",
        rows: vec![row(
            "view-usage",
            "Open usage",
            None,
            Action::Sub {
                session: cx.id.clone(),
                sub: Some(SubView::Usage),
            },
        )],
    })
}

/// The view's own keys.
pub fn keys() -> Vec<KeyBinding> {
    Vec::new()
}

pub fn press(_: &str, _: &Cx<'_>) -> Option<Action> {
    None
}
