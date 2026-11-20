//! The board's health over a board place (Home, Tasks): corrald's word that the board failed
//! or is out of date, with its Retry (`btn-retry-board`), the stale board's `as of` stamp, and
//! the dimming of what the place lists while the board is last known. Every word and choice is
//! `work::daemon_status`'s.

use nsh_core::prelude::*;

use crate::model::work::{self, Status};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::notice::notice;
use crate::ui::kit::work::pressable;
use crate::ui::term;

fn status(app: AppState) -> Memo<Status> {
    computed(move || app.feed.with(work::daemon_status))
}

/// The banner with its Retry, then the `as of` stamp; nothing while the board is current.
pub fn board_status(app: AppState) -> AnyElement {
    let status = status(app);
    dynamic(move || {
        let status = status.get();
        if status.banner.is_none() && status.as_of.is_none() {
            return None;
        }
        Some(untracked(move || {
            let banner = status.banner.as_ref().map(|banner| {
                let retry = banner
                    .actions
                    .first()
                    .map(|button| pressable(app, button, false));
                notice(app, banner.hue, &banner.text, retry)
            });
            let stamp = status.as_of.map(|stamp| chip(app, stamp, Hue::Held));
            let frame = match app.form {
                Form::Terminal => view().px(term::cells(1.0)),
                Form::Desktop => view().px(px(20.0)).pt(px(12.0)).gap(px(8.0)),
                Form::Phone => view().px(px(18.0)).pt(px(8.0)).gap(px(8.0)),
            };
            frame
                .test_id("board-status")
                .flex_none()
                .children(banner)
                .children(stamp.map(|stamp| view().flex_row().child(stamp)))
                .into_element()
        }))
    })
    .into_element()
}

/// `content` faded while the board it shows is last known, as Activity › Feed fades its rows.
pub fn dim_stale(app: AppState, content: View) -> View {
    let status = status(app);
    let stale = computed(move || status.with(|status| status.dimmed));
    content.style_when(move || stale.get(), |s| s.saturate(0.35).opacity(0.78))
}
